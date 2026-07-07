//! Hand-written style commands — the `StyleCommand` category: one-off styling verbs
//! (`errcho`), a `synoptic`-backed `code_highlight`, and a `grep`-crate-backed `keyword_highlight`,
//! none part of the generated `recho` matrix. Echo verbs build on the stylized-echo engine
//! (`_wrap`/`_scoped` in [`crate::support::doc_style`], `EchoArgs` from [`crate::support::args`]);
//! `code_highlight` colours via [`crate::support::color_theme`]. This module also owns `_styled_echo`,
//! the print-in-a-style helper the generated `recho` matrix ([`crate::categories::autogen_styles`])
//! forwards to. Add manual style commands here — `build.rs` regenerates the matrix separately and
//! never touches this file.

#[bashrs_macros::category(command = StyleCommand, prefix = "style_")]
mod commands {
    use crate::support::args::EchoArgs;
    use crate::support::doc_style::{_scoped, _wrap};
    use crate::support::{color_theme, input, streamgrep};
    use clap::Args;
    use std::path::Path;

    /// echo in bold red, to stderr
    #[unprefixed]
    pub fn errcho(args: EchoArgs) {
        eprintln!("{}", _scoped(&_wrap(["bo", "", "r"]), &args.words.join(" ")));
    }

    /// Syntax-highlight code — from an argument, a file, or stdin
    #[unprefixed]
    pub fn code_highlight(args: CodeHighlightArgs) {
        let code = match input::read_input(args.source.as_deref()) {
            Ok(code) => code,
            Err(err) => return eprintln!("code_highlight: {err}"),
        };
        let Some(ext) = _language(&args) else {
            return eprintln!("code_highlight: pass a language with --lang (e.g. --lang rs) unless the input is a file");
        };
        print!("{}", color_theme::highlight(&code, &ext));
    }

    /// Code to highlight (inline text, a file path, or omitted to read stdin), plus the
    /// language to highlight it as.
    #[derive(Args)]
    pub struct CodeHighlightArgs {
        /// Code text, or a path to a file — omit to read stdin.
        source: Option<String>,
        /// Language / file extension (e.g. rs, py, js). Inferred from the file when
        /// `source` is a path; required otherwise.
        #[arg(short, long)]
        lang: Option<String>,
    }

    /// Highlight every match of PATTERN in the input, keeping all lines (like `grep --color`)
    #[unprefixed]
    pub fn keyword_highlight(args: KeywordHighlightArgs) {
        let text = match input::read_input(args.source.as_deref()) {
            Ok(text) => text,
            Err(err) => return eprintln!("keyword_highlight: {err}"),
        };
        // Print every line, colouring only the matches — the grep crate's passthru mode does this
        // directly (no `pattern|$` trick, and no external `grep`).
        streamgrep::highlight(&args.pattern, &text);
    }

    /// The term to highlight, and where to read the text.
    #[derive(Args)]
    pub struct KeywordHighlightArgs {
        /// Regex whose matches are highlighted.
        pattern: String,
        /// Text to search: a file path, inline text, or omitted to read stdin.
        source: Option<String>,
    }

    /// The extension to highlight as: `--lang` if given, else a `source` file's extension.
    fn _language(args: &CodeHighlightArgs) -> Option<String> {
        if let Some(lang) = &args.lang {
            return Some(lang.clone());
        }
        let path = Path::new(args.source.as_deref()?);
        if path.is_file() {
            path.extension()?.to_str().map(str::to_owned)
        } else {
            None
        }
    }

    /// Print the words scoped in the style named by `criteria` (see [`crate::support::doc_style`]).
    pub(crate) fn _styled_echo(criteria: [&str; 3], args: &EchoArgs) {
        println!("{}", _scoped(&_wrap(criteria), &args.words.join(" ")));
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn language_prefers_the_flag_then_a_file_extension() {
            let flag_wins = CodeHighlightArgs { source: Some("x.py".into()), lang: Some("rs".into()) };
            assert_eq!(_language(&flag_wins).as_deref(), Some("rs"));
            let text_no_lang = CodeHighlightArgs { source: Some("let x = 5".into()), lang: None };
            assert_eq!(_language(&text_no_lang), None); // not a file and no --lang → unknown
        }
    }
}
