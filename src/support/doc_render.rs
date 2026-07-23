//! Rendering an embedded Markdown doc to ANSI-coloured terminal text via `minimad`'s line-based
//! parser and our own emit through [`crate::support::doc_style`]'s named style vocabulary. The
//! hand-built, *marker-stripping* counterpart to [`crate::support::theme_code`]'s synoptic
//! highlighting: `#`, `**`, `` ` `` and friends are consumed, not shown, and every element is
//! coloured via [`crate::support::doc_style`]'s element helpers — this module owns the *structure*
//! (what a heading, bullet, quote or code line becomes), `doc_style` owns the colours. Inline
//! marks are honoured even inside a heading — a `**word**` in a title restyles in place, then the
//! title colour resumes.
//!
//! Line-based on purpose — exactly one output line per source line. Pre-drawn box tables and
//! other fixed-layout blocks pass through untouched (minimad parses them as plain text, since
//! box-drawing chars aren't Markdown). The one reflowed block is a markdown *pipe* table: its
//! rows are collected and column-aligned through `table_formatter` (the `table` command's engine,
//! splitting/joining on " | "), still one output line per source row.
//!
//! minimad is deliberately simple and line-oriented, which shapes two things worth knowing when
//! authoring the `.md`: nested bullets go **one** level deep — `  -` (two spaces) is a sub-bullet,
//! but four-plus spaces trip Markdown's indented-code-block rule and render as a code line, not a
//! deeper bullet — and links: minimad parses no links, so `_linkify` styles both Markdown
//! `[text](url)` (text blue, URL bright-cyan underlined, markers dropped) and bare
//! `http(s)://`/`www.` URLs, in place.

use crate::support::doc_style::{self, RESET};

/// Nested-list glyphs by depth; index saturates at the deepest we style (minimad only reaches the
/// first two anyway — see the module note on the four-space rule).
const BULLETS: [&str; 3] = ["• ", "◦ ", "▪ "];

/// Render a Markdown `doc` to ANSI-coloured text for terminal display (`dl -c`'s site listing).
/// Markers are stripped; headings, emphasis, inline code, list items and blockquotes are coloured
/// via `doc_style`'s element helpers; every other line (paragraphs, pre-drawn tables) passes
/// through with only its inline spans styled. One output line per source line.
pub(crate) fn render_doc(doc: &str) -> String {
    use minimad::{CompositeStyle, Line};
    let parsed = minimad::parse_text(doc, minimad::Options::default());
    let lines = &parsed.lines;
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        match &lines[i] {
            // A markdown pipe-table block: gather the contiguous run and align it as a unit
            // through `table_formatter` (the `table` command's engine), splitting/joining on the
            // " | " the rows are already written in — one column-width pass, in one place.
            Line::TableRow(_) | Line::TableRule(_) => {
                let start = i;
                while i < lines.len() && matches!(lines[i], Line::TableRow(_) | Line::TableRule(_)) {
                    i += 1;
                }
                _emit_table(&mut out, &lines[start..i]);
                continue;
            }
            Line::Normal(composite) => match composite.style {
                // Heading — bold blue via `_header` (the `gg`/`lll` title look), but inline marks
                // are kept and restyled *within* the title: `_header` wraps through
                // `doc_style::_scoped`, which re-asserts the heading colour after each nested span
                // closes — so `**word**` in a heading shows bold-yellow, then the blue resumes.
                CompositeStyle::Header(_) => {
                    let inner: String = composite.compounds.iter().map(_inline).collect();
                    out.push_str(&doc_style::heading(&inner));
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
                    out.push_str(&format!("{}{}{RESET}", doc_style::quote(), _plain(composite)));
                }
                // Indented (four-space) code block → cyan, indent restored (minimad strips it).
                // Our docs use this only for the legend block; a line here is literal by intent.
                CompositeStyle::Code => {
                    out.push_str(&format!("    {}{}{RESET}", doc_style::code(), _plain(composite)));
                }
                // Paragraphs and anything else (incl. pre-drawn box-table rows): only inline spans
                // are styled, so fixed-layout text survives verbatim.
                _ => _emit_spans(&mut out, composite),
            },
            // HorizontalRule / CodeFence: our docs use neither — emit the bare newline below.
            _ => {}
        }
        out.push('\n');
        i += 1;
    }
    out
}

/// Render a contiguous markdown pipe-table block, aligned through `table_formatter`. Each row is
/// rebuilt as a `| cell | … |` line with its inline spans styled (bold/links/etc.), then the whole
/// block is handed to `format_table` splitting and joining on " | " — the shape the rows are
/// already in — so column widths are found once, by the shared engine, measured on *display*
/// width (ANSI- and emoji-aware). `emit_frame` keeps the outer `| … |` border; the `|---|` rule
/// aligns like any other row. (Alignment, not wrapping: a row wider than the terminal overflows.)
fn _emit_table(out: &mut String, block: &[minimad::Line]) {
    use minimad::Line;
    let lines: Vec<String> = block
        .iter()
        .map(|line| match line {
            Line::TableRow(row) => _table_row(&row.cells),
            Line::TableRule(rule) => _table_rule(rule.cells.len()),
            _ => String::new(),
        })
        .collect();
    let opts = table_formatter::FormatOptions {
        divide_by: " | ".to_string(),
        join_with: " | ".to_string(),
        emit_frame: true,
        ..Default::default()
    };
    for aligned in table_formatter::format_table(&lines, &opts).unwrap_or(lines) {
        out.push_str(&aligned);
        out.push('\n');
    }
}

/// One table row as `| cell | cell | … |`, each cell's inline spans styled.
fn _table_row(cells: &[minimad::Composite]) -> String {
    let mut out = String::from("|");
    for cell in cells {
        out.push(' ');
        _emit_spans(&mut out, cell);
        out.push_str(" |");
    }
    out
}

/// A header rule as `| --- | --- | … |`, one per column — aligned like any other row.
fn _table_rule(columns: usize) -> String {
    let mut out = String::new();
    for _ in 0..columns {
        out.push_str("| --- ");
    }
    if columns > 0 {
        out.push('|');
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

/// One inline span → ANSI, taking its colour from [`doc_style`]'s element helpers: code, then bold,
/// then italic (mutually exclusive here). Unmarked text goes through [`_linkify`], which is where
/// Markdown links get their colour (minimad doesn't parse them).
fn _inline(compound: &minimad::Compound) -> String {
    let s = compound.as_str();
    let style = if compound.code {
        doc_style::code()
    } else if compound.bold {
        doc_style::bold()
    } else if compound.italic {
        doc_style::italic()
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
                        out.push_str(&doc_style::link_text());
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
    out.push_str(&doc_style::link_url());
    out.push_str(url);
    out.push_str(RESET);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The heading style (bold blue) as a raw escape, for asserting headings carry it.
    const HEADING: &str = "\x1b[1;34m";

    #[test]
    fn headings_lose_their_markers_and_are_coloured() {
        let out = render_doc("# Legend");
        assert!(!out.contains('#'), "the `#` marker must be stripped: {out:?}");
        assert!(out.contains("Legend"), "heading text kept: {out:?}");
        assert!(out.contains(HEADING), "heading is bold blue: {out:?}");
    }

    #[test]
    fn a_bold_word_inside_a_heading_restyles_in_place() {
        // The heading is bold-blue, but a `**word**` within it shows bold-yellow, and the heading
        // colour must resume afterward (the mid-style restyle via `_scoped`).
        let out = render_doc("# risks run the **other** direction");
        let head = HEADING; // the heading's bold blue (shared _header style)
        let bold = doc_style::bold(); // the doc bold style
        assert!(out.contains(&format!("{bold}other{RESET}")), "the bold word is re-marked: {out:?}");
        assert!(out.contains(&format!("{RESET}{head} direction")), "heading colour resumes after: {out:?}");
    }

    #[test]
    fn inline_emphasis_markers_are_stripped_and_coloured() {
        let out = render_doc("plain **bold** and `code` and *italic* here");
        assert!(!out.contains('*') && !out.contains('`'), "markers stripped: {out:?}");
        assert!(out.contains(&format!("{}bold{RESET}", doc_style::bold())), "bold uses the theme: {out:?}");
        assert!(out.contains(&format!("{}code{RESET}", doc_style::code())), "code uses the theme: {out:?}");
        assert!(out.contains(&format!("{}italic{RESET}", doc_style::italic())), "italic uses the theme: {out:?}");
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
        assert!(out.contains(&format!("{}quoted{RESET}", doc_style::quote())), "dim quote: {out:?}");
    }

    #[test]
    fn links_are_recoloured_and_lose_their_markers() {
        let out = render_doc("see [ImprovedTube #623](https://example.com/x) here");
        assert!(!out.contains("]("), "the `](` link joiner is gone: {out:?}");
        assert!(out.contains(&format!("{}ImprovedTube #623{RESET}", doc_style::link_text())), "text blue: {out:?}");
        assert!(out.contains(&format!("{}https://example.com/x{RESET}", doc_style::link_url())), "url cyan: {out:?}");
        // a bracket that isn't a real link is left alone
        assert!(render_doc("array[0] = 1").contains("array[0] = 1"), "non-link bracket preserved");
    }

    #[test]
    fn bare_urls_are_styled_in_place() {
        let out = render_doc("see https://example.com/x and www.foo.org, ok");
        assert!(out.contains(&format!("{}https://example.com/x{RESET}", doc_style::link_url())), "http url: {out:?}");
        // trailing comma is prose, trimmed off the styled URL
        assert!(out.contains(&format!("{}www.foo.org{RESET}", doc_style::link_url())), "www url: {out:?}");
        // a bare domain with no scheme/www is left plain
        assert!(!render_doc("visit example.com now").contains(&doc_style::link_url()), "bare domain untouched");
    }

    #[test]
    fn markdown_pipe_tables_align_via_table_formatter() {
        // minimad parses `| … |` as TableRow/TableRule (not Normal); these were once dropped
        // entirely. Now the block goes through table_formatter (split/join on " | "): content
        // survives, `**` is styled away, every row shares one display width (the alignment), and
        // the frame is kept.
        let table = "| Voice | Hz |\n|---|---|\n| **whistle register** | 3322 |\n";
        let out = render_doc(table);
        let rows: Vec<&str> = out.lines().collect();
        assert_eq!(rows.len(), 3, "one output line per source line (header, rule, data)");
        assert!(out.contains("Voice") && out.contains("whistle register") && out.contains("3322"));
        assert!(!out.contains("**"), "the bold markers are consumed, not shown: {out:?}");
        let w = |s: &str| console::measure_text_width(s);
        assert_eq!(w(rows[0]), w(rows[2]), "header and data align to one width: {out:?}");
        assert_eq!(w(rows[0]), w(rows[1]), "the rule shares that width");
        for row in &rows {
            assert!(row.starts_with('|') && row.ends_with('|'), "framed row: {row:?}");
        }
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
