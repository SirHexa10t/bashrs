//! In-process stream grep on ripgrep's `grep` crate — the engine behind the search commands
//! `hg`, the `g`/`g<N>` family ([`crate::categories::autogen_lookup`]), and `keyword_highlight`
//! ([`crate::categories::styles`]). Those filter a *single* stream (piped input, a file, or inline
//! text), so this is about self-containment — no shelling out to system `grep` — not speed: on one
//! stream the crate and GNU grep are on par (the crate's edge is the parallel directory walk,
//! which these commands don't do).

use std::io::IsTerminal;

use grep::printer::{ColorSpecs, StandardBuilder, UserColorSpec};
use grep::regex::{RegexMatcher, RegexMatcherBuilder};
use grep::searcher::SearcherBuilder;
use termcolor::{ColorChoice, StandardStream};

/// Print the lines of `text` matching `pattern` — literal and case-insensitive, like `grep -iF` —
/// colouring matches, with `context` lines around each (0 = none). Backs `hg` and the `g`/`g<N>`
/// family.
pub(crate) fn filter(pattern: &str, text: &str, context: usize) {
    match RegexMatcherBuilder::new().case_insensitive(true).build(&escape_literal(pattern)) {
        Ok(matcher) => search(&matcher, text, context, false),
        Err(err) => eprintln!("g: could not build matcher: {err}"),
    }
}

/// Print *every* line of `text`, colouring matches of the regex `pattern` — the highlight-don't-
/// filter behaviour of `keyword_highlight` (was `grep -E --color "pattern|$"`). Case-sensitive,
/// matching the original.
pub(crate) fn highlight(pattern: &str, text: &str) {
    match RegexMatcherBuilder::new().build(pattern) {
        Ok(matcher) => search(&matcher, text, 0, true),
        Err(err) => eprintln!("keyword_highlight: invalid regex: {err}"),
    }
}

/// Run `matcher` over `text` and print results like `grep --color=auto` — coloured on a terminal,
/// plain when piped. `context` lines surround each match; in `passthru` mode the non-matching
/// lines are printed too.
fn search(matcher: &RegexMatcher, text: &str, context: usize, passthru: bool) {
    let mut searcher = SearcherBuilder::new()
        .line_number(false)
        .passthru(passthru)
        .before_context(context)
        .after_context(context)
        .build();
    // `--color=auto`: colour matches only when stdout is a terminal, plain text when piped.
    let color = if std::io::stdout().is_terminal() { ColorChoice::Always } else { ColorChoice::Never };
    let mut printer = StandardBuilder::new()
        .color_specs(match_color())
        .build(StandardStream::stdout(color));
    if let Err(err) = searcher.search_slice(matcher, text.as_bytes(), printer.sink(matcher)) {
        eprintln!("grep: {err}");
    }
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
fn escape_literal(pattern: &str) -> String {
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
