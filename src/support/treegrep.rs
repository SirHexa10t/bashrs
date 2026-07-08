//! Recursive directory search — the engine behind `gg` ([`crate::categories::autogen_lookup`]). Walks a
//! tree with ripgrep's `ignore` walker and searches each file with the `grep` crate: first matching
//! **filenames**, then matching **contents** (`path:line:text`). Binary files are skipped (NUL
//! detection); over-long lines are omitted (`text_limit`, for minified/dumped text). Paths that
//! can't be read for permissions are collected and returned, so the caller can offer a root re-scan
//! (re-exec under the superuser command) scoped to just those paths.
//!
//! Two passes on purpose: the filenames pass reads no file *contents* (only directory metadata), so
//! it's cheap, and running it first gives the "names, then contents" ordering while content matches
//! stream live and memory stays bounded. Like the `find`/`grep -r` it replaces, it searches
//! *everything* — hidden and .gitignored files included.

use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use grep::matcher::Matcher;
use grep::regex::{RegexMatcher, RegexMatcherBuilder};
use grep::searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::{DirEntry, WalkBuilder, WalkState};
use termcolor::{Buffer, BufferWriter, Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::support::delve;
use crate::support::doc_style::_header;
use crate::support::streamgrep;

/// Options mapped from the `gg` flags (the search roots are passed to [`search`] separately).
pub struct Options {
    pub line_number: bool,
    /// Lines of context to show around each file-content match (0 = none).
    pub context: usize,
    /// Also search *inside* files normally skipped as binary, by decoding known formats — subtitle
    /// tracks, torrent text (`--delve`; see [`crate::support::delve`]).
    pub delve: bool,
}

/// Recursively search `roots` for `expressions` (literal, case-insensitive, OR'd): print matching
/// filenames, then matching file contents. Everything goes to stdout; diagnostics to stderr.
/// Returns the paths that couldn't be read for permissions, so the caller can offer a root re-scan.
pub(crate) fn search(expressions: &[String], roots: &[PathBuf], opts: &Options) -> BTreeSet<PathBuf> {
    let matcher = match build_matcher(expressions) {
        Ok(matcher) => matcher,
        Err(err) => {
            eprintln!("gg: invalid expression: {err}");
            return BTreeSet::new();
        }
    };
    if roots.is_empty() {
        return BTreeSet::new();
    }
    let color = std::io::stdout().is_terminal();
    let denied: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

    section("matching filenames", color);
    let names = search_filenames(&matcher, roots, &denied);
    print_filenames(&names, &matcher, color);

    section("matching file contents", color);
    let found_contents = search_contents(&matcher, roots, opts, color, &denied);

    if names.is_empty() && !found_contents {
        eprintln!("NO RESULTS FOUND");
    }
    denied.into_inner().unwrap_or_default()
}

/// Compile the expressions into one case-insensitive, literal (escaped) alternation.
fn build_matcher(expressions: &[String]) -> Result<RegexMatcher, grep::regex::Error> {
    let pattern = expressions
        .iter()
        .map(|expr| streamgrep::escape_literal(expr))
        .collect::<Vec<_>>()
        .join("|");
    RegexMatcherBuilder::new().case_insensitive(true).build(&pattern)
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
fn print_filenames(names: &[PathBuf], matcher: &RegexMatcher, color: bool) {
    let mut out = StandardStream::stdout(if color { ColorChoice::Always } else { ColorChoice::Never });
    let mut spec = ColorSpec::new();
    spec.set_fg(Some(Color::Black)).set_bg(Some(Color::Red));
    for path in names {
        let text = path.to_string_lossy();
        let bytes = text.as_bytes();
        let mut ranges = Vec::new();
        let _ = matcher.find_iter(bytes, |m| {
            ranges.push((m.start(), m.end()));
            true
        });
        let mut last = 0;
        for (start, end) in ranges {
            let _ = out.write_all(&bytes[last..start]);
            let _ = out.set_color(&spec);
            let _ = out.write_all(&bytes[start..end]);
            let _ = out.reset();
            last = end;
        }
        let _ = out.write_all(&bytes[last..]);
        let _ = out.write_all(b"\n");
    }
}

/// Phase 2 — matching file contents, printed `path:line:text` and streamed as each file completes.
/// Returns whether anything matched.
fn search_contents(
    matcher: &RegexMatcher,
    roots: &[PathBuf],
    opts: &Options,
    color: bool,
    denied: &Mutex<BTreeSet<PathBuf>>,
) -> bool {
    let bufwtr = BufferWriter::stdout(if color { ColorChoice::Always } else { ColorChoice::Never });
    let found = AtomicBool::new(false);
    let ctx = Ctx { matcher, bufwtr: &bufwtr, found: &found, denied, delve: opts.delve };
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
        let color = |fg, bg| {
            let mut spec = ColorSpec::new();
            spec.set_fg(Some(fg)).set_bg(bg);
            spec
        };
        GgSink {
            matcher,
            buffer,
            path: path.to_string_lossy().into_owned().into_bytes(),
            path_color: color(Color::Magenta, None),
            line_color: color(Color::Yellow, None),
            match_color: color(Color::Black, Some(Color::Red)),
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
            self.write_highlighted(content)?;
        } else {
            self.buffer.write_all(content)?;
        }
        self.buffer.write_all(b"\n")
    }

    /// Write `content`, colouring each match span black-on-red.
    fn write_highlighted(&mut self, content: &[u8]) -> io::Result<()> {
        let mut spans = Vec::new();
        let _ = self.matcher.find_iter(content, |m| {
            spans.push((m.start(), m.end()));
            true
        });
        let mut last = 0;
        for (start, end) in spans {
            self.buffer.write_all(&content[last..start])?;
            field(self.buffer, &self.match_color, &content[start..end])?;
            last = end;
        }
        self.buffer.write_all(&content[last..])
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

/// Write `bytes` in `color`, then reset. Colour codes are emitted only if `buf` is in colour mode.
fn field(buf: &mut Buffer, color: &ColorSpec, bytes: &[u8]) -> io::Result<()> {
    buf.set_color(color)?;
    buf.write_all(bytes)?;
    buf.reset()
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
            flush(buffer, ctx);
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
    flush(buffer, ctx);
}

/// Search text decoded by [`delve`] for one file, printing matches as `path:text` — no line numbers,
/// since the decoded lines don't line up with lines in the original binary file.
fn delve_search(text: &[u8], path: &Path, buffer: &mut Buffer, ctx: &Ctx) {
    let mut searcher =
        SearcherBuilder::new().binary_detection(BinaryDetection::none()).line_number(false).build();
    let sink = GgSink::new(ctx.matcher, buffer, path);
    let _ = searcher.search_slice(ctx.matcher, text, sink);
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
fn section(title: &str, color: bool) {
    let line = format!("{title}:");
    println!("\n{}", if color { _header(&line) } else { line });
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
}
