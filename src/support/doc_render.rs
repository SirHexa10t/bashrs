//! Rendering an embedded Markdown doc to ANSI-coloured terminal text via `minimad`'s line-based
//! parser and our own emit through [`crate::support::doc_style`]'s named style vocabulary. The
//! hand-built, *marker-stripping* counterpart to [`crate::support::color_theme`]'s synoptic
//! highlighting: `#`, `**`, `` ` `` and friends are consumed, not shown, and every element is
//! coloured through the same vocabulary the rest of the crate uses (`_wrap`), reproducing the
//! surveyed termimad look: bold-blue headings, yellow bold, magenta italic, cyan code.
//!
//! Line-based on purpose — exactly one output line per source line — so pre-drawn box tables and
//! other fixed-layout blocks pass through without reflow (minimad parses them as plain text, since
//! box-drawing chars aren't Markdown), and the "line count preserved" contract holds.
//!
//! minimad is deliberately simple and line-oriented, which shapes two things worth knowing when
//! authoring the `.md`: nested bullets go **one** level deep — `  -` (two spaces) is a sub-bullet,
//! but four-plus spaces trip Markdown's indented-code-block rule and render as a code line, not a
//! deeper bullet — and a link stays literal `[text](url)` (minimad doesn't parse link spans).

use crate::support::doc_style::{escape, RESET, _header, _wrap};

/// Nested-list glyphs by depth; index saturates at the deepest we style (minimad only reaches the
/// first two anyway — see the module note on the four-space rule).
const BULLETS: [&str; 3] = ["• ", "◦ ", "▪ "];

/// Render a Markdown `doc` to ANSI-coloured text for terminal display (`dl -c`'s site listing).
/// Markers are stripped; headings, emphasis, inline code, list items and blockquotes are coloured
/// via the project's style vocabulary; every other line (paragraphs, pre-drawn tables) passes
/// through with only its inline spans styled. One output line per source line.
pub(crate) fn render_doc(doc: &str) -> String {
    use minimad::{CompositeStyle, Line};
    let mut out = String::new();
    for line in &minimad::parse_text(doc, minimad::Options::default()).lines {
        if let Line::Normal(composite) = line {
            match composite.style {
                // Heading — styled whole (bold blue), so inner marks are dropped: reuse `_header`,
                // the same look as `gg`/`lll` section titles.
                CompositeStyle::Header(_) => out.push_str(&_header(&_plain(composite))),
                // Bullet: `depth` is the leading-space count, two per level.
                CompositeStyle::ListItem(depth) => {
                    let level = (depth / 2) as usize;
                    out.push_str(&"  ".repeat(level));
                    out.push_str(BULLETS[level.min(BULLETS.len() - 1)]);
                    _emit_spans(&mut out, composite);
                }
                // Numbered item (e.g. the Sources): keep the number — dropping it would renumber
                // the list to nothing.
                CompositeStyle::OrderedListItem { index, .. } => {
                    out.push_str(&format!("{index}. "));
                    _emit_spans(&mut out, composite);
                }
                // Blockquote → dim (the vocabulary has no grey).
                CompositeStyle::Quote => {
                    out.push_str(&format!("{}{}{RESET}", _wrap(["da", "", ""]), _plain(composite)));
                }
                // Indented (four-space) code block → cyan, indent restored (minimad strips it).
                // Our docs use this only for the legend block; a line here is literal by intent.
                CompositeStyle::Code => {
                    out.push_str(&format!("    {}{}{RESET}", _wrap(["", "", "c"]), _plain(composite)));
                }
                // Paragraphs and anything else (incl. pre-drawn box-table rows): only inline spans
                // are styled, so fixed-layout text survives verbatim.
                _ => _emit_spans(&mut out, composite),
            }
        }
        out.push('\n');
    }
    out
}

/// The composite's text with no styling — for elements coloured as a whole (headings, quotes).
fn _plain(composite: &minimad::Composite) -> String {
    composite.compounds.iter().map(|c| c.as_str()).collect()
}

/// Append each inline span of `composite`, styled.
fn _emit_spans(out: &mut String, composite: &minimad::Composite) {
    for compound in &composite.compounds {
        out.push_str(&_inline(compound));
    }
}

/// One inline span → ANSI, matching the surveyed termimad palette: inline code cyan, **bold** in
/// bold yellow, *italic* in magenta (keeping the italic slant — the style vocabulary has no italic
/// weight, so a raw SGR pairs slant `3` with magenta `35`), everything else plain.
fn _inline(compound: &minimad::Compound) -> String {
    let s = compound.as_str();
    if compound.code {
        format!("{}{s}{RESET}", _wrap(["", "", "c"])) // cyan
    } else if compound.bold {
        format!("{}{s}{RESET}", _wrap(["bo", "", "y"])) // bold yellow
    } else if compound.italic {
        format!("{}{s}{RESET}", escape("3;35")) // italic + magenta
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SGR a `_wrap` criterion resolves to, for asserting a span carries a given colour.
    fn sgr(criteria: [&str; 3]) -> String {
        _wrap(criteria)
    }

    #[test]
    fn headings_lose_their_markers_and_are_coloured() {
        let out = render_doc("# Legend");
        assert!(!out.contains('#'), "the `#` marker must be stripped: {out:?}");
        assert!(out.contains("Legend"), "heading text kept: {out:?}");
        assert!(out.contains(&sgr(["bo", "", "b"])), "heading is bold blue: {out:?}");
    }

    #[test]
    fn inline_emphasis_markers_are_stripped_and_coloured() {
        let out = render_doc("plain **bold** and `code` and *italic* here");
        assert!(!out.contains('*') && !out.contains('`'), "markers stripped: {out:?}");
        assert!(out.contains(&format!("{}bold{RESET}", sgr(["bo", "", "y"]))), "bold is bold-yellow: {out:?}");
        assert!(out.contains(&format!("{}code{RESET}", sgr(["", "", "c"]))), "code is cyan: {out:?}");
        assert!(out.contains(&format!("{}italic{RESET}", escape("3;35"))), "italic is magenta: {out:?}");
    }

    #[test]
    fn bullets_become_glyphs_and_nest_one_level() {
        let out = render_doc("- top\n  - nested");
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].contains("• ") && lines[0].contains("top"), "top bullet: {out:?}");
        assert!(lines[1].starts_with("  ") && lines[1].contains("◦ "), "nested bullet indented: {out:?}");
        assert!(!out.contains("- "), "raw `- ` markers replaced by glyphs: {out:?}");
    }

    #[test]
    fn ordered_items_keep_their_numbers() {
        let out = render_doc("1. first\n2. second");
        assert!(out.contains("1. ") && out.contains("2. "), "numbering preserved: {out:?}");
        assert!(out.contains("first") && out.contains("second"));
    }

    #[test]
    fn blockquotes_are_dimmed_without_the_marker() {
        let out = render_doc("> quoted");
        assert!(!out.contains('>'), "quote marker stripped: {out:?}");
        assert!(out.contains(&format!("{}quoted{RESET}", sgr(["da", "", ""]))), "dim quote: {out:?}");
    }

    #[test]
    fn predrawn_table_rows_pass_through_verbatim() {
        // Box-drawing chars aren't Markdown, so a table row must survive character-for-character.
        let row = "│ Vimeo │ 11 │ incl. On Demand │";
        let out = render_doc(row);
        assert_eq!(out, format!("{row}\n"), "table row must be verbatim: {out:?}");
    }

    #[test]
    fn one_output_line_per_source_line() {
        let doc = "# H\n\npara\n- a\n  - b\n1. x\n│ t │\n\n> q";
        assert_eq!(
            render_doc(doc).lines().count(),
            doc.lines().count(),
            "line count must be preserved (no reflow)"
        );
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert_eq!(render_doc(""), "");
    }
}
