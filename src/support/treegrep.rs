//! Recursive directory search — the engine behind the `gg` family ([`crate::categories::autogen_lookup`])
//! and its `GG`/`GGG` variants. Walks a tree with ripgrep's `ignore` walker and searches each file
//! with the `grep` crate: first matching **filenames**, then matching **contents** (`path:line:text`),
//! the expressions compiled by [`streamgrep`]'s shared matcher builder (literal, or regexes under
//! `-E`). Binary files are skipped (NUL detection); over-long lines are swapped for a placeholder
//! (`MAX_LINE`, for minified/dumped text); under `--save`, results also stream into a live file, with
//! a path-sorted `_sorted` sibling left on completion (`SaveSink`). Paths that can't be read for
//! permissions are collected and returned, so the caller can offer a root re-scan (re-exec under the
//! superuser command) scoped to just those paths.
//!
//! Two passes on purpose: the filenames pass reads no file *contents* (only directory metadata), so
//! it's cheap, and running it first gives the "names, then contents" ordering while content matches
//! stream live and memory stays bounded. Like the `find`/`grep -r` it replaces, it searches
//! *everything* — hidden and .gitignored files included.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use grep::matcher::Matcher;
use grep::regex::RegexMatcher;
use grep::searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::{DirEntry, WalkBuilder, WalkState};
use termcolor::{Buffer, BufferWriter, Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::support::delve;
use crate::support::doc_style::_header;
use crate::support::shell;
use crate::support::streamgrep;

/// Options mapped from the `gg` flags (the search roots are passed to [`search`] separately).
pub struct Options {
    pub line_number: bool,
    /// Lines of context to show around each file-content match (0 = none).
    pub context: usize,
    /// Also search *inside* files normally skipped as binary, by decoding known formats — subtitle
    /// tracks, torrent text (`--delve`; see [`crate::support::delve`]).
    pub delve: bool,
    /// Treat the expression(s) as regular expressions instead of literal text (`-E`).
    pub regex: bool,
    /// When set, tee results (plain) into this live file and leave a sorted `_sorted` sibling
    /// ([`sorted_path`]) on completion (`gg --save`).
    pub save: Option<PathBuf>,
}

/// Recursively search `roots` for `expressions` (literal or `-E` regex, case-insensitive, OR'd):
/// print matching filenames, then matching file contents. Output goes to stdout — and, under
/// `--save`, plain to the `opts.save` live file as it arrives, with a path-sorted copy appended to
/// its `_sorted` sibling on completion; diagnostics to stderr. Returns the paths that couldn't be
/// read for permissions, so the caller can offer a root re-scan.
pub(crate) fn search(expressions: &[String], roots: &[PathBuf], opts: &Options) -> BTreeSet<PathBuf> {
    let exprs: Vec<&str> = expressions.iter().map(String::as_str).collect();
    let matcher = match streamgrep::build_matcher(&exprs, opts.regex) {
        Ok(matcher) => matcher,
        Err(err) => {
            eprintln!("gg: invalid expression: {err}");
            return BTreeSet::new();
        }
    };
    if roots.is_empty() {
        return BTreeSet::new();
    }
    // `--save` tees results (plain) into a file as well as the terminal. Opening it appends to what's
    // already there — the header for the first pass, the first pass's results for the root re-scan —
    // so both passes share the one file.
    let save = match opts.save.as_deref() {
        Some(path) => match SaveSink::open(path) {
            Ok(sink) => Some(sink),
            Err(err) => {
                eprintln!("gg: cannot write to {} ({err}); continuing without --save", path.display());
                None
            }
        },
        None => None,
    };
    // Colour follows the usual `--color=auto` rule even under `--save`: the terminal copy is coloured
    // and `SaveSink::record` strips the escapes so the saved file stays plain. Section headings are
    // still suppressed while saving (below), keeping the saved result stream uncluttered and sortable.
    let color = shell::stdout_color();
    let denied: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

    if save.is_none() {
        section("matching filenames", color);
    }
    let names = search_filenames(&matcher, roots, &denied);
    print_filenames(&names, &matcher, color, save.as_ref());

    if save.is_none() {
        section("matching file contents", color);
    }
    let found_contents = search_contents(&matcher, roots, opts, color, &denied, save.as_ref());

    if names.is_empty() && !found_contents {
        eprintln!("NO RESULTS FOUND");
    }
    if let Some(save) = &save {
        if let Err(err) = save.finish() {
            eprintln!("gg: could not sort the saved results: {err}");
        }
    }
    denied.into_inner().unwrap_or_default()
}

/// The file side of `gg --save`. Each result block is appended to the live file as it arrives (so a
/// crash mid-search still leaves everything on disk) and remembered; when the pass finishes, its
/// blocks are appended — sorted by path — to a `_sorted` sibling ([`sorted_path`]). The live file is
/// never rewritten, only appended: it's the crash-safety net, deleted by the caller once the sorted
/// file is complete. A pass owns only its own section (everything from `start`), so the main pass
/// and the root re-scan each sort their own contribution — no cross-process re-parsing of
/// `path:line:text`.
struct SaveSink {
    /// The live (arrival-order) file, append-only.
    live: Mutex<File>,
    /// The live file's path — to locate the `_sorted` sibling and seed it with the header.
    path: PathBuf,
    /// Offset where this pass's output begins; everything before it belongs to earlier passes.
    start: u64,
    /// Each appended block as `(path, bytes)`. A file's matches arrive as one already-line-ordered
    /// block, so ordering the blocks by path gives path-then-line for free.
    blocks: Mutex<Vec<(PathBuf, Vec<u8>)>>,
}

impl SaveSink {
    /// Open `path` for appending, noting where this pass's output will start.
    fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().append(true).create(true).open(path)?;
        let start = file.metadata()?.len();
        Ok(SaveSink {
            live: Mutex::new(file),
            path: path.to_path_buf(),
            start,
            blocks: Mutex::new(Vec::new()),
        })
    }

    /// Append one file's block to the live file and keep it for the final sort. The terminal copy
    /// keeps its colour; here the ANSI escapes are stripped so the saved file is plain text. Empty
    /// blocks skip.
    fn record(&self, path: &Path, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let plain = strip_ansi(bytes);
        let _ = self.live.lock().unwrap().write_all(&plain);
        self.blocks.lock().unwrap().push((path.to_path_buf(), plain));
    }

    /// Append this pass's blocks — ordered by path, stably, so a path's name-match line stays above
    /// its content matches — to the `_sorted` sibling. The first pass creates that file, seeded with
    /// everything before its own section (the header); a later pass (the root re-scan) finds the
    /// earlier sorted output already there and just appends. The live file is left untouched, so no
    /// moment exists where results are only in memory.
    fn finish(&self) -> io::Result<()> {
        let mut blocks = std::mem::take(&mut *self.blocks.lock().unwrap());
        blocks.sort_by(|(a, _), (b, _)| a.cmp(b));
        let sorted = sorted_path(&self.path);
        let seed = !sorted.exists();
        let mut out = OpenOptions::new().append(true).create(true).open(&sorted)?;
        if seed {
            io::copy(&mut File::open(&self.path)?.take(self.start), &mut out)?;
        }
        for (_, bytes) in &blocks {
            out.write_all(bytes)?;
        }
        out.flush()
    }
}

/// The `_sorted` sibling of a `--save` live file — the sorted final artifact [`SaveSink::finish`]
/// writes (and the caller's notice points at, after removing the live file).
pub(crate) fn sorted_path(live: &Path) -> PathBuf {
    let mut name = live.file_name().unwrap_or_default().to_os_string();
    name.push("_sorted");
    live.with_file_name(name)
}

/// Strip ANSI colour escapes — `termcolor`'s SGR sequences — from `bytes`, so the terminal keeps its
/// colour while the saved file stays plain text. Drops every CSI sequence (`ESC [` … final byte); on
/// this Linux-only tool `termcolor` emits nothing else.
///
/// Hand-rolled rather than `console::strip_ansi_codes` (already in-tree via `table_formatter`): that
/// takes `&str`, but our buffer can hold non-UTF-8 match bytes — the case `g`/`gg` are byte-oriented
/// for — which the required lossy UTF-8 conversion would corrupt.
fn strip_ansi(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            i += 2;
            while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                i += 1;
            }
            i += 1; // consume the final byte (e.g. `m`)
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// Phase 1 — every entry (file or directory) whose *name* matches. Reads no file contents.
fn search_filenames(
    matcher: &RegexMatcher,
    roots: &[PathBuf],
    denied: &Mutex<BTreeSet<PathBuf>>,
) -> Vec<PathBuf> {
    let hits: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    walk(roots).build_parallel().run(|| {
        let hits = &hits;
        Box::new(move |result| {
            name_hit(result, matcher, hits, denied);
            WalkState::Continue
        })
    });
    let mut hits = hits.into_inner().unwrap_or_default();
    hits.sort();
    hits
}

/// Record one walk entry whose name matches (or its error, if it was a permission denial).
fn name_hit(
    result: Result<DirEntry, ignore::Error>,
    matcher: &RegexMatcher,
    hits: &Mutex<Vec<PathBuf>>,
    denied: &Mutex<BTreeSet<PathBuf>>,
) {
    match result {
        Ok(entry) => {
            if let Some(name) = entry.file_name().to_str() {
                if matcher.is_match(name.as_bytes()).unwrap_or(false) {
                    hits.lock().unwrap().push(entry.path().to_path_buf());
                }
            }
        }
        Err(err) => record_walk_denied(&err, denied),
    }
}

/// Print matching paths, colouring the matched span (black-on-red, on a terminal).
fn print_filenames(names: &[PathBuf], matcher: &RegexMatcher, color: ColorChoice, save: Option<&SaveSink>) {
    let mut out = StandardStream::stdout(color);
    let match_style = match_spec();
    for path in names {
        let text = path.to_string_lossy();
        let bytes = text.as_bytes();
        let _ = write_spans(&mut out, matcher, bytes, &match_style);
        let _ = out.write_all(b"\n");
        // Under `--save`, a name match is a one-line block keyed by its path (plain — no colour).
        if let Some(save) = save {
            let mut line = bytes.to_vec();
            line.push(b'\n');
            save.record(path, &line);
        }
    }
}

/// Phase 2 — matching file contents, printed `path:line:text` and streamed as each file completes.
/// Returns whether anything matched.
fn search_contents(
    matcher: &RegexMatcher,
    roots: &[PathBuf],
    opts: &Options,
    color: ColorChoice,
    denied: &Mutex<BTreeSet<PathBuf>>,
    save: Option<&SaveSink>,
) -> bool {
    let bufwtr = BufferWriter::stdout(color);
    let found = AtomicBool::new(false);
    let ctx = Ctx { matcher, bufwtr: &bufwtr, found: &found, denied, delve: opts.delve, save };
    let line_number = opts.line_number;
    let context = opts.context;

    walk(roots).build_parallel().run(|| {
        let ctx = &ctx;
        let mut searcher = build_searcher(line_number, context);
        let mut buffer = ctx.bufwtr.buffer();
        Box::new(move |result| {
            match result {
                Ok(entry) => scan_file(&entry, &mut searcher, &mut buffer, ctx),
                Err(err) => record_walk_denied(&err, ctx.denied),
            }
            WalkState::Continue
        })
    });
    found.load(Ordering::Relaxed)
}

/// The per-file-invariant context for the content search, bundled so `scan_file` stays small and
/// both walks pass exactly the same thing.
struct Ctx<'a> {
    matcher: &'a RegexMatcher,
    bufwtr: &'a BufferWriter,
    found: &'a AtomicBool,
    denied: &'a Mutex<BTreeSet<PathBuf>>,
    delve: bool,
    save: Option<&'a SaveSink>,
}

/// A searcher that skips binary files (NUL detection) and numbers lines when asked.
fn build_searcher(line_number: bool, context: usize) -> Searcher {
    SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(line_number)
        .before_context(context)
        .after_context(context)
        .build()
}

/// The longest match/context line `gg` prints in full; anything longer is replaced by [`TOO_LONG`],
/// so a match inside minified or dumped text can't flood the terminal with one giant line.
const MAX_LINE: usize = 2000;
const TOO_LONG: &[u8] = b"<match found, but result was too long to display>";

/// A `grep` sink for `gg`'s output: `path:line:text` for matches, `path-line-text` for context, with
/// magenta paths, yellow line numbers, and black-on-red match spans — and any line over [`MAX_LINE`]
/// chars swapped for [`TOO_LONG`]. It writes to a `termcolor` buffer, so colour is emitted only when
/// that buffer is in colour mode (a terminal); piped output stays plain.
struct GgSink<'a> {
    matcher: &'a RegexMatcher,
    buffer: &'a mut Buffer,
    path: Vec<u8>,
    path_color: ColorSpec,
    line_color: ColorSpec,
    match_color: ColorSpec,
}

impl<'a> GgSink<'a> {
    fn new(matcher: &'a RegexMatcher, buffer: &'a mut Buffer, path: &Path) -> Self {
        GgSink {
            matcher,
            buffer,
            path: path.to_string_lossy().into_owned().into_bytes(),
            path_color: spec(Color::Magenta, None),
            line_color: spec(Color::Yellow, None),
            match_color: match_spec(),
        }
    }

    /// Write one line as `path<sep>line<sep>text` (`sep` is `:` for a match, `-` for context). An
    /// over-long line becomes [`TOO_LONG`]; a match line has its match spans highlighted.
    fn write_line(&mut self, raw: &[u8], line_number: Option<u64>, is_match: bool) -> io::Result<()> {
        let sep = if is_match { &b":"[..] } else { &b"-"[..] };
        field(self.buffer, &self.path_color, &self.path)?;
        self.buffer.write_all(sep)?;
        if let Some(n) = line_number {
            field(self.buffer, &self.line_color, n.to_string().as_bytes())?;
            self.buffer.write_all(sep)?;
        }
        let content = trim_eol(raw);
        if content.len() > MAX_LINE {
            self.buffer.write_all(TOO_LONG)?;
        } else if is_match {
            write_spans(self.buffer, self.matcher, content, &self.match_color)?;
        } else {
            self.buffer.write_all(content)?;
        }
        self.buffer.write_all(b"\n")
    }

}

impl Sink for GgSink<'_> {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> io::Result<bool> {
        let mut line_number = mat.line_number();
        for line in mat.lines() {
            self.write_line(line, line_number, true)?;
            line_number = line_number.map(|n| n + 1);
        }
        Ok(true)
    }

    fn context(&mut self, _searcher: &Searcher, ctx: &SinkContext<'_>) -> io::Result<bool> {
        self.write_line(ctx.bytes(), ctx.line_number(), false)?;
        Ok(true)
    }

    fn context_break(&mut self, _searcher: &Searcher) -> io::Result<bool> {
        self.buffer.write_all(b"--\n")?;
        Ok(true)
    }
}

/// A `ColorSpec` with the given foreground (and optional background).
fn spec(fg: Color, bg: Option<Color>) -> ColorSpec {
    let mut spec = ColorSpec::new();
    spec.set_fg(Some(fg)).set_bg(bg);
    spec
}

/// The black-on-red match style, shared by the filename listing and the content sink (and mirrored
/// by `g`'s printer colours in [`streamgrep`]).
fn match_spec() -> ColorSpec {
    spec(Color::Black, Some(Color::Red))
}

/// Write `bytes` in `color`, then reset. Colour codes are emitted only if `out` is in colour mode.
fn field(out: &mut impl WriteColor, color: &ColorSpec, bytes: &[u8]) -> io::Result<()> {
    out.set_color(color)?;
    out.write_all(bytes)?;
    out.reset()
}

/// Write `bytes` with each of `matcher`'s match spans coloured `spec` — the one span-splitting loop
/// behind both the filename listing and the match-line highlighting.
fn write_spans(out: &mut impl WriteColor, matcher: &RegexMatcher, bytes: &[u8], spec: &ColorSpec) -> io::Result<()> {
    let mut spans = Vec::new();
    let _ = matcher.find_iter(bytes, |m| {
        spans.push((m.start(), m.end()));
        true
    });
    let mut last = 0;
    for (start, end) in spans {
        out.write_all(&bytes[last..start])?;
        field(out, spec, &bytes[start..end])?;
        last = end;
    }
    out.write_all(&bytes[last..])
}

/// A line without its trailing `\n` / `\r\n`.
fn trim_eol(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    if end > 0 && line[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && line[end - 1] == b'\r' {
        end -= 1;
    }
    &line[..end]
}

/// Search one file into `buffer`, then flush it to stdout atomically.
fn scan_file(entry: &DirEntry, searcher: &mut Searcher, buffer: &mut Buffer, ctx: &Ctx) {
    if !entry.file_type().is_some_and(|ft| ft.is_file()) {
        return;
    }
    buffer.clear();
    // In `--delve` mode, a binary format we understand (subtitle tracks, torrent text) is decoded
    // and *that* is searched; files with no decoder fall through to the normal (raw) search.
    if ctx.delve {
        if let Some(text) = delve::extract(entry.path()) {
            delve_search(&text, entry.path(), buffer, ctx);
            emit(entry.path(), buffer, ctx);
            return;
        }
    }
    {
        let sink = GgSink::new(ctx.matcher, buffer, entry.path());
        if let Err(err) = searcher.search_path(ctx.matcher, entry.path(), sink) {
            if err.kind() == io::ErrorKind::PermissionDenied {
                ctx.denied.lock().unwrap().insert(entry.path().to_path_buf());
            }
        }
    } // sink dropped → the &mut borrow of `buffer` ends
    emit(entry.path(), buffer, ctx);
}

/// Search text decoded by [`delve`] for one file, printing matches as `path:text` — no line numbers,
/// since the decoded lines don't line up with lines in the original binary file.
fn delve_search(text: &[u8], path: &Path, buffer: &mut Buffer, ctx: &Ctx) {
    let mut searcher =
        SearcherBuilder::new().binary_detection(BinaryDetection::none()).line_number(false).build();
    let sink = GgSink::new(ctx.matcher, buffer, path);
    let _ = searcher.search_slice(ctx.matcher, text, sink);
}

/// Send a completed file's buffer to the terminal and — under `--save` — to the save file plus the
/// sort set. The single funnel for both the delve and raw-search paths.
fn emit(path: &Path, buffer: &Buffer, ctx: &Ctx) {
    flush(buffer, ctx);
    if let Some(save) = ctx.save {
        save.record(path, buffer.as_slice());
    }
}

/// Flush a completed file's buffer to stdout atomically, recording that something matched.
fn flush(buffer: &Buffer, ctx: &Ctx) {
    if !buffer.as_slice().is_empty() {
        ctx.found.store(true, Ordering::Relaxed);
        let _ = ctx.bufwtr.print(buffer);
    }
}

/// A walker configured to search *everything* — hidden and ignored files included (matching the
/// old `find`/`grep -r`), unlike ripgrep's gitignore-aware default.
fn walk(roots: &[PathBuf]) -> WalkBuilder {
    let mut builder = WalkBuilder::new(&roots[0]);
    for root in &roots[1..] {
        builder.add(root);
    }
    builder.standard_filters(false);
    builder
}

/// Record a walk error's path if it was a permission denial (a directory we couldn't descend).
fn record_walk_denied(err: &ignore::Error, denied: &Mutex<BTreeSet<PathBuf>>) {
    let is_permission = err.io_error().is_some_and(|e| e.kind() == io::ErrorKind::PermissionDenied);
    if is_permission {
        if let Some(path) = error_path(err) {
            denied.lock().unwrap().insert(path.to_path_buf());
        }
    }
}

/// Dig through `ignore::Error`'s wrapping variants (depth / line-number / partial) to the
/// underlying path, if the error carries one.
fn error_path(err: &ignore::Error) -> Option<&Path> {
    match err {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            error_path(err)
        }
        ignore::Error::Partial(errs) => errs.iter().find_map(error_path),
        _ => None,
    }
}

/// Print a section header — the shared bold-blue "header" style on a terminal, plain when piped.
fn section(title: &str, color: ColorChoice) {
    let line = format!("{title}:");
    println!("\n{}", if color == ColorChoice::Always { _header(&line) } else { line });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::shell::captured;

    /// Run `gg`'s real sink over `text` (matching `pattern`) and return the captured output —
    /// colour stripped, so assertions read as the plain `path<sep>line<sep>text` lines.
    fn gg_output(pattern: &str, text: &[u8], line_numbers: bool, context: usize) -> String {
        let matcher = grep::regex::RegexMatcherBuilder::new().build(pattern).unwrap();
        let mut searcher = build_searcher(line_numbers, context);
        captured(|buf| {
            let sink = GgSink::new(&matcher, buf, Path::new("f"));
            searcher.search_slice(&matcher, text, sink).unwrap();
        })
    }

    #[test]
    fn formats_a_match_as_path_line_text() {
        // Only the matching line is emitted, as `path:line:text`.
        assert_eq!(gg_output("match", b"first\nsecond match\n", true, 0), "f:2:second match\n");
    }

    #[test]
    fn context_lines_use_a_dash_separator() {
        // Match on line 2 with one line of context each side: the match is `path:line:`, the
        // surrounding context is `path-line-`.
        let out = gg_output("match", b"above\nthe match\nbelow\n", true, 1);
        assert_eq!(out, "f-1-above\nf:2:the match\nf-3-below\n");
    }

    #[test]
    fn an_over_long_line_becomes_the_placeholder() {
        // A match line longer than `MAX_LINE` is swapped for `TOO_LONG`, so a hit inside minified or
        // dumped text can't flood the terminal with one giant line.
        let line = format!("match{}", "x".repeat(MAX_LINE));
        let out = gg_output("match", format!("{line}\n").as_bytes(), false, 0);
        assert_eq!(out, format!("f:{}\n", std::str::from_utf8(TOO_LONG).unwrap()));
    }

    #[test]
    fn every_segment_of_a_multi_match_line_is_written() {
        // Two matches on one line: the span-splitting in `write_highlighted` must reproduce the
        // line exactly (colour stripped here), with nothing dropped or duplicated at the boundaries.
        assert_eq!(gg_output("aa", b"aa bb aa cc\n", false, 0), "f:aa bb aa cc\n");
    }

    #[test]
    fn strip_ansi_drops_colour_escapes_only() {
        // `--save` keeps colour on screen but stores plain text: a coloured `path:line:text` line
        // reduces to the plain text, while text with no escapes comes back untouched.
        let coloured = b"\x1b[35mf\x1b[0m:\x1b[33m2\x1b[0m:a \x1b[30m\x1b[41mmatch\x1b[0m here\n";
        assert_eq!(strip_ansi(coloured), b"f:2:a match here\n".to_vec());
        assert_eq!(strip_ansi(b"plain, no escapes"), b"plain, no escapes".to_vec());
    }

    #[test]
    fn save_sink_logs_live_then_leaves_a_sorted_sibling() {
        let live = std::env::temp_dir().join(format!("bashrs_save_sink_{}", std::process::id()));
        let sorted = sorted_path(&live);
        assert!(sorted.to_string_lossy().ends_with("_sorted"), "sibling name: {sorted:?}");
        let _ = std::fs::remove_file(&sorted); // a stale sibling would corrupt the seed check
        // First pass: the live file starts with the header `_gg` writes; blocks arrive out of order.
        std::fs::write(&live, "# search term used: x\n").unwrap();
        let first = SaveSink::open(&live).unwrap();
        first.record(Path::new("/z"), b"\x1b[35m/z\x1b[0m:1:zed\n"); // coloured, like a real block
        first.record(Path::new("/a"), b"/a:1:first\n/a:2:second\n");
        first.record(Path::new("/m"), b""); // an empty block is skipped entirely
        first.finish().unwrap();
        // Second pass (the root re-scan): appends to the same live file, sorts only its own section.
        let second = SaveSink::open(&live).unwrap();
        second.record(Path::new("/root/b"), b"/root/b:1:late\n");
        second.record(Path::new("/root/a"), b"/root/a:1:early\n");
        second.finish().unwrap();
        // The live file is a pure arrival-order log (ANSI stripped), never rewritten…
        assert_eq!(
            std::fs::read_to_string(&live).unwrap(),
            "# search term used: x\n/z:1:zed\n/a:1:first\n/a:2:second\n/root/b:1:late\n/root/a:1:early\n",
        );
        // …while the `_sorted` sibling holds the header, then each pass's section sorted by path.
        assert_eq!(
            std::fs::read_to_string(&sorted).unwrap(),
            "# search term used: x\n/a:1:first\n/a:2:second\n/z:1:zed\n/root/a:1:early\n/root/b:1:late\n",
        );
        let _ = std::fs::remove_file(&live);
        let _ = std::fs::remove_file(&sorted);
    }
}
