//! In-process stream grep on ripgrep's `grep` crate — the engine behind the search commands
//! `hg`, the `g`/`g<N>` family ([`crate::categories::autogen_lookup`]), and `keyword_highlight`
//! ([`crate::categories::styles`]); the matcher builder here ([`build_matcher`]) is also shared with
//! the recursive [`treegrep`](crate::support::treegrep). The commands here filter a *single* stream
//! (piped input, a file, or inline text), so this is about self-containment — no shelling out to
//! system `grep` — not speed: on one stream the crate and GNU grep are on par (the crate's edge is
//! the parallel directory walk, which these commands don't do).

use grep::printer::{ColorSpecs, StandardBuilder, UserColorSpec};
use grep::regex::{RegexMatcher, RegexMatcherBuilder};
use grep::searcher::{Searcher, SearcherBuilder};
use termcolor::{StandardStream, WriteColor};

use crate::support::shell;

/// How a single-stream search runs — the `g` knobs, mirroring [`crate::support::treegrep::Options`]
/// on the recursive side. `Default` is a plain literal search with no context or line numbers, which
/// is exactly what `hg` wants.
#[derive(Default)]
pub(crate) struct Options {
    /// Lines of context around each match (0 = none), like `grep -C`.
    pub context: usize,
    /// Prefix each printed line with its number (`grep -n`).
    pub line_number: bool,
    /// Treat the pattern as a regex rather than literal text (`grep -E`).
    pub regex: bool,
    /// Print the *non*-matching lines instead (`grep -v`).
    pub invert: bool,
}

/// Print the lines of `text` matching any of `patterns` — literal and case-insensitive, like
/// `grep -iF`, with several patterns OR'd like repeated `grep -e` — colouring matches. `opts`
/// carries the `grep`-style knobs: `context` lines around each match (`-C`), `line_number`
/// prefixes (`-n`), `regex` matching (`-E`), and `invert` to print the non-matching lines instead
/// (`-v`). Backs `hg` and the `g`/`g<N>` family.
pub(crate) fn filter(patterns: &[String], text: &[u8], opts: &Options) {
    filter_into(patterns, text, opts, stdout());
}

/// The body of [`filter`], parameterised over the output sink so tests can capture it into an
/// in-memory buffer (production passes [`stdout`]).
fn filter_into<W: WriteColor>(patterns: &[String], text: &[u8], opts: &Options, wtr: W) {
    let matcher = match build_matcher(patterns, opts.regex) {
        Ok(matcher) => matcher,
        Err(err) => return eprintln!("g: could not build matcher: {err}"),
    };
    let mut searcher = SearcherBuilder::new()
        .line_number(opts.line_number)
        .invert_match(opts.invert)
        .before_context(opts.context)
        .after_context(opts.context)
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

/// Stdout as a `--color=auto` sink (the policy lives in [`shell::stdout_color`]).
fn stdout() -> StandardStream {
    StandardStream::stdout(shell::stdout_color())
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

/// Compile `expressions` into one case-insensitive alternation — literal (escaped) by default, or
/// each treated as a regular expression when `regex` is set (`-E`), isolated as `(?:…)` so the
/// alternation can't bleed between them. The one matcher builder behind the `g` and `gg` families.
pub(crate) fn build_matcher(expressions: &[String], regex: bool) -> Result<RegexMatcher, grep::regex::Error> {
    let pattern = expressions
        .iter()
        .map(|expr| if regex { format!("(?:{expr})") } else { regex::escape(expr) })
        .collect::<Vec<_>>()
        .join("|");
    RegexMatcherBuilder::new().case_insensitive(true).build(&pattern)
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
            filter_into(&["<match".into()], b"a <match one\n\xff\xfe junk\nb <match two\n", &Options::default(), w)
        });
        assert!(out.contains("a <match one"), "first match missing: {out:?}");
        assert!(out.contains("b <match two"), "second match missing: {out:?}");
        assert!(!out.contains("junk"), "non-matching line leaked: {out:?}");
    }

    #[test]
    fn invert_match_keeps_only_the_non_matching_lines() {
        // `-v`: the lines *without* the pattern survive; the matching ones are dropped.
        let out = captured(|w| {
            filter_into(&["keep".into()], b"keep me\ndrop this\nkeep again\n", &Options { invert: true, ..Default::default() }, w)
        });
        assert!(out.contains("drop this"), "non-matching line missing: {out:?}");
        assert!(!out.contains("keep me") && !out.contains("keep again"), "matching line leaked: {out:?}");
    }

    #[test]
    fn multiple_patterns_are_ored() {
        // Repeated `-e` terms all match — the single alternation `build_matcher` compiles.
        let out = captured(|w| {
            filter_into(&["alpha".into(), "gamma".into()], b"alpha\nbeta\ngamma\n", &Options::default(), w)
        });
        assert!(out.contains("alpha") && out.contains("gamma"), "{out:?}");
        assert!(!out.contains("beta"), "{out:?}");
    }
}
