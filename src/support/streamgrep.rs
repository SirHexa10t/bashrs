//! In-process stream grep on ripgrep's `grep` crate — the engine behind the search commands
//! `hg`, the `g`/`g<N>` family ([`crate::categories::autogen_lookup`]), and `keyword_highlight`
//! ([`crate::categories::styles`]). Those filter a *single* stream (piped input, a file, or inline
//! text), so this is about self-containment — no shelling out to system `grep` — not speed: on one
//! stream the crate and GNU grep are on par (the crate's edge is the parallel directory walk,
//! which these commands don't do).

use std::io::IsTerminal;

use grep::printer::{ColorSpecs, StandardBuilder, UserColorSpec};
use grep::regex::{RegexMatcher, RegexMatcherBuilder};
use grep::searcher::{Searcher, SearcherBuilder};
use termcolor::{ColorChoice, StandardStream, WriteColor};

/// Print the lines of `text` matching `pattern` — literal and case-insensitive, like `grep -iF` —
/// colouring matches, with `context` lines around each (0 = none). With `line_number`, each line is
/// prefixed with its number (`grep -n`). Backs `hg` and the `g`/`g<N>` family.
pub(crate) fn filter(pattern: &str, text: &[u8], context: usize, line_number: bool) {
    filter_into(pattern, text, context, line_number, stdout());
}

/// The body of [`filter`], parameterised over the output sink so tests can capture it into an
/// in-memory buffer (production passes [`stdout`]).
fn filter_into<W: WriteColor>(pattern: &str, text: &[u8], context: usize, line_number: bool, wtr: W) {
    let matcher = match RegexMatcherBuilder::new().case_insensitive(true).build(&escape_literal(pattern)) {
        Ok(matcher) => matcher,
        Err(err) => return eprintln!("g: could not build matcher: {err}"),
    };
    let mut searcher = SearcherBuilder::new()
        .line_number(line_number)
        .before_context(context)
        .after_context(context)
        .build();
    search(&matcher, text, &mut searcher, wtr);
}

/// Print *every* line of `text`, colouring matches of the regex `pattern` — the highlight-don't-
/// filter behaviour of `keyword_highlight` (was `grep -E --color "pattern|$"`). Case-sensitive,
/// matching the original.
pub(crate) fn highlight(pattern: &str, text: &[u8]) {
    let matcher = match RegexMatcherBuilder::new().build(pattern) {
        Ok(matcher) => matcher,
        Err(err) => return eprintln!("keyword_highlight: invalid regex: {err}"),
    };
    let mut searcher = SearcherBuilder::new().passthru(true).line_number(false).build();
    search(&matcher, text, &mut searcher, stdout());
}

/// Run `searcher` over `text`, printing matches to `wtr` (`grep`-style). Generic over the sink so
/// production writes to the terminal ([`stdout`]) while tests capture into an in-memory buffer.
fn search<W: WriteColor>(matcher: &RegexMatcher, text: &[u8], searcher: &mut Searcher, wtr: W) {
    let mut printer = StandardBuilder::new().color_specs(match_color()).build(wtr);
    if let Err(err) = searcher.search_slice(matcher, text, printer.sink(matcher)) {
        eprintln!("grep: {err}");
    }
}

/// Stdout as a `--color=auto` sink: coloured matches when it's a terminal, plain text when piped.
fn stdout() -> StandardStream {
    let color = if std::io::stdout().is_terminal() { ColorChoice::Always } else { ColorChoice::Never };
    StandardStream::stdout(color)
}

/// Colour matches black-on-red: a red *background* with black text, not red text. A filled block
/// breaks the glyphs' pattern and draws the eye far better than recolouring the characters — and it
/// mirrors the old `GREP_COLORS='mt=7;31'` look (reverse-video red renders as a red background on a
/// dark terminal). termcolor has no reverse style, and its default foreground is the terminal's
/// text colour (not black), so we set both background and foreground explicitly.
fn match_color() -> ColorSpecs {
    let specs: Vec<UserColorSpec> = ["match:bg:red", "match:fg:black"]
        .iter()
        .map(|spec| spec.parse().expect("built-in colour spec is valid"))
        .collect();
    ColorSpecs::new(&specs)
}

/// Escape regex metacharacters so PATTERN matches literally (`-F`). Mirrors `regex::escape`
/// without the `regex` facade crate, which would fatten the shared regex build `bashrs` links via
/// `synoptic`.
pub(crate) fn escape_literal(pattern: &str) -> String {
    const META: &[char] = &[
        '\\', '.', '+', '*', '?', '(', ')', '|', '[', ']', '{', '}', '^', '$', '#', '&', '-', '~',
    ];
    let mut escaped = String::with_capacity(pattern.len());
    for ch in pattern.chars() {
        if META.contains(&ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::shell::captured;

    #[test]
    fn finds_a_literal_match_inside_non_utf8_data() {
        // The `0xFF 0xFE` (invalid UTF-8) between two matching lines is exactly what broke `g`
        // before: both `<match` lines are still found, the stream isn't rejected, and the
        // non-matching line is dropped — GNU-grep behaviour on a mixed text/binary stream.
        let out = captured(|w| {
            filter_into("<match", b"a <match one\n\xff\xfe junk\nb <match two\n", 0, false, w)
        });
        assert!(out.contains("a <match one"), "first match missing: {out:?}");
        assert!(out.contains("b <match two"), "second match missing: {out:?}");
        assert!(!out.contains("junk"), "non-matching line leaked: {out:?}");
    }
}
