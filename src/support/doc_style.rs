//! The styling engine — SGR-escape assembly, scope-aware wrapping, and the markdown element styles.
//! The one place raw ANSI escapes are built, shared by the `recho` command matrix
//! ([`crate::categories::autogen_styles`]), the hand-written style commands
//! ([`crate::categories::styles`], e.g. `errcho`), and anything needing a styled string: `lll`'s
//! header, `gg`'s section titles, `code_highlight`.
//!
//! A nested colour/style restores the enclosing one when it ends (rather than clearing to the
//! terminal default), and every span starts from a clean slate so styles don't compound — see
//! [`_scoped`]. Styles are assembled from [`crate::support::theme`]'s `Style`s: [`_wrap`] joins their
//! SGR sub-codes, and the element helpers (`heading`, `bold`, `code`, …) name the roles `dl -c`'s
//! renderer paints.

use crate::support::theme::{Basic, Md, Style, Underline, Weight};

/// Ends a styled span, restoring the terminal to its default.
pub(crate) const RESET: &str = "\x1b[0m";

/// The ANSI escape for the given SGR sub-codes — e.g. `"1;34"` → `"\x1b[1;34m"`. The single place a
/// raw escape is constructed, so nothing else has to write `\x1b[…m` by hand.
pub(crate) fn escape(sgr: &str) -> String {
    format!("\x1b[{sgr}m")
}

/// Assemble an ANSI escape from a set of `Style`s — read each one's [`Style::sgr`], skip the
/// empties (a `None`-style member), and join with `;`. e.g. `_wrap(&[&Weight::Bold, &Basic::Red])`
/// → `"\x1b[1;31m"`; `_wrap(&[&Weight::Bold])` → `"\x1b[1m"`.
pub(crate) fn _wrap(styles: &[&dyn Style]) -> String {
    let sgr = styles.iter().map(|s| s.sgr()).filter(|c| !c.is_empty()).collect::<Vec<_>>().join(";");
    escape(&sgr)
}

/// Style `text` in the shared bold-blue "header" style — `lll`'s column row and `gg`'s section
/// titles both use it, so the style is defined once and composed from theme styles like the rest.
pub(crate) fn _header(text: &str) -> String {
    _scoped(&_wrap(&[&Weight::Bold, &Basic::Blue]), text)
}

// --- markdown element styles (the palette `dl -c`'s renderer paints) ----------------------------

/// Heading — bold blue, via the shared [`_header`] look. Takes the already-inline-styled `inner`
/// so a `**word**` within a title restyles in place ([`_scoped`] re-asserts the heading colour
/// after each nested span).
pub(crate) fn heading(inner: &str) -> String {
    _header(inner)
}

/// **Bold** → bold yellow.
pub(crate) fn bold() -> String {
    _wrap(&[&Weight::Bold, &Basic::Yellow])
}

/// *Italic* → italic + bright magenta.
pub(crate) fn italic() -> String {
    _wrap(&[&Md::Italic, &Md::BrightMagenta])
}

/// Inline code and indented code blocks → orange (a red-ish tone the palette otherwise underuses).
pub(crate) fn code() -> String {
    _wrap(&[&Basic::Orange])
}

/// Blockquote → dim (dim reads as an aside; the palette has no grey colour).
pub(crate) fn quote() -> String {
    _wrap(&[&Weight::Dark])
}

/// A Markdown link's visible text → blue (plain, so it reads distinctly from a bold-blue heading).
pub(crate) fn link_text() -> String {
    _wrap(&[&Basic::Blue])
}

/// A link's URL (Markdown or bare) → bright cyan, underlined — the "this is a link" look.
pub(crate) fn link_url() -> String {
    _wrap(&[&Underline::Underlined, &Md::BrightCyan])
}

/// Style `text` as a broken/dangling link — bold red — so `lll` flags a Windows `.lnk` shortcut
/// (its name and target) against `ls`'s own colouring. Scoped, so it nests safely.
pub(crate) fn broken_link_text(text: &str) -> String {
    _scoped(&_wrap(&[&Weight::Bold, &Basic::Red]), text)
}

/// Style `text` as a "problematic" / bad-status marker — bold red — e.g. a `dl` heads-up that an
/// imported cookie store expired, or a video missing its thumbnail. Pairs with [`approved`]. A
/// distinct role from [`broken_link_text`], though it shares the colour. Scoped, so it nests safely.
pub(crate) fn problematic(text: &str) -> String {
    _scoped(&_wrap(&[&Weight::Bold, &Basic::Red]), text)
}

/// Style `text` as an "approved" / good-status marker — bold green — e.g. a video that already
/// carries its thumbnail. Pairs with [`problematic`]. Scoped, so it nests safely.
pub(crate) fn approved(text: &str) -> String {
    _scoped(&_wrap(&[&Weight::Bold, &Basic::Green]), text)
}

/// Style `text` with `codes`, keeping nested styles scoped instead of compounded.
///
/// Scopes are encoded in the stream itself, so nesting survives across the separate processes of
/// `recho "$(gecho …)"`: a span *opens* with `RESET + codes` (the reset is a clean start; the codes
/// set this style) and *closes* with a lone `RESET`. So we re-assert `codes` after each *closing*
/// reset already in `text` (a nested span that ended), and leave *opening* resets (a reset
/// immediately followed by an SGR) alone. Both are ordinary ANSI — harmless when not re-processed —
/// so the output renders the same in a terminal, a pipe, a file, or another `recho`.
pub(crate) fn _scoped(codes: &str, text: &str) -> String {
    let mut out = format!("{RESET}{codes}"); // open: clean start + this style
    let mut rest = text;
    while let Some(i) = rest.find(RESET) {
        let cut = i + RESET.len();
        out.push_str(&rest[..cut]); // text up to and including the reset
        rest = &rest[cut..];
        if !rest.starts_with("\x1b[") {
            out.push_str(codes); // a nested span closed here → restore my style
        }
    }
    out.push_str(rest);
    out.push_str(RESET); // close: lone reset
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ANSI shorthands for readable expectations.
    const RED: &str = "\x1b[1;31m";
    const GREEN: &str = "\x1b[1;32m";
    const BLUE: &str = "\x1b[1;34m";
    const ULINE: &str = "\x1b[1;4m";

    #[test]
    fn escape_builds_an_sgr_sequence() {
        assert_eq!(escape("1;34"), "\x1b[1;34m");
        assert_eq!(escape(""), "\x1b[m");
    }

    #[test]
    fn wrap_joins_style_sgrs_and_skips_empties() {
        assert_eq!(_wrap(&[&Weight::Bold, &Basic::Red]), "\x1b[1;31m"); // bold red
        assert_eq!(_wrap(&[&Weight::Bold]), "\x1b[1m"); // bold only (boecho)
        assert_eq!(
            _wrap(&[&Weight::Dark, &Underline::Underlined, &Basic::Red]),
            "\x1b[2;4;31m" // dark underlined red
        );
        // `None`-style empties drop out rather than leaving a stray `;`.
        assert_eq!(_wrap(&[&Basic::None, &Basic::Red]), "\x1b[31m");
    }

    #[test]
    fn header_is_bold_blue() {
        assert_eq!(_header("hi"), format!("{RESET}{BLUE}hi{RESET}"));
    }

    #[test]
    fn element_styles_paint_the_expected_colours() {
        assert_eq!(bold(), _wrap(&[&Weight::Bold, &Basic::Yellow]));
        assert_eq!(italic(), "\x1b[3;95m"); // italic + bright magenta
        assert_eq!(code(), "\x1b[38;5;208m"); // orange
        assert_eq!(quote(), "\x1b[2m"); // dim
        assert_eq!(link_text(), "\x1b[34m"); // blue
        assert_eq!(link_url(), "\x1b[4;96m"); // underline + bright cyan
        assert!(heading("Hi").contains("Hi") && heading("Hi").ends_with(RESET));
        assert!(broken_link_text("x").contains(&_wrap(&[&Weight::Bold, &Basic::Red]))); // bold red
        assert_eq!(problematic("x"), format!("{RESET}{RED}x{RESET}")); // bold red, scoped
        assert_eq!(approved("x"), format!("{RESET}{GREEN}x{RESET}")); // bold green, scoped
    }

    #[test]
    fn plain_text_is_wrapped_and_reset() {
        assert_eq!(_scoped(RED, "hi"), format!("{RESET}{RED}hi{RESET}"));
    }

    #[test]
    fn empty_text_is_just_the_style_and_reset() {
        assert_eq!(_scoped(RED, ""), format!("{RESET}{RED}{RESET}"));
    }

    #[test]
    fn a_closing_reset_restores_the_enclosing_style() {
        let out = _scoped(RED, &format!("red {} y", _scoped(GREEN, "g")));
        assert_eq!(out, format!("{RESET}{RED}red {RESET}{GREEN}g{RESET}{RED} y{RESET}"));
    }

    #[test]
    fn a_nested_span_starts_clean() {
        let out = _scoped(RED, &format!("r {}", _scoped(ULINE, "u")));
        assert!(out.contains(&format!("{RESET}{ULINE}u{RESET}")), "underline span not clean: {out}");
    }

    #[test]
    fn an_already_processed_reset_re_asserts_the_style() {
        let out = _scoped(RED, "a \x1b[0m b");
        assert_eq!(out, format!("{RESET}{RED}a {RESET}{RED} b{RESET}"));
    }

    #[test]
    fn deeply_nested_spans_unwind_to_each_enclosing_style() {
        let inner = _scoped(GREEN, &format!("b {} d", _scoped(BLUE, "c")));
        let out = _scoped(RED, &format!("a {inner} e"));
        assert_eq!(
            out,
            format!("{RESET}{RED}a {RESET}{GREEN}b {RESET}{BLUE}c{RESET}{GREEN} d{RESET}{RED} e{RESET}"),
        );
    }

    #[test]
    fn sibling_spans_each_restore_the_enclosing_style() {
        let out = _scoped(RED, &format!("{} mid {}", _scoped(GREEN, "g"), _scoped(BLUE, "b")));
        assert!(out.contains(&format!("{GREEN}g{RESET}")), "green sibling not scoped: {out}");
        assert!(out.contains(&format!("{BLUE}b{RESET}")), "blue sibling not scoped: {out}");
        assert!(out.contains(&format!("{RED} mid {RESET}")), "middle should be red: {out}");
    }
}
