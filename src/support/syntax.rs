//! Syntax-highlighting a block of code with ANSI colour, via `synoptic`. Shared by the
//! `code_highlight` style command and by `bashrs_sourcefile` (which colours the shell it
//! prints), so both go through one palette.

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

/// Wrap `text` in the ANSI colour for a synoptic token `kind`; unknown kinds stay plain.
fn paint(kind: &str, text: &str) -> String {
    match PALETTE.iter().find(|(k, _)| *k == kind) {
        Some((_, code)) => format!("\x1b[{code}m{text}\x1b[0m"),
        None => text.to_owned(),
    }
}

/// synoptic token kind → SGR colour code.
const PALETTE: &[(&str, &str)] = &[
    ("keyword", "35"),   // magenta
    ("macro", "35"),
    ("string", "32"),    // green
    ("character", "32"),
    ("comment", "90"),   // grey
    ("digit", "36"),     // cyan
    ("number", "36"),
    ("boolean", "33"),   // yellow
    ("function", "34"),  // blue
    ("struct", "33"),
    ("type", "33"),
    ("namespace", "34"),
    ("reference", "36"),
    ("attribute", "33"),
    ("tag", "34"),
    ("header", "35"),
    ("operator", "37"),  // white
];

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
    fn highlight_colours_known_languages_and_passes_unknown_through() {
        let rust = highlight("let x = 5;", "rs");
        assert!(rust.contains('\x1b'), "rust should be coloured: {rust:?}");
        assert!(rust.ends_with('\n'));
        // an unknown language has no rules → returned uncoloured, not an error
        assert_eq!(highlight("let x = 5;", "nosuchlang"), "let x = 5;\n");
    }
}
