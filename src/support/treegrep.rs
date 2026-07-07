//! Recursive directory search — the engine behind `gg` ([`crate::categories::lookup`]). Walks a
//! tree with ripgrep's `ignore` walker and searches each file with the `grep` crate: first matching
//! **filenames**, then matching **contents** (`path:line:text`). Binary files are skipped (NUL
//! detection); over-long lines are omitted (`text_limit`, for minified/dumped text). Paths that
//! can't be read for permissions are collected and reported — an opt-in root re-scan of just those
//! is the planned follow-up.
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
use grep::printer::{ColorSpecs, StandardBuilder, UserColorSpec};
use grep::regex::{RegexMatcher, RegexMatcherBuilder};
use grep::searcher::{BinaryDetection, Searcher, SearcherBuilder};
use ignore::{DirEntry, WalkBuilder, WalkState};
use termcolor::{Buffer, BufferWriter, Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::support::doc_style::_header;
use crate::support::streamgrep;

/// Options mapped from the `gg` flags.
pub struct Options {
    pub dir: String,
    /// Omit matches on lines longer than this many bytes; `None` = no limit.
    pub text_limit: Option<u64>,
    pub line_number: bool,
    /// Lines of context to show around each file-content match (0 = none).
    pub context: usize,
}

/// Search `dir` recursively for `expressions` (literal, case-insensitive, OR'd): print matching
/// filenames, then matching file contents. Everything goes to stdout; diagnostics to stderr.
pub(crate) fn search(expressions: &[String], opts: &Options) {
    let matcher = match build_matcher(expressions) {
        Ok(matcher) => matcher,
        Err(err) => return eprintln!("gg: invalid expression: {err}"),
    };
    let color = std::io::stdout().is_terminal();
    let denied: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

    section("matching filenames", color);
    let names = search_filenames(&matcher, opts, &denied);
    print_filenames(&names, &matcher, color);

    section("matching file contents", color);
    let found_contents = search_contents(&matcher, opts, color, &denied);

    if names.is_empty() && !found_contents {
        eprintln!("NO RESULTS FOUND");
    }
    report_denied(&denied.into_inner().unwrap_or_default());
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
    opts: &Options,
    denied: &Mutex<BTreeSet<PathBuf>>,
) -> Vec<PathBuf> {
    let hits: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    walk(&opts.dir).build_parallel().run(|| {
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
    opts: &Options,
    color: bool,
    denied: &Mutex<BTreeSet<PathBuf>>,
) -> bool {
    let bufwtr = BufferWriter::stdout(if color { ColorChoice::Always } else { ColorChoice::Never });
    let found = AtomicBool::new(false);
    let ctx = Ctx { matcher, bufwtr: &bufwtr, text_limit: opts.text_limit, found: &found, denied };
    let line_number = opts.line_number;
    let context = opts.context;

    walk(&opts.dir).build_parallel().run(|| {
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
    text_limit: Option<u64>,
    found: &'a AtomicBool,
    denied: &'a Mutex<BTreeSet<PathBuf>>,
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

/// Colours for `gg`'s `path:line:text` output: magenta (purple) paths, yellow line numbers, and
/// matches black-on-red (the shared highlight). The `grep` printer resolves these `ColorSpecs`.
fn content_color() -> ColorSpecs {
    let specs: Vec<UserColorSpec> =
        ["path:fg:magenta", "line:fg:yellow", "match:bg:red", "match:fg:black"]
            .iter()
            .map(|spec| spec.parse().expect("built-in colour spec is valid"))
            .collect();
    ColorSpecs::new(&specs)
}

/// Search one file into `buffer`, then flush it to stdout atomically.
fn scan_file(entry: &DirEntry, searcher: &mut Searcher, buffer: &mut Buffer, ctx: &Ctx) {
    if !entry.file_type().is_some_and(|ft| ft.is_file()) {
        return;
    }
    buffer.clear();
    {
        let mut printer = StandardBuilder::new()
            .color_specs(content_color())
            .heading(false)
            .max_columns(ctx.text_limit)
            .build(&mut *buffer);
        let sink = printer.sink_with_path(ctx.matcher, entry.path());
        if let Err(err) = searcher.search_path(ctx.matcher, entry.path(), sink) {
            if err.kind() == io::ErrorKind::PermissionDenied {
                ctx.denied.lock().unwrap().insert(entry.path().to_path_buf());
            }
        }
    } // printer dropped → the &mut borrow of `buffer` ends
    if !buffer.as_slice().is_empty() {
        ctx.found.store(true, Ordering::Relaxed);
        let _ = ctx.bufwtr.print(buffer);
    }
}

/// A walker configured to search *everything* — hidden and ignored files included (matching the
/// old `find`/`grep -r`), unlike ripgrep's gitignore-aware default.
fn walk(dir: &str) -> WalkBuilder {
    let mut builder = WalkBuilder::new(dir);
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

/// List the paths skipped for permissions — the input the planned root re-scan will consume.
fn report_denied(denied: &BTreeSet<PathBuf>) {
    if denied.is_empty() {
        return;
    }
    eprintln!("\n{} path(s) skipped (permission denied):", denied.len());
    for path in denied {
        eprintln!("  {}", path.display());
    }
    eprintln!("(re-running these as root isn't wired up yet)");
}
