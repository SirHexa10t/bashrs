//! Who made a device, from its MAC address — the IEEE OUI registry, however this machine can
//! reach it.
//!
//! # Why this isn't a table in the binary
//!
//! The registry is ~40 000 assignments and ~1 MB even compacted, against a 13 MB binary — carrying
//! it would be an 8% cost for a convenience column, and it would be *stale from the day it was
//! compiled*. So it is treated like every other bulky resource this project depends on: fetched
//! once into `~/.bashrs`, refreshed by a compile, and read from disk.
//!
//! A hand-curated list of ~70 prefixes used to stand in for this. It was worse than it looked: on
//! a real home network every single device came back with no vendor at all, because a curated list
//! covers the vendors someone thought of and real hardware is made by companies nobody lists —
//! `88:d0:39` is Tonly Technology, `00:b8:c2` is Heights Telecom. The curated list survives as
//! [`super::lan`]'s *hint* source (what a vendor implies about a device), which is a different and
//! much smaller job.
//!
//! # Sources, in order
//!
//! 1. **A system database**, where the distribution ships one — nmap, wireshark and `hwdata` all
//!    carry the registry, and reading theirs costs nothing and stays current with their updates.
//! 2. **Our own cache**, `~/.bashrs/user-data/oui.tsv`, written by [`refresh`].
//!
//! All of them are read into one map on first use and never again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where distributions put the registry. Read-only, and whichever appears first wins — they hold
/// the same data in different spellings, all handled by [`parse_line`].
const SYSTEM_SOURCES: &[&str] = &[
    "/usr/share/nmap/nmap-mac-prefixes",
    "/usr/share/wireshark/manuf",
    "/usr/share/hwdata/oui.txt",
    "/usr/share/misc/oui.txt",
    "/var/lib/ieee-data/oui.txt",
    "/usr/share/arp-scan/ieee-oui.txt",
];

/// The IEEE's own published registry — the source [`refresh`] fetches when no system copy exists.
const IEEE_CSV: &str = "https://standards-oui.ieee.org/oui/oui.csv";

/// Where our own compacted copy lives — one `hex<TAB>vendor` line per assignment.
///
/// Supplied by the caller rather than derived here: `support` sits below `conf` in the crate's
/// layering and may not reach up to ask where `~/.bashrs` is, so the layer that owns that answer
/// hands it down once ([`install_cache_path`]) — the same shape `tools::install` uses. Unset, only
/// the system registries are consulted, which is a perfectly good degradation.
static CACHE_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Tell this module where its cache may live. First call wins; later ones are ignored, so nothing
/// can move the file under a lookup already in flight.
pub(crate) fn install_cache_path(path: PathBuf) {
    let _ = CACHE_PATH.set(path);
}

/// The configured cache path, if a caller supplied one.
pub(crate) fn cache_path() -> Option<&'static Path> {
    CACHE_PATH.get().map(PathBuf::as_path)
}

/// The loaded registry, keyed by the OUI's three bytes packed into a `u32`.
static REGISTRY: OnceLock<HashMap<u32, String>> = OnceLock::new();

/// The manufacturer that registered `mac`'s prefix, or `None` when the registry doesn't have it
/// (or isn't available). `mac` may be any common spelling — the first three octets are what count.
pub(crate) fn vendor(mac: &str) -> Option<&'static str> {
    let registry = REGISTRY.get_or_init(load);
    registry.get(&prefix_of(mac)?).map(String::as_str)
}

/// The three-byte OUI of a MAC, packed — `aa:bb:cc:…` → `0x00AABBCC`. Accepts `:`/`-` separated
/// and bare hex, since the spellings turn up in different places.
fn prefix_of(mac: &str) -> Option<u32> {
    let hex: String =
        mac.chars().filter(|character| character.is_ascii_hexdigit()).take(6).collect();
    (hex.len() == 6).then(|| u32::from_str_radix(&hex, 16).ok())?
}


/// Where the loaded registry came from, for the same diagnostic reason.
pub(crate) fn source() -> Option<PathBuf> {
    SYSTEM_SOURCES
        .iter()
        .map(PathBuf::from)
        .chain(cache_path().map(Path::to_path_buf))
        .find(|path| path.is_file())
}

/// Read the first available source into a lookup. Any failure yields an empty map — a missing
/// vendor column is a small loss, and never a reason for a scan to fail.
fn load() -> HashMap<u32, String> {
    let Some(path) = source() else { return HashMap::new() };
    let Ok(text) = std::fs::read_to_string(&path) else { return HashMap::new() };
    parse(&text)
}

/// Parse a registry in any of the shipped spellings.
pub(crate) fn parse(text: &str) -> HashMap<u32, String> {
    text.lines().filter_map(parse_line).collect()
}

/// One registry line → `(packed OUI, vendor)`.
///
/// Four formats are in circulation and they are distinguishable without a mode flag:
/// - IEEE CSV — `MA-L,AABBCC,Vendor Name,Address`
/// - our cache / wireshark `manuf` — `AA:BB:CC<TAB>Vendor`
/// - nmap `nmap-mac-prefixes` — `AABBCC Vendor Name`
/// - IEEE `oui.txt` — `AA-BB-CC   (hex)\t\tVENDOR NAME`
///
/// Everything hinges on the first field being six hex digits once separators are dropped, so the
/// parse is: take the first token, see whether it is an OUI, and treat the rest as the name.
fn parse_line(line: &str) -> Option<(u32, String)> {
    // `.trim()` also sheds the CR of the IEEE's CRLF line endings.
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // The IEEE CSV announces itself: a registry class, then the OUI, then the name — and any of
    // those fields may be quoted, because plenty of company names contain a comma ("Google, Inc.").
    // Splitting such a line on commas is exactly the bug that put a postal address in the vendor
    // column, so it gets a real field parse.
    if let Some(rest) = ["MA-L,", "MA-M,", "MA-S,", "CID,", "IAB,"]
        .iter()
        .find_map(|registry| line.strip_prefix(registry))
    {
        let fields = csv_fields(rest);
        let (oui, name) = (fields.first()?, fields.get(1)?);
        return Some((exactly_six_hex(oui)?, tidy(name)?));
    }
    // Every other spelling is whitespace-separated: the OUI first, the name after.
    let (head, rest) = line.split_once(['\t', ' ']).unwrap_or((line, ""));
    let prefix = exactly_six_hex(head)?;
    // `oui.txt` writes `(hex)` between the prefix and the name; `manuf` carries a short name and
    // then a long one, tab-separated — the short one is the better column.
    let name = rest
        .trim()
        .trim_start_matches("(hex)")
        .trim_start_matches("(base 16)")
        .trim()
        .split('\t')
        .next()
        .unwrap_or("");
    Some((prefix, tidy(name)?))
}

/// Split one CSV record into fields, honouring double-quoted fields (which may contain commas)
/// and the doubled `""` that escapes a quote inside one.
fn csv_fields(record: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut characters = record.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' if quoted && characters.peek() == Some(&'"') => {
                current.push('"');
                characters.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut current)),
            other => current.push(other),
        }
    }
    fields.push(current);
    fields
}

/// A field that is exactly a six-hex-digit OUI, packed — rejecting anything longer, so a stray
/// hex blob can't pass for a prefix.
fn exactly_six_hex(field: &str) -> Option<u32> {
    let field = field.trim();
    let digits = field.chars().filter(|character| character.is_ascii_hexdigit()).count();
    (digits == 6).then(|| prefix_of(field))?
}

/// A vendor name as it should appear in a column: trimmed, and shorn of the corporate suffixes
/// that make every second entry a paragraph. `None` for a name that turns out to be nothing.
fn tidy(name: &str) -> Option<String> {
    let name = name.trim().trim_matches('"').trim();
    // Longest first, so "Co. Ltd" doesn't strip to "Co." and stop.
    let trimmed = [
        ", Inc.", ", Inc", " Inc.", " Inc", ", LLC", " LLC", ", Ltd.", ", Ltd", " Co. Ltd",
        " Co.,Ltd", " Co., Ltd", " Co Ltd", " Ltd.", " Ltd", " GmbH", " B.V.", " S.A.", " A/S",
        " Corporation", " Corp.", " Corp", " Technologies", " Technology Co",
    ]
    .iter()
    .fold(name.to_string(), |name, suffix| {
        match name.len() > suffix.len() && name.to_lowercase().ends_with(&suffix.to_lowercase()) {
            true => name[..name.len() - suffix.len()].trim_end_matches([' ', ',']).to_string(),
            false => name,
        }
    });
    let trimmed = trimmed.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// SIDE EFFECTS — fetch the IEEE registry and write the compacted cache. Returns how many
/// assignments landed.
///
/// Skipped entirely when a system database already covers it: distributions keep theirs current,
/// and a second copy would only be another thing to go stale.
pub(crate) fn refresh() -> Result<usize, String> {
    if let Some(path) = SYSTEM_SOURCES.iter().map(Path::new).find(|path| path.is_file()) {
        return Err(format!("a system registry is already present ({})", path.display()));
    }
    let csv = crate::support::exec::capture_stdout("curl", ["-fsSL", "--max-time", "120", IEEE_CSV])
        .ok_or_else(|| "could not fetch the IEEE registry (offline?)".to_string())?;
    let parsed = parse(&csv);
    if parsed.is_empty() {
        return Err("the fetched registry held no usable entries".to_string());
    }
    // Compacted on the way in: the published CSV is 3.7 MB of which the addresses are most, and
    // the cache is re-read on every scan.
    let mut lines: Vec<String> =
        parsed.iter().map(|(prefix, name)| format!("{prefix:06x}\t{name}")).collect();
    lines.sort_unstable();
    let path = cache_path().ok_or("no cache location was configured")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }
    std::fs::write(path, lines.join("\n") + "\n")
        .map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(parsed.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefix_is_read_from_any_mac_spelling() {
        assert_eq!(prefix_of("aa:bb:cc:dd:ee:ff"), Some(0x00AA_BBCC));
        assert_eq!(prefix_of("AA-BB-CC-DD-EE-FF"), Some(0x00AA_BBCC));
        assert_eq!(prefix_of("aabbccddeeff"), Some(0x00AA_BBCC));
        assert_eq!(prefix_of("aa:bb:cc"), Some(0x00AA_BBCC), "the prefix alone is enough");
        assert_eq!(prefix_of("aa:bb"), None, "too few octets to name a manufacturer");
        assert_eq!(prefix_of(""), None);
        assert_eq!(prefix_of("not-a-mac"), None, "letters past f are not hex");
    }

    /// The four spellings in circulation, each parsed without being told which it is.
    #[test]
    fn every_shipped_registry_format_parses() {
        // IEEE CSV, verbatim from the published file — note that a name containing a comma is
        // quoted while its address is not, and vice versa. Splitting on commas gets this wrong.
        assert_eq!(
            parse_line("MA-L,CCF411,\"Google, Inc.\",1600 Amphitheatre Parkway Mountain View CA US 94043 "),
            Some((0x00CC_F411, "Google".to_string())),
            "a quoted name with a comma must survive, and the address must not leak in"
        );
        assert_eq!(
            parse_line("MA-L,88D039,Tonly Technology Co. Ltd ,\"Section 37, Zhongkai Hi-Tech Zone CN\""),
            Some((0x0088_D039, "Tonly Technology".to_string())),
            "an unquoted name beside a QUOTED address holding commas"
        );
        assert_eq!(
            parse_line("MA-L,00B8C2,Heights Telecom T ltd,Moshe Lerer 15 Nes Ziona   IL 7404996 "),
            Some((0x0000_B8C2, "Heights Telecom T".to_string())), "the legal suffix goes, case-insensitively"
        );
        // Our own compacted cache, and wireshark's `manuf`.
        assert_eq!(
            parse_line("88d039\tTonly Technology"),
            Some((0x0088_D039, "Tonly Technology".to_string()))
        );
        assert_eq!(
            parse_line("00:B8:C2\tHeights Telecom T ltd"),
            Some((0x0000_B8C2, "Heights Telecom T".to_string())), "the legal suffix goes, case-insensitively"
        );
        // nmap's prefixes file.
        assert_eq!(
            parse_line("B827EB Raspberry Pi Foundation"),
            Some((0x00B8_27EB, "Raspberry Pi Foundation".to_string()))
        );
        // IEEE's classic oui.txt.
        assert_eq!(
            parse_line("00-1A-11   (hex)\t\tGoogle, Inc."),
            Some((0x0000_1A11, "Google".to_string()))
        );
    }

    /// The corporate suffixes are stripped so the column reads as a manufacturer rather than a
    /// registration — but only as a suffix, never mid-name.
    #[test]
    fn vendor_names_lose_their_corporate_tail_and_nothing_else() {
        assert_eq!(tidy("Google, Inc.").as_deref(), Some("Google"));
        // The legal tail goes; the company name itself stays whole.
        assert_eq!(tidy("Tonly Technology Co. Ltd ").as_deref(), Some("Tonly Technology"));
        assert_eq!(tidy("Cisco Systems, Inc").as_deref(), Some("Cisco Systems"));
        assert_eq!(tidy("AVM GmbH").as_deref(), Some("AVM"));
        assert_eq!(tidy("Raspberry Pi Foundation").as_deref(), Some("Raspberry Pi Foundation"));
        // A name that IS its suffix keeps itself rather than vanishing.
        assert_eq!(tidy("Ltd").as_deref(), Some("Ltd"));
        assert_eq!(tidy("   ").as_deref(), None);
        assert_eq!(tidy("").as_deref(), None);
    }

    #[test]
    fn csv_fields_respect_quotes_and_escaped_quotes() {
        assert_eq!(csv_fields("a,b,c"), ["a", "b", "c"]);
        assert_eq!(csv_fields(r#"a,"b,still b",c"#), ["a", "b,still b", "c"]);
        assert_eq!(csv_fields(r#""say ""hi""",next"#), [r#"say "hi""#, "next"]);
        assert_eq!(csv_fields(""), [""]);
    }

    #[test]
    fn noise_and_headers_are_not_assignments() {
        for line in [
            "",
            "   ",
            "# a comment",
            "Registry,Assignment,Organization Name,Organization Address",
            "OUI/MA-L                                                    Organization",
            "not hex at all,Some Vendor",
            "AABBCC,", // an OUI with no vendor names nobody
        ] {
            assert_eq!(parse_line(line), None, "should not parse: {line:?}");
        }
    }

    /// A whole (small) registry, parsed and looked up the way a scan does it.
    #[test]
    fn a_registry_round_trips_from_text_to_lookup() {
        let text = "\
# comment
ccf411\tGoogle
88d039\tTonly Technology
00b8c2\tHeights Telecom T ltd
";
        let registry = parse(text);
        assert_eq!(registry.len(), 3);
        assert_eq!(registry.get(&prefix_of("cc:f4:11:a2:bf:ae").unwrap()).unwrap(), "Google");
        assert_eq!(registry.get(&prefix_of("88:D0:39:7F:72:20").unwrap()).unwrap(), "Tonly Technology");
        assert!(!registry.contains_key(&prefix_of("00:f0:21:0f:f0:02").unwrap()), "unassigned");
    }

    /// The end-to-end path — source discovery, file read, parse, lookup — against whatever
    /// registry this machine actually has. Skipped with a notice where none is installed, so it
    /// proves the real thing where it can and never fails where it can't.
    #[test]
    fn a_real_installed_registry_resolves_real_manufacturers() {
        let Some(path) = source() else {
            eprintln!("SKIPPED: no OUI registry on this machine (net_local --refresh-vendors)");
            return;
        };
        assert!(!parse(&std::fs::read_to_string(&path).unwrap()).is_empty(),
            "a registry at {} must load", path.display());
        // Prefixes with long-standing, unambiguous assignments.
        for (mac, expect) in [("b8:27:eb:11:22:33", "Raspberry"), ("cc:f4:11:00:00:01", "Google")] {
            let found = vendor(mac).unwrap_or("");
            assert!(found.contains(expect), "{mac} resolved to {found:?}, expected ~{expect}");
        }
        // A prefix nobody registered stays unknown rather than being invented.
        assert_eq!(vendor("00:f0:21:0f:f0:02"), None, "an unassigned prefix has no manufacturer");
    }

    /// A machine with no registry at all must degrade to a blank column, never to a failure —
    /// which is exactly the state of the machine this was written on.
    #[test]
    fn an_absent_registry_is_an_empty_lookup_not_an_error() {
        assert!(parse("").is_empty());
        // `vendor` consults a lazily-loaded global; on a machine with no source it answers None.
        let _ = vendor("aa:bb:cc:dd:ee:ff");
    }
}
