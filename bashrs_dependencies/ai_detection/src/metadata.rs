//! Finding AI provenance markers in file *metadata* — half of this crate's engine, beside
//! [`crate::hidden`]'s character scan.
//!
//! Two halves, deliberately separable:
//!
//! - **Extraction** ([`extract`]): pull `(field, value)` string pairs out of the places each
//!   format keeps metadata — Markdown frontmatter, HTML `<meta>` tags, SVG `<metadata>`
//!   blocks, PNG text chunks, JPEG APP segments. No format library: each is a bounded walk of
//!   a documented layout, and the payloads wanted are exactly the textual ones.
//! - **Matching** ([`marker_in`]): the vendor names and the boundary-guarded `ai` tokens.
//!   Kept apart from extraction, since metadata is inventoried in full by default whether or
//!   not anything matched.
//!
//! Extraction never decodes media. A JPEG's pixels and a PNG's IDAT stay unread; only the
//! metadata containers are opened, so "no markers found" is a statement about the file's
//! labels, not its content.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

/// One metadata string worth reporting.
#[derive(Debug, PartialEq, Eq)]
pub struct Finding {
    /// Where it sat — `frontmatter "generator"`, `PNG tEXt "Software"`, `XMP CreatorTool`.
    pub field: String,
    pub value: String,
    pub why: Why,
}

/// The reason a [`Finding`] made the report.
#[derive(Debug, PartialEq, Eq)]
pub enum Why {
    /// A marker matched inside the value — which one, as matched.
    Marker(String),
    /// The field itself carries provenance (`Software`, `generator`, …): its value is
    /// reported whatever it says, since "what wrote this file" is the question being asked.
    ProvenanceField,
    /// Nothing matched; metadata is listed in full by default and this string is part of
    /// that inventory.
    Everything,
}

/// Vendor and product names matched case-insensitively as substrings — distinctive enough
/// that appearing anywhere in metadata is worth reporting. Grows like `LEAN_SKIPS`: one name
/// at a time, each earning its place.
pub const VENDOR_MARKERS: &[&str] = &[
    "claude",
    "anthropic",
    "openai",
    "chatgpt",
    "gemini",
    "grok",
    "midjourney",
    "dall-e",
    "dalle",
    "copilot",
    "stable diffusion",
    "synthid",
    "c2pa",
    "firefly",
];

/// Fields whose value is reported even when no marker matches: they exist to say what wrote
/// the file, so their content *is* the answer. Compared case-insensitively against the bare
/// key (`generator`), not the display name extraction builds around it.
pub const PROVENANCE_FIELDS: &[&str] = &[
    "generator",
    "software",
    "creatortool",
    "created_with",
    "created-with",
    "producer",
    // Stable Diffusion writes its whole prompt into a PNG tEXt chunk with this keyword.
    "parameters",
];

/// The marker that matches inside `text`, if any — returned as matched, for display.
///
/// Vendors are substrings. `ai` and `a.i.` are boundary-guarded instead: unguarded, `ai` is
/// inside *said, maintain, email, available, détail* and would flag nearly every description
/// field. Leading boundaries are line-start, space, `-` and `—`; trailing boundaries are those
/// plus `.`, for the sentence-final `made with AI.` — trailing only, deliberately asymmetric:
/// a period *before* `ai` is how domains spell (`mistral.ai`, `x.ai`), and flagging every
/// `.ai` mention would be noise. So `AI editor`, `an AI`, `AI-generated`, `made —AI— here`
/// and `Created with AI.` match, while `maintain` and `OpenAIKit` (caught by its vendor name
/// anyway) do not.
pub fn marker_in(text: &str) -> Option<String> {
    let lowered = text.to_lowercase();
    for vendor in VENDOR_MARKERS {
        if lowered.contains(vendor) {
            return Some((*vendor).to_string());
        }
    }
    static AI_TOKEN: OnceLock<Regex> = OnceLock::new();
    let ai = AI_TOKEN.get_or_init(|| {
        Regex::new(r"(?im)(?:^|[ \-—])(ai|a\.i\.)(?:$|[ \-—.])").expect("a fixed, tested pattern")
    });
    ai.captures(text).map(|found| found[1].to_string())
}

/// Whether this field name is one whose value gets reported regardless of content.
pub fn is_provenance_field(key: &str) -> bool {
    PROVENANCE_FIELDS.iter().any(|field| key.eq_ignore_ascii_case(field))
}

/// Classify one extracted pair into a [`Finding`], or `None` when it earns no place in the
/// report. `key` is the bare field name for the provenance check; `field` the display name.
fn classify(field: String, key: &str, value: String, everything: bool) -> Option<Finding> {
    let why = if let Some(marker) = marker_in(&value) {
        Why::Marker(marker)
    } else if is_provenance_field(key) {
        Why::ProvenanceField
    } else if everything {
        Why::Everything
    } else {
        return None;
    };
    Some(Finding { field, value, why })
}

/// Whether [`extract`] has an extractor for this path — for a caller deciding whether the
/// file's bytes are worth reading at all.
pub fn handles(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("md" | "markdown" | "html" | "htm" | "svg" | "png" | "jpg" | "jpeg")
    )
}

/// Every reportable metadata string in `bytes`, routed by format. `None` when the format has
/// no extractor; `Some(empty)` means understood, nothing reportable.
pub fn extract(path: &Path, bytes: &[u8], everything: bool) -> Option<Vec<Finding>> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "md" | "markdown" => Some(frontmatter(std::str::from_utf8(bytes).ok()?, everything)),
        "html" | "htm" => Some(html_meta(std::str::from_utf8(bytes).ok()?, everything)),
        "svg" => Some(svg_metadata(std::str::from_utf8(bytes).ok()?, everything)),
        "png" => Some(png_chunks(bytes, everything)),
        "jpg" | "jpeg" => Some(jpeg_segments(bytes, everything)),
        _ => None,
    }
}

// --- text formats ---------------------------------------------------------------------------

/// YAML frontmatter keys, shallowly: `key: value` lines between the opening `---` and the
/// next. Shallow on purpose — provenance keys sit at the top level, and a YAML parser is a
/// dependency this doesn't need for `generator: claude`.
///
/// A line is a key only when it *looks like one*: unindented, and named in plain
/// `[A-Za-z0-9_-]` characters. Anything else is prose — the continuation lines of a folded
/// scalar (`description: >`), quoted markup like `<meta …>: …` in a description — and prose
/// split on `:` mints garbage keys ("Remove multi-vendor AI provenance marks" was one).
/// Prose lines are not dropped outright, though: they still get the marker sweep, so a
/// vendor name buried in a folded description is found — it just cannot become a field.
fn frontmatter(text: &str, everything: bool) -> Vec<Finding> {
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Vec::new();
    }
    let mut found = Vec::new();
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        match yaml_key_value(line) {
            Some((key, value)) => found.extend(classify(
                format!("frontmatter \"{key}\""),
                key,
                value.trim_matches(['"', '\'']).to_string(),
                everything,
            )),
            // Prose: only a marker earns it a place — sweeping whole description
            // paragraphs into the report-everything mode would bury the fields.
            None => {
                if let Some(marker) = marker_in(line) {
                    found.push(Finding {
                        field: "frontmatter".to_string(),
                        value: line.trim().to_string(),
                        why: Why::Marker(marker),
                    });
                }
            }
        }
    }
    found
}

/// `line` as a top-level YAML `key: value` pair, or `None` when it reads as prose.
fn yaml_key_value(line: &str) -> Option<(&str, &str)> {
    if line.starts_with([' ', '\t']) {
        return None; // indented: a nested mapping or a folded scalar's continuation
    }
    let (key, value) = line.split_once(':')?;
    let key = key.trim_end();
    let plausible = !key.is_empty()
        && key.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
    let value = value.trim();
    // `>`/`|` (with optional chomping +/-) announce a folded/literal block: the value is the
    // indented lines that follow, which the prose sweep handles — the indicator itself says
    // nothing worth reporting.
    let block_indicator =
        matches!(value, ">" | "|" | ">-" | ">+" | "|-" | "|+");
    (plausible && !value.is_empty() && !block_indicator).then_some((key, value))
}

/// `<meta name|property="…" content="…">` tags, either attribute order.
fn html_meta(text: &str, everything: bool) -> Vec<Finding> {
    static META: OnceLock<Regex> = OnceLock::new();
    static NAME: OnceLock<Regex> = OnceLock::new();
    static CONTENT: OnceLock<Regex> = OnceLock::new();
    let meta = META.get_or_init(|| Regex::new(r"(?is)<meta\s[^>]*>").expect("fixed"));
    let name = NAME
        .get_or_init(|| Regex::new(r#"(?i)\b(?:name|property)\s*=\s*["']([^"']*)["']"#).expect("fixed"));
    let content =
        CONTENT.get_or_init(|| Regex::new(r#"(?i)\bcontent\s*=\s*["']([^"']*)["']"#).expect("fixed"));

    let mut found = Vec::new();
    for tag in meta.find_iter(text) {
        let tag = tag.as_str();
        let Some(key) = name.captures(tag).map(|c| c[1].to_string()) else { continue };
        let Some(value) = content.captures(tag).map(|c| c[1].to_string()) else { continue };
        found.extend(classify(format!("<meta {key}>"), &key, value, everything));
    }
    found
}

/// The inner text of `<metadata>` blocks, tags stripped. SVG provenance (RDF, XMP,
/// Inkscape's blocks) all lives there, and the marker scan only needs the words.
fn svg_metadata(text: &str, everything: bool) -> Vec<Finding> {
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    let block =
        BLOCK.get_or_init(|| Regex::new(r"(?is)<metadata[^>]*>(.*?)</metadata>").expect("fixed"));
    let tag = TAG.get_or_init(|| Regex::new(r"<[^>]*>").expect("fixed"));

    let mut found = Vec::new();
    for inner in block.captures_iter(text) {
        let words = tag.replace_all(&inner[1], " ");
        let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
        if words.is_empty() {
            continue;
        }
        found.extend(classify("<metadata>".to_string(), "metadata", words, everything));
    }
    found
}

// --- binary formats -------------------------------------------------------------------------

/// PNG text chunks (`tEXt`, `iTXt`), plus the C2PA container chunk by presence.
///
/// `zTXt` values are deflate-compressed; presence is reported for a provenance keyword, but
/// the value is not decoded — inflating it would be the first real dependency this scan
/// takes, for a chunk almost nothing writes.
fn png_chunks(bytes: &[u8], everything: bool) -> Vec<Finding> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    let mut found = Vec::new();
    let Some(mut rest) = bytes.strip_prefix(SIGNATURE) else { return found };

    while rest.len() >= 8 {
        let length = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let kind = &rest[4..8];
        let Some(data) = rest.get(8..8 + length.min(rest.len().saturating_sub(8))) else { break };

        match kind {
            b"tEXt" => {
                if let Some((key, value)) = split_nul(data) {
                    found.extend(classify(format!("PNG tEXt \"{key}\""), &key, value, everything));
                }
            }
            b"iTXt" => {
                // keyword \0 compressed? \0 method \0 language \0 translated \0 text
                if let Some((key, tail)) = split_nul_raw(data) {
                    let uncompressed = tail.first() == Some(&0);
                    let text = tail
                        .split(|byte| *byte == 0)
                        .nth(3)
                        .filter(|_| uncompressed)
                        .and_then(|raw| std::str::from_utf8(raw).ok());
                    if let Some(value) = text {
                        found.extend(classify(
                            format!("PNG iTXt \"{key}\""),
                            &key,
                            value.to_string(),
                            everything,
                        ));
                    }
                }
            }
            b"zTXt" => {
                if let Some((key, _)) = split_nul_raw(data) {
                    if is_provenance_field(&key) {
                        found.push(Finding {
                            field: format!("PNG zTXt \"{key}\""),
                            value: "<compressed text chunk — value not decoded>".to_string(),
                            why: Why::ProvenanceField,
                        });
                    }
                }
            }
            // The C2PA container chunk: its being there is the finding.
            b"caBX" => found.push(Finding {
                field: "PNG chunk caBX".to_string(),
                value: "C2PA / JUMBF content-credentials container present".to_string(),
                why: Why::Marker("c2pa".to_string()),
            }),
            _ => {}
        }
        // length + type + data + CRC
        let advance = 8 + length + 4;
        if advance > rest.len() {
            break; // malformed length: stop rather than trust it
        }
        rest = &rest[advance..];
    }
    found
}

/// Latin-1 keyword before the first NUL, UTF-8-ish text after — the `tEXt` layout.
fn split_nul(data: &[u8]) -> Option<(String, String)> {
    let (key, value) = split_nul_raw(data)?;
    Some((key, String::from_utf8_lossy(value).into_owned()))
}

fn split_nul_raw(data: &[u8]) -> Option<(String, &[u8])> {
    let nul = data.iter().position(|byte| *byte == 0)?;
    let key = std::str::from_utf8(&data[..nul]).ok()?.to_string();
    Some((key, &data[nul + 1..]))
}

/// JPEG APP segments: XMP packets (APP1) as text, EXIF (APP1) as printable runs, and the
/// C2PA JUMBF container (APP11) by presence. Walks segment headers only — never the entropy-
/// coded image data, which starts at SOS and is where a JPEG's bulk lives.
fn jpeg_segments(bytes: &[u8], everything: bool) -> Vec<Finding> {
    let mut found = Vec::new();
    let Some(mut rest) = bytes.strip_prefix(b"\xff\xd8") else { return found };

    while rest.len() >= 4 && rest[0] == 0xFF {
        let marker = rest[1];
        if marker == 0xDA {
            break; // start of scan: metadata is behind us
        }
        let length = usize::from(u16::from_be_bytes([rest[2], rest[3]]));
        let Some(payload) = rest.get(4..2 + length) else { break };

        match marker {
            0xE1 if payload.starts_with(b"http://ns.adobe.com/xap/1.0/\0") => {
                let xmp = String::from_utf8_lossy(&payload[29..]);
                found.extend(xmp_findings(&xmp, everything));
            }
            0xE1 if payload.starts_with(b"Exif\0\0") => {
                // No TIFF walk: the textual tags surface as printable runs, and those are
                // what the markers could hide in.
                for run in printable_runs(&payload[6..]) {
                    found.extend(classify("EXIF text".to_string(), "exif", run, everything));
                }
            }
            0xEB => {
                let jumbf = payload.windows(4).any(|w| w == b"jumb" || w == b"c2pa" || w == b"JP  ");
                if jumbf || everything {
                    found.push(Finding {
                        field: "JPEG APP11".to_string(),
                        value: "JUMBF segment (C2PA content credentials live here)".to_string(),
                        why: if jumbf {
                            Why::Marker("c2pa".to_string())
                        } else {
                            Why::Everything
                        },
                    });
                }
            }
            _ => {}
        }
        rest = &rest[2 + length..];
    }
    found
}

/// XMP fields worth naming: `CreatorTool` however namespaced, both attribute and element
/// spellings — plus a whole-packet marker sweep so a vendor name anywhere in the XMP is
/// caught even in a field this doesn't name.
fn xmp_findings(xmp: &str, everything: bool) -> Vec<Finding> {
    static ATTR: OnceLock<Regex> = OnceLock::new();
    static ELEM: OnceLock<Regex> = OnceLock::new();
    let attribute = ATTR
        .get_or_init(|| Regex::new(r#"(?i)\bxmp:CreatorTool\s*=\s*["']([^"']*)["']"#).expect("fixed"));
    let element = ELEM.get_or_init(|| {
        Regex::new(r"(?is)<xmp:CreatorTool[^>]*>(.*?)</xmp:CreatorTool>").expect("fixed")
    });

    let mut found = Vec::new();
    for value in attribute
        .captures_iter(xmp)
        .chain(element.captures_iter(xmp))
        .map(|c| c[1].trim().to_string())
    {
        found.extend(classify("XMP CreatorTool".to_string(), "creatortool", value, everything));
    }
    if found.is_empty() {
        if let Some(marker) = marker_in(xmp) {
            found.push(Finding {
                field: "XMP packet".to_string(),
                value: format!("marker inside the XMP data: {marker}"),
                why: Why::Marker(marker),
            });
        } else if everything {
            found.extend(classify("XMP packet".to_string(), "xmp", xmp.to_string(), true));
        }
    }
    found
}

/// Runs of printable ASCII at least eight bytes long — `strings(1)`'s idea, tuned longer so
/// binary coincidence (four readable bytes happen constantly) stays out of the report.
fn printable_runs(data: &[u8]) -> Vec<String> {
    let mut runs = Vec::new();
    let mut current = String::new();
    for byte in data {
        if (0x20..0x7F).contains(byte) {
            current.push(*byte as char);
        } else {
            if current.len() >= 8 {
                runs.push(std::mem::take(&mut current));
            }
            current.clear();
        }
    }
    if current.len() >= 8 {
        runs.push(current);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boundary rule, exactly as specified: line-start/end, space, `-`, `—` — nothing
    /// else. The negatives are the words that made a bare substring unusable.
    #[test]
    fn ai_matches_as_a_token_and_never_inside_a_word() {
        for hit in [
            "AI",
            "an AI tool",
            "AI-generated",
            "made by —AI— here",
            "the A.I. did it",
            "ai",
            // The trailing period is a boundary: sentence-final is how prose ends.
            "Created with AI.",
            "polished by A.I.",
        ] {
            assert!(marker_in(hit).is_some(), "{hit:?} should match");
        }
        for miss in ["maintain", "email chain", "said", "available", "détail", "aid", "air"] {
            assert_eq!(marker_in(miss), None, "{miss:?} must not match");
        }
        // The period is a TRAILING boundary only: a leading one is how domains spell, and a
        // word ending in `ai` doesn't become a marker by ending a sentence.
        for miss in ["mistral.ai", "hosted on x.ai", "we flew to Dubai."] {
            assert_eq!(marker_in(miss), None, "{miss:?} must not match");
        }
    }

    #[test]
    fn vendors_match_as_substrings_case_insensitively() {
        assert_eq!(marker_in("Made with CLAUDE code"), Some("claude".to_string()));
        assert_eq!(marker_in("stable diffusion v1.5"), Some("stable diffusion".to_string()));
        assert_eq!(marker_in("plain description"), None);
    }

    /// A provenance field's value is reported whatever it says — "what wrote this" is the
    /// question, so the answer is interesting even when it names no vendor we know.
    #[test]
    fn provenance_fields_report_any_value_and_other_fields_need_a_marker() {
        let text = "---\ngenerator: SomeUnknownTool 3.1\ntitle: my notes\n---\nbody";
        let found = frontmatter(text, false);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].why, Why::ProvenanceField);
        assert!(found[0].field.contains("generator"));

        let with_marker = "---\ntitle: drafted by Claude\n---\n";
        let found = frontmatter(with_marker, false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].why, Why::Marker("claude".to_string()));
    }

    /// The SKILL.md shape that minted a sentence as a key: a folded scalar's continuation
    /// lines are prose, not keys — but a marker inside them is still found, because dropping
    /// the line outright would lose a real match sharing it.
    #[test]
    fn folded_scalar_prose_is_not_a_key_but_its_markers_still_count() {
        let text = "---\nname: my-skill\ndescription: >\n  Remove provenance marks: invisible Unicode,\n  covers Claude and friends.\n  <meta generator>: an example, plus Gemini too.\n---\n";
        let found = frontmatter(text, true);
        assert!(
            found.iter().all(|f| !f.field.contains("Remove") && !f.field.contains("<meta")),
            "prose and quoted markup must not become fields: {found:?}"
        );
        assert!(
            found.iter().all(|f| f.value != ">"),
            "a block-scalar indicator is syntax, not a value: {found:?}"
        );
        let markers: Vec<&str> = found
            .iter()
            .filter_map(|f| match &f.why {
                Why::Marker(m) => Some(m.as_str()),
                _ => None,
            })
            .collect();
        assert!(markers.contains(&"claude"), "a marker in prose is still found: {found:?}");
        assert!(markers.contains(&"gemini"), "even on a line quoting markup: {found:?}");
        // And report-everything does not dump the prose lines themselves.
        assert!(
            found.iter().filter(|f| f.why == Why::Everything).all(|f| f.field != "frontmatter"),
            "prose only surfaces on a marker match: {found:?}"
        );
    }

    #[test]
    fn report_all_sweeps_up_the_rest_and_says_so() {
        let text = "---\ntitle: my notes\n---\n";
        assert!(frontmatter(text, false).is_empty(), "nothing matches, nothing reported");
        let all = frontmatter(text, true);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].why, Why::Everything);
    }

    #[test]
    fn html_meta_reads_both_attribute_orders() {
        let html = r#"<html><head>
            <meta name="generator" content="Claude Artifacts">
            <meta content="unrelated" name="viewport">
        </head></html>"#;
        let found = html_meta(html, false);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].why, Why::Marker("claude".to_string()));
    }

    #[test]
    fn svg_metadata_blocks_are_scanned_with_tags_stripped() {
        let svg = "<svg><metadata><rdf:RDF><dc:creator>Midjourney</dc:creator></rdf:RDF></metadata></svg>";
        let found = svg_metadata(svg, false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].why, Why::Marker("midjourney".to_string()));
    }

    /// A synthesized PNG: real signature, one tEXt chunk, no image data needed — the walk
    /// reads chunk headers, not pixels.
    #[test]
    fn png_text_chunks_are_walked_and_software_is_a_provenance_field() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let data = b"Software\0My Own Editor";
        png.extend((data.len() as u32).to_be_bytes());
        png.extend(b"tEXt");
        png.extend(data);
        png.extend([0_u8; 4]); // CRC, unchecked
        let found = png_chunks(&png, false);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].why, Why::ProvenanceField);
        assert_eq!(found[0].value, "My Own Editor");

        // Stable Diffusion's calling card: the whole prompt under "parameters".
        let mut sd = b"\x89PNG\r\n\x1a\n".to_vec();
        let data = b"parameters\0a cat, masterpiece, 8k";
        sd.extend((data.len() as u32).to_be_bytes());
        sd.extend(b"tEXt");
        sd.extend(data);
        sd.extend([0_u8; 4]);
        assert_eq!(png_chunks(&sd, false).len(), 1, "the SD prompt chunk is provenance");
    }

    /// A malformed chunk length must stop the walk, not send it out of bounds.
    #[test]
    fn a_lying_png_length_stops_the_walk_cleanly() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(u32::MAX.to_be_bytes());
        png.extend(b"tEXt");
        png.extend(b"Software\0x");
        assert!(png_chunks(&png, false).len() <= 1, "no panic, no runaway");
    }

    /// A synthesized JPEG with an XMP APP1: CreatorTool comes out with its value.
    #[test]
    fn jpeg_xmp_creatortool_is_extracted() {
        let xmp = br#"<x:xmpmeta><rdf:Description xmp:CreatorTool="Adobe Firefly 2.0"/></x:xmpmeta>"#;
        let mut payload = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        payload.extend_from_slice(xmp);
        let mut jpeg = b"\xff\xd8".to_vec();
        jpeg.push(0xFF);
        jpeg.push(0xE1);
        jpeg.extend(((payload.len() + 2) as u16).to_be_bytes());
        jpeg.extend(&payload);
        jpeg.extend(b"\xff\xda"); // SOS: the walk must stop here
        let found = jpeg_segments(&jpeg, false);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].field, "XMP CreatorTool");
        assert_eq!(found[0].why, Why::Marker("firefly".to_string()));
        assert_eq!(found[0].value, "Adobe Firefly 2.0");
    }

    /// APP11 with JUMBF magic is C2PA — presence is the finding.
    #[test]
    fn jpeg_app11_jumbf_is_flagged_as_c2pa() {
        let payload = b"..jumb..";
        let mut jpeg = b"\xff\xd8".to_vec();
        jpeg.push(0xFF);
        jpeg.push(0xEB);
        jpeg.extend(((payload.len() + 2) as u16).to_be_bytes());
        jpeg.extend(payload);
        let found = jpeg_segments(&jpeg, false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].why, Why::Marker("c2pa".to_string()));
    }

    /// The runs rule: eight printable bytes or more, so binary coincidence stays out.
    #[test]
    fn printable_runs_ignore_short_coincidences() {
        let data = b"\x00\x01ab\x02LongEnoughRun\x03tiny\x04";
        assert_eq!(printable_runs(data), vec!["LongEnoughRun".to_string()]);
    }

    /// Formats without an extractor are `None` (skip), understood-but-empty is `Some([])`.
    #[test]
    fn unknown_formats_are_skipped_not_guessed() {
        assert!(!handles(Path::new("x.rs")), "no extractor, no read");
        assert!(handles(Path::new("x.PNG")), "routing is case-insensitive");
        assert!(extract(Path::new("x.rs"), b"fn main() {}", false).is_none());
        assert_eq!(
            extract(Path::new("x.md"), b"plain body, no frontmatter", false),
            Some(Vec::new())
        );
    }
}
