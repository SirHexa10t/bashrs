//! The program's one colour theme — every semantic colour decision in a single place, so code
//! highlighting ([`crate::support::color_theme`]) and Markdown docs ([`crate::support::doc_render`])
//! share one palette.
//!
//! Base colours are the named vocabulary in [`crate::support::generative_constants`] (the same
//! colours the `recho` command matrix exposes), resolved through [`crate::support::doc_style`];
//! this module assigns those to roles and adds the few shades the vocabulary doesn't carry —
//! grey, the high-intensity cyan/magenta, and the italic attribute. Those extras live here (not in
//! the vocabulary) precisely so they never spawn `<name>echo` commands.

use crate::support::doc_style::{escape, _header, _wrap};
use crate::support::generative_constants::COLORS;

// --- theme-only shades: deliberately NOT in the recho vocabulary ---
/// Muted grey for code comments (SGR bright-black).
const GREY: &str = "90";
/// High-intensity cyan/magenta (the bright SGR variants) — the doc render's inline code and italic.
const BRIGHT_CYAN: &str = "96";
const BRIGHT_MAGENTA: &str = "95";
/// The italic attribute — the vocabulary's weights are only bold/dark.
const ITALIC: &str = "3";

/// The SGR sub-code of a base vocabulary colour name (`"m"` → `"35"`); `""` for an unknown key,
/// which only a typo in this file could produce.
fn base(name: &str) -> &'static str {
    COLORS.iter().find(|(key, _, _)| *key == name).map_or("", |(_, sgr, _)| *sgr)
}

// --- code highlighting: synoptic token kind → colour ---

/// The ANSI style opening for a synoptic token `kind`, or `None` for kinds left uncoloured. The
/// colours are the shared vocabulary (magenta/green/cyan/…); only `comment` reaches for the
/// theme-only grey.
pub(crate) fn code_style(kind: &str) -> Option<String> {
    let colour = match kind {
        "keyword" | "macro" | "header" => base("m"), // magenta
        "string" | "character" => base("g"),         // green
        "comment" => GREY,
        "digit" | "number" | "reference" => base("c"), // cyan
        "boolean" | "struct" | "type" | "attribute" => base("y"), // yellow
        "function" | "namespace" | "tag" => base("b"), // blue
        "operator" => base("w"),                       // white
        _ => return None,
    };
    Some(escape(colour))
}

// --- Markdown docs: element → style opening (item text follows, then a RESET) ---

/// Heading — bold blue, via the shared [`_header`] look (also `gg`/`lll` titles). Takes the
/// already-inline-styled `inner` so a `**word**` within a title restyles in place (that's
/// `_header`'s `_scoped` mechanism re-asserting the heading colour after the nested span).
pub(crate) fn doc_heading(inner: &str) -> String {
    _header(inner)
}

/// **Bold** → bold yellow.
pub(crate) fn doc_bold() -> String {
    _wrap(["bo", "", "y"])
}

/// *Italic* → italic + bright magenta (no italic weight in the vocabulary, so the attribute is raw).
pub(crate) fn doc_italic() -> String {
    escape(&format!("{ITALIC};{BRIGHT_MAGENTA}"))
}

/// Inline code and indented code blocks → bright cyan.
pub(crate) fn doc_code() -> String {
    escape(BRIGHT_CYAN)
}

/// Blockquote → dim (the vocabulary has no grey colour, and dim reads as an aside).
pub(crate) fn doc_quote() -> String {
    _wrap(["da", "", ""])
}

/// A Markdown link's visible text → blue (plain, so it reads distinctly from a bold-blue heading).
pub(crate) fn doc_link_text() -> String {
    _wrap(["", "", "b"])
}

/// A link's URL (Markdown or bare) → bright cyan, underlined — the "this is a link" look.
pub(crate) fn doc_link_url() -> String {
    escape(&format!("4;{BRIGHT_CYAN}")) // 4 = underline
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::doc_style::RESET;

    #[test]
    fn base_resolves_vocabulary_colours_and_is_safe_on_typos() {
        assert_eq!(base("c"), "36"); // cyan is in the vocabulary
        assert_eq!(base("b"), "34"); // blue
        assert_eq!(base("zz"), ""); // unknown key → empty, never a panic
    }

    #[test]
    fn code_style_maps_known_kinds_and_skips_unknowns() {
        assert_eq!(code_style("string").unwrap(), escape("32")); // green, from the vocabulary
        assert_eq!(code_style("comment").unwrap(), escape("90")); // theme-only grey
        // kinds sharing a colour resolve identically (one source, not copy-pasted codes)
        assert_eq!(code_style("keyword").unwrap(), code_style("header").unwrap());
        assert!(code_style("no_such_kind").is_none());
    }

    #[test]
    fn doc_styles_are_the_bright_palette() {
        assert_eq!(doc_code(), escape("96")); // bright cyan
        assert_eq!(doc_italic(), escape("3;95")); // italic + bright magenta
        assert_eq!(doc_bold(), _wrap(["bo", "", "y"])); // bold yellow
        assert_eq!(doc_quote(), _wrap(["da", "", ""])); // dim
        assert_eq!(doc_link_text(), _wrap(["", "", "b"])); // blue
        assert_eq!(doc_link_url(), escape("4;96")); // underline + bright cyan
        let heading = doc_heading("Title");
        assert!(heading.contains("Title") && heading.ends_with(RESET), "{heading:?}");
    }
}
