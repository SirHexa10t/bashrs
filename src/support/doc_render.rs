//! Rendering an embedded Markdown doc to ANSI-coloured terminal text via `minimad`'s line-based
//! parser and our own emit through [`crate::support::doc_style`]'s named style vocabulary. The
//! hand-built, *marker-stripping* counterpart to [`crate::support::color_theme`]'s synoptic
//! highlighting: `#`, `**`, `` ` `` and friends are consumed, not shown, and every element is
//! coloured through the shared [`crate::support::theme`] — this module owns the *structure* (what
//! a heading, bullet, quote or code line becomes), the theme owns the colours. Inline
//! marks are honoured even inside a heading — a `**word**` in a title restyles in place, then the
//! title colour resumes.
//!
//! Line-based on purpose — exactly one output line per source line — so pre-drawn box tables and
//! other fixed-layout blocks pass through without reflow (minimad parses them as plain text, since
//! box-drawing chars aren't Markdown), and the "line count preserved" contract holds.
//!
//! minimad is deliberately simple and line-oriented, which shapes two things worth knowing when
//! authoring the `.md`: nested bullets go **one** level deep — `  -` (two spaces) is a sub-bullet,
//! but four-plus spaces trip Markdown's indented-code-block rule and render as a code line, not a
//! deeper bullet — and links: minimad parses no links, so `_linkify` styles both Markdown
//! `[text](url)` (text blue, URL bright-cyan underlined, markers dropped) and bare
//! `http(s)://`/`www.` URLs, in place.

use crate::support::{doc_style::RESET, theme};

/// Nested-list glyphs by depth; index saturates at the deepest we style (minimad only reaches the
/// first two anyway — see the module note on the four-space rule).
const BULLETS: [&str; 3] = ["• ", "◦ ", "▪ "];

/// Render a Markdown `doc` to ANSI-coloured text for terminal display (`dl -c`'s site listing).
/// Markers are stripped; headings, emphasis, inline code, list items and blockquotes are coloured
/// via the shared theme; every other line (paragraphs, pre-drawn tables) passes
/// through with only its inline spans styled. One output line per source line.
pub(crate) fn render_doc(doc: &str) -> String {
    use minimad::{CompositeStyle, Line};
    let mut out = String::new();
    for line in &minimad::parse_text(doc, minimad::Options::default()).lines {
        if let Line::Normal(composite) = line {
            match composite.style {
                // Heading — bold blue via `_header` (the `gg`/`lll` title look), but inline marks
                // are kept and restyled *within* the title: `_header` wraps through
                // `doc_style::_scoped`, which re-asserts the heading colour after each nested span
                // closes — so `**word**` in a heading shows bold-yellow, then the blue resumes.
                CompositeStyle::Header(_) => {
                    let inner: String = composite.compounds.iter().map(_inline).collect();
                    out.push_str(&theme::doc_heading(&inner));
                }
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
                    out.push_str(&format!("{}{}{RESET}", theme::doc_quote(), _plain(composite)));
                }
                // Indented (four-space) code block → cyan, indent restored (minimad strips it).
                // Our docs use this only for the legend block; a line here is literal by intent.
                CompositeStyle::Code => {
                    out.push_str(&format!("    {}{}{RESET}", theme::doc_code(), _plain(composite)));
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

/// One inline span → ANSI, taking its colour from the shared [`theme`]: code, then bold, then
/// italic (mutually exclusive here). Unmarked text goes through [`_linkify`], which is where
/// Markdown links get their colour (minimad doesn't parse them).
fn _inline(compound: &minimad::Compound) -> String {
    let s = compound.as_str();
    let style = if compound.code {
        theme::doc_code()
    } else if compound.bold {
        theme::doc_bold()
    } else if compound.italic {
        theme::doc_italic()
    } else {
        return _linkify(s);
    };
    format!("{style}{s}{RESET}")
}

/// Rewrite links in plain text so they read as links, markers dropped (colour marks the role): a
/// Markdown `[text](url)` → its text in the link-text colour, a space, then the URL styled; a
/// *bare* `http://`/`https://`/`www.` URL → styled in place. minimad parses neither. A `[` that
/// isn't a real link, and everything else, pass through unchanged.
fn _linkify(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    loop {
        // Handle whichever comes first: a Markdown-link `[` or a bare-URL scheme.
        let (at, is_md) = match (rest.find('['), _url_start(rest)) {
            (Some(m), Some(b)) => (m.min(b), m <= b),
            (Some(m), None) => (m, true),
            (None, Some(b)) => (b, false),
            (None, None) => break,
        };
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        if is_md {
            let after = &rest[1..]; // past the `[`
            // A link is `[text](url)`: a `]` immediately followed by `(`, then a closing `)`.
            if let Some(close) = after.find(']') {
                if let Some(url_on) = after[close + 1..].strip_prefix('(') {
                    if let Some(end) = url_on.find(')') {
                        out.push_str(&theme::doc_link_text());
                        out.push_str(&after[..close]); // link text
                        out.push_str(RESET);
                        out.push(' ');
                        _push_url(&mut out, &url_on[..end]); // URL
                        rest = &url_on[end + 1..];
                        continue;
                    }
                }
            }
            out.push('['); // not a real `[text](url)` — keep the bracket, resume after it
            rest = after;
        } else {
            let end = _url_len(rest); // rest starts at the scheme
            _push_url(&mut out, &rest[..end]);
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    out
}

/// Byte offset of the earliest bare-URL scheme in `s`, if any.
fn _url_start(s: &str) -> Option<usize> {
    ["https://", "http://", "www."].iter().filter_map(|p| s.find(p)).min()
}

/// Length of the bare URL at the start of `s`: up to the next whitespace, minus trailing sentence
/// punctuation / closing brackets (prose, not part of the address).
fn _url_len(s: &str) -> usize {
    let mut end = s.find(char::is_whitespace).unwrap_or(s.len());
    while end > 0 && b".,;:!?)]}'\"".contains(&s.as_bytes()[end - 1]) {
        end -= 1;
    }
    end
}

/// Append `url` in the theme's link-URL style (bright cyan, underlined), then a reset.
fn _push_url(out: &mut String, url: &str) {
    out.push_str(&theme::doc_link_url());
    out.push_str(url);
    out.push_str(RESET);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SGR a `_wrap` criterion resolves to, for asserting a span carries a given colour.
    fn sgr(criteria: [&str; 3]) -> String {
        crate::support::doc_style::_wrap(criteria)
    }

    #[test]
    fn headings_lose_their_markers_and_are_coloured() {
        let out = render_doc("# Legend");
        assert!(!out.contains('#'), "the `#` marker must be stripped: {out:?}");
        assert!(out.contains("Legend"), "heading text kept: {out:?}");
        assert!(out.contains(&sgr(["bo", "", "b"])), "heading is bold blue: {out:?}");
    }

    #[test]
    fn a_bold_word_inside_a_heading_restyles_in_place() {
        // The heading is bold-blue, but a `**word**` within it shows bold-yellow, and the heading
        // colour must resume afterward (the mid-style restyle via `_scoped`).
        let out = render_doc("# risks run the **other** direction");
        let head = sgr(["bo", "", "b"]); // the heading's bold blue (shared _header style)
        let bold = theme::doc_bold(); // the theme's bold
        assert!(out.contains(&format!("{bold}other{RESET}")), "the bold word is re-marked: {out:?}");
        assert!(out.contains(&format!("{RESET}{head} direction")), "heading colour resumes after: {out:?}");
    }

    #[test]
    fn inline_emphasis_markers_are_stripped_and_coloured() {
        let out = render_doc("plain **bold** and `code` and *italic* here");
        assert!(!out.contains('*') && !out.contains('`'), "markers stripped: {out:?}");
        assert!(out.contains(&format!("{}bold{RESET}", theme::doc_bold())), "bold uses the theme: {out:?}");
        assert!(out.contains(&format!("{}code{RESET}", theme::doc_code())), "code uses the theme: {out:?}");
        assert!(out.contains(&format!("{}italic{RESET}", theme::doc_italic())), "italic uses the theme: {out:?}");
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
        assert!(out.contains(&format!("{}quoted{RESET}", theme::doc_quote())), "dim quote: {out:?}");
    }

    #[test]
    fn links_are_recoloured_and_lose_their_markers() {
        let out = render_doc("see [ImprovedTube #623](https://example.com/x) here");
        assert!(!out.contains("]("), "the `](` link joiner is gone: {out:?}");
        assert!(out.contains(&format!("{}ImprovedTube #623{RESET}", theme::doc_link_text())), "text blue: {out:?}");
        assert!(out.contains(&format!("{}https://example.com/x{RESET}", theme::doc_link_url())), "url cyan: {out:?}");
        // a bracket that isn't a real link is left alone
        assert!(render_doc("array[0] = 1").contains("array[0] = 1"), "non-link bracket preserved");
    }

    #[test]
    fn bare_urls_are_styled_in_place() {
        let out = render_doc("see https://example.com/x and www.foo.org, ok");
        assert!(out.contains(&format!("{}https://example.com/x{RESET}", theme::doc_link_url())), "http url: {out:?}");
        // trailing comma is prose, trimmed off the styled URL
        assert!(out.contains(&format!("{}www.foo.org{RESET}", theme::doc_link_url())), "www url: {out:?}");
        // a bare domain with no scheme/www is left plain
        assert!(!render_doc("visit example.com now").contains(&theme::doc_link_url()), "bare domain untouched");
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
