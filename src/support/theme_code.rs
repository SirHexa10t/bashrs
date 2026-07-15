//! Syntax-highlighting a block of code with ANSI colour, via `synoptic`. Shared by the
//! `code_highlight` style command and by `bashrs_sourcefile` (which colours the shell it
//! prints). The kind→colour map ([`_code_colour`]) is the code-highlighting theme; its colours are
//! [`crate::support::theme`] atoms, and [`argname`] reuses it to style a lone flag name as code.
//! (Rendering an embedded Markdown *doc* for `dl -c` is a separate
//! concern — marker-stripping rather than highlighting — and lives in
//! [`crate::support::doc_render`], the hand-built sibling of this synoptic-backed module.)

use crate::support::doc_style::{escape, RESET};
use crate::support::theme::{Basic, Md, Style};

/// Colour `code` as language `ext` (a file extension synoptic knows — `rs`, `sh`, `py`, …),
/// returning the ANSI-coloured text with each line newline-terminated. An extension synoptic
/// doesn't recognise comes back uncoloured (still one newline per line), never an error.
pub(crate) fn highlight(code: &str, ext: &str) -> String {
    // synoptic hands back an empty (no-op) highlighter for unknown extensions rather than
    // `None`, so an unrecognised language just yields uncoloured text.
    let Some(mut hl) = synoptic::from_extension(ext, 4) else {
        return code.to_owned();
    };
    let lines: Vec<String> = code.lines().map(str::to_owned).collect();
    hl.run(&lines);
    let mut out = String::new();
    for (y, line) in lines.iter().enumerate() {
        for token in hl.line(y, line) {
            match token {
                synoptic::TokOpt::Some(text, kind) => out.push_str(&paint(&kind, &text)),
                synoptic::TokOpt::None(text) => out.push_str(&text),
            }
        }
        out.push('\n');
    }
    out
}

/// Wrap `text` in the colour this module assigns to a synoptic token `kind`; unknown kinds stay plain.
fn paint(kind: &str, text: &str) -> String {
    match _code_colour(kind) {
        Some(sgr) => format!("{}{text}{RESET}", escape(sgr)),
        None => text.to_owned(),
    }
}

/// The code-highlighting theme: a synoptic token `kind` → the SGR of its `theme` colour, or `None`
/// for kinds left uncoloured. Base colours come from the vocabulary; `comment` reaches for the
/// doc-only grey.
fn _code_colour(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "keyword" | "macro" | "header" => Basic::Magenta.sgr(),
        "string" | "character" => Basic::Green.sgr(),
        "comment" => Md::Grey.sgr(),
        "digit" | "number" | "reference" => Basic::Cyan.sgr(),
        "boolean" | "struct" | "type" | "attribute" => Basic::Yellow.sgr(),
        "function" | "namespace" | "tag" => Basic::Blue.sgr(),
        "operator" => Basic::White.sgr(),
        // Not a synoptic token — a standalone code term (a command-line flag name), styled green
        // by [`argname`] for non-highlighted contexts like the yt-dlp taglist.
        "argname" => Basic::Green.sgr(),
        _ => return None,
    })
}

/// Style `text` as an argument/flag name (a code term) → green. For contexts that aren't full
/// syntax highlighting but still want a flag to read as code, e.g. the yt-dlp `taglist`.
pub(crate) fn argname(text: &str) -> String {
    paint("argname", text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_colours_known_kinds_and_leaves_others_plain() {
        assert_eq!(paint("string", "hi"), "\x1b[32mhi\x1b[0m");
        assert_eq!(paint("comment", "// x"), "\x1b[90m// x\x1b[0m");
        assert_eq!(paint("no_such_kind", "raw"), "raw"); // graceful: printed uncoloured
    }

    #[test]
    fn argname_styles_a_flag_as_green_code() {
        assert_eq!(argname("--audio-format"), "\x1b[32m--audio-format\x1b[0m");
    }

    #[test]
    fn highlight_colours_known_languages_and_passes_unknown_through() {
        let rust = highlight("let x = 5;", "rs");
        assert!(rust.contains('\x1b'), "rust should be coloured: {rust:?}");
        assert!(rust.ends_with('\n'));
        // an unknown language has no rules → returned uncoloured, not an error
        assert_eq!(highlight("let x = 5;", "nosuchlang"), "let x = 5;\n");
    }
}
