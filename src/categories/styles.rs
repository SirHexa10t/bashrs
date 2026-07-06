//! Hand-written style commands — the `StyleCommand` category: one-off styling verbs (like
//! `errcho`) and a `synoptic`-backed `code_highlight`, none part of the generated `recho`
//! matrix. Echo verbs build on the stylized-echo engine (`_wrap`/`_scoped`, `EchoArgs`) in
//! [`crate::categories::autogen_styles`]; `code_highlight` reads via
//! [`crate::support::input`] and colours via [`crate::support::syntax`]. Add manual style
//! commands here — `build.rs` regenerates the matrix separately and never touches this file.

#[bashrs_macros::category(command = StyleCommand, prefix = "style_")]
mod commands {
    use crate::categories::autogen_styles::{EchoArgs, _scoped, _wrap};
    use crate::support::{input, syntax};
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
        print!("{}", syntax::highlight(&code, &ext));
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
