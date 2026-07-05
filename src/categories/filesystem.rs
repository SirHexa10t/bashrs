//! Filesystem commands (`fs_*`). Currently `lll` — a long `ls` listing aligned into a
//! table via `table_formatter`, under a bold-blue header from the style engine. Windows
//! `.lnk` shortcuts (which can't work under Bash) are flagged red as `name.lnk (Windows)->
//! target`, treating them like broken links; the target is read from the shortcut when we
//! can, and omitted (still flagged) when we can't.

#[bashrs_macros::category(command = FilesystemCommand, prefix = "fs_")]
mod commands {
    use std::collections::HashMap;
    use std::path::Path;

    use crate::categories::autogen_styles::{_scoped, _wrap};
    use crate::support::exec;
    use clap::Args;

    /// Custom `ls` (like `ll`): a long, all-files listing aligned into a table. Extra
    /// arguments pass straight through to `ls` — e.g. `lll -S` sorts by size.
    #[unprefixed]
    pub fn lll(args: LllArgs) {
        let mut ls_args: Vec<&str> = LS_FLAGS.to_vec();
        ls_args.extend(args.passthrough.iter().map(String::as_str));
        if let Some(output) = exec::capture_stdout("ls", ls_args) {
            let targets = _lnk_targets(&args.passthrough);
            for row in _format_listing(&output, &targets) {
                println!("{row}");
            }
        }
    }

    /// Paths and/or `ls` flags, passed straight through to `ls` (e.g. `-S` to sort by
    /// size, `-R` to recurse).
    #[derive(Args)]
    pub struct LllArgs {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        pub passthrough: Vec<String>,
    }

    /// The `ls` flags that define an `lll` listing: long, all, mtime-sorted (reversed),
    /// block + human sizes, type indicators, literal names, coloured, directories first.
    const LS_FLAGS: &[&str] =
        &["-tarlushFN", "--color=always", "--time-style=+%F_%T", "--group-directories-first"];
    /// Metadata fields `ls -l` prints before the name: blocks, permissions, hard-links,
    /// owner, group, size, date.
    const META_COLS: usize = 7;
    /// Column labels, matched to `LS_FLAGS` (the 7 metadata columns, then the name).
    const HEADER: &[&str] =
        &["Blocks", "Permissions", "Hard-Links", "Owner", "Group", "Size", "Date_Modified", "Filename"];
    /// Spaces between aligned columns (matches `table_formatter`'s default).
    const TABLE_SEPARATOR: usize = 2;

    // ——— Formatting (pure) ————————————————————————————————————————————————

    /// Turn raw `ls -l` output into aligned rows under a bold-blue header, flagging any
    /// `.lnk` entry (see [`_name_cell`]). Pure given `targets`, so it's unit-tested.
    fn _format_listing(ls_output: &str, targets: &HashMap<String, String>) -> Vec<String> {
        let header = _scoped(&_wrap(["bo", "", "b"]), &HEADER.join("\t"));
        let rows: Vec<String> = std::iter::once(header)
            .chain(
                ls_output
                    .lines()
                    .filter(|line| !line.starts_with("total ")) // drop `ls -l`'s block-count line
                    .map(|line| _row(line, targets)),
            )
            .collect();
        table_formatter::format_table(&rows, TABLE_SEPARATOR, None)
    }

    /// One listing row: the first [`META_COLS`] whitespace fields tab-delimited, then the
    /// name as a single trailing column (see [`_name_cell`]).
    fn _row(line: &str, targets: &HashMap<String, String>) -> String {
        let mut fields = line.split_whitespace();
        let meta: Vec<&str> = fields.by_ref().take(META_COLS).collect();
        let name = fields.collect::<Vec<_>>().join(" "); // runs collapsed → can't split the column
        if name.is_empty() {
            meta.join("\t")
        } else {
            format!("{}\t{}", meta.join("\t"), _name_cell(&name, targets))
        }
    }

    /// Render the name column. A `.lnk` shortcut is treated as a broken Windows link: the
    /// name (and its target, when known) go red via `recho`'s style, joined by a plain
    /// ` (Windows)-> ` marker so it fits `ls`'s aesthetic and reads unambiguously. Any
    /// other entry keeps `ls`'s own colouring untouched.
    fn _name_cell(name: &str, targets: &HashMap<String, String>) -> String {
        let plain = table_formatter::strip_ansi(name);
        let plain = plain.strip_suffix(|c: char| "*/=>@|".contains(c)).unwrap_or(&plain); // -F indicator
        if !plain.ends_with(".lnk") {
            return name.to_string();
        }
        let red = |s: &str| _scoped(&_wrap(["bo", "", "r"]), s);
        match targets.get(plain) {
            Some(target) => {
                let target = target.split_whitespace().collect::<Vec<_>>().join(" ");
                format!("{} (Windows)-> {}", red(plain), red(&target))
            }
            None => format!("{} (Windows)->", red(plain)),
        }
    }

    // ——— `.lnk` targets (I/O + parsing) ——————————————————————————————————

    /// Resolve `.lnk` targets for the entries being listed, keyed by filename. Only the
    /// unambiguous cases are resolved — a single directory (or a single `.lnk` file), and
    /// not recursive — because a recursive or multi-path listing could repeat a name across
    /// directories; those entries are still flagged, just without a target.
    fn _lnk_targets(passthrough: &[String]) -> HashMap<String, String> {
        let mut map = HashMap::new();
        if passthrough.iter().any(|a| _is_recursive_flag(a)) {
            return map;
        }
        let paths: Vec<&str> = passthrough.iter().map(String::as_str).filter(|a| !a.starts_with('-')).collect();
        match paths.as_slice() {
            [] => _collect_lnks(Path::new("."), &mut map),
            [one] if Path::new(one).is_dir() => _collect_lnks(Path::new(one), &mut map),
            [one] => _add_lnk(Path::new(one), &mut map),
            _ => {} // multiple paths → names could collide, so leave targets unresolved
        }
        map
    }

    fn _is_recursive_flag(arg: &str) -> bool {
        arg == "--recursive" || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('R'))
    }

    fn _collect_lnks(dir: &Path, map: &mut HashMap<String, String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            _add_lnk(&entry.path(), map);
        }
    }

    fn _add_lnk(path: &Path, map: &mut HashMap<String, String>) {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { return };
        if !name.ends_with(".lnk") {
            return;
        }
        if let Ok(bytes) = std::fs::read(path) {
            if let Some(target) = _lnk_target(&bytes) {
                map.insert(name, target);
            }
        }
    }

    /// Extract a Windows shortcut's local target path from its bytes, or `None` if it isn't
    /// a shortcut or stores no local path (IDList-only, network-only, etc.). A minimal read
    /// of the MS-SHLLINK format: header → flags → skip the optional IDList → `LinkInfo` →
    /// `LocalBasePath`. The path is ANSI, decoded lossily — fine for ASCII paths.
    fn _lnk_target(bytes: &[u8]) -> Option<String> {
        if _read_u32(bytes, 0)? != 0x4C {
            return None; // HeaderSize must be 76 (0x4C)
        }
        let flags = _read_u32(bytes, 20)?; // LinkFlags
        if flags & 0x2 == 0 {
            return None; // no LinkInfo → no LocalBasePath
        }
        let mut info = 76; // past the fixed-size header
        if flags & 0x1 != 0 {
            info += 2 + _read_u16(bytes, info)? as usize; // skip HasLinkTargetIDList (u16 size + data)
        }
        if _read_u32(bytes, info + 8)? & 0x1 == 0 {
            return None; // LinkInfoFlags lacks VolumeIDAndLocalBasePath
        }
        let base = info + _read_u32(bytes, info + 16)? as usize; // LocalBasePath offset, from LinkInfo start
        _read_cstr(bytes, base)
    }

    fn _read_u32(bytes: &[u8], off: usize) -> Option<u32> {
        bytes.get(off..off + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }

    fn _read_u16(bytes: &[u8], off: usize) -> Option<u16> {
        bytes.get(off..off + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap()))
    }

    fn _read_cstr(bytes: &[u8], start: usize) -> Option<String> {
        let rest = bytes.get(start..)?;
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        (end > 0).then(|| String::from_utf8_lossy(&rest[..end]).into_owned())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A realistic `ls -tarlushFN -l` data line: 7 metadata fields, then `name`.
        fn row(name: &str) -> String {
            format!("4 -rw-r--r-- 1 user group 1.2K 2024-01-01_12:00:00 {name}")
        }

        fn one_target(name: &str, target: &str) -> HashMap<String, String> {
            HashMap::from([(name.to_string(), target.to_string())])
        }

        /// A minimal valid `.lnk`: header (HasLinkInfo, no IDList) + a `LinkInfo` whose
        /// `LocalBasePath` is `target`.
        fn minimal_lnk(target: &str) -> Vec<u8> {
            let mut b = vec![0u8; 76];
            b[0] = 0x4C; // HeaderSize
            b[20] = 0x02; // LinkFlags: HasLinkInfo only
            let path = target.as_bytes();
            let header_size = 28u32; // 7 * u32, no Unicode fields
            let li = [header_size + path.len() as u32 + 1, header_size, 1, 0, header_size, 0, 0];
            b.extend(li.iter().flat_map(|v| v.to_le_bytes()));
            b.extend_from_slice(path);
            b.push(0);
            b
        }

        #[test]
        fn row_delimits_metadata_and_keeps_a_simple_name() {
            assert_eq!(
                _row(&row("file.txt"), &HashMap::new()),
                "4\t-rw-r--r--\t1\tuser\tgroup\t1.2K\t2024-01-01_12:00:00\tfile.txt"
            );
        }

        #[test]
        fn row_keeps_a_spaced_name_as_one_column() {
            assert!(_row(&row("my report final.txt"), &HashMap::new()).ends_with("\tmy report final.txt"));
        }

        #[test]
        fn row_keeps_a_symlink_arrow_together() {
            assert!(_row(&row("link  ->  /path/to/target"), &HashMap::new()).ends_with("\tlink -> /path/to/target"));
        }

        #[test]
        fn row_collapses_whitespace_runs_in_the_name() {
            assert!(_row(&row("two  spaces"), &HashMap::new()).ends_with("\ttwo spaces"));
            assert!(_row("4 -rw-r--r-- 1 u g 1K date a\tb\tc", &HashMap::new()).ends_with("\ta b c"));
        }

        #[test]
        fn row_survives_short_lines() {
            assert_eq!(_row("fragment", &HashMap::new()), "fragment");
            assert_eq!(_row("", &HashMap::new()), "");
        }

        #[test]
        fn name_cell_leaves_a_regular_entry_untouched() {
            let colored = "\x1b[01;34mnotes.txt\x1b[0m";
            assert_eq!(_name_cell(colored, &HashMap::new()), colored);
        }

        #[test]
        fn name_cell_flags_a_lnk_with_its_target_in_red() {
            let cell = _name_cell("shortcut.lnk", &one_target("shortcut.lnk", "C:\\app\\x.exe"));
            assert!(cell.contains("(Windows)->"), "marker missing: {cell}");
            assert!(cell.contains("C:\\app\\x.exe"), "target missing: {cell}");
            assert!(cell.contains("\x1b[1;31m"), "should be bold red: {cell}");
        }

        #[test]
        fn name_cell_flags_a_lnk_without_a_target_when_unresolved() {
            let cell = _name_cell("mystery.lnk", &HashMap::new());
            assert!(cell.trim_end().ends_with("(Windows)->"), "expected marker, no target: {cell}");
            assert!(cell.contains("\x1b[1;31m"), "should be bold red: {cell}");
        }

        #[test]
        fn name_cell_detects_lnk_through_ansi_and_type_indicator() {
            // `ls --color=always -F` can wrap the name in colour codes and append `*`.
            let cell = _name_cell("\x1b[01;32mrun.lnk\x1b[0m*", &HashMap::new());
            assert!(cell.contains("(Windows)->"), "should see .lnk under ANSI + indicator: {cell}");
        }

        #[test]
        fn format_listing_annotates_a_lnk_row_and_drops_total() {
            let out = _format_listing(&format!("total 0\n{}", row("go.lnk")), &one_target("go.lnk", "C:\\tools\\go.exe"));
            assert_eq!(out.len(), 2, "header + one row");
            assert!(out[1].contains("(Windows)->") && out[1].contains("C:\\tools\\go.exe"), "row: {}", out[1]);
        }

        #[test]
        fn format_listing_leads_with_a_header() {
            let out = _format_listing(&format!("total 8\n{}\n{}", row("a.txt"), row("b.txt")), &HashMap::new());
            assert_eq!(out.len(), 3, "header + two files");
            assert!(out[0].contains("Blocks") && out[0].contains("Filename"), "first row is the header");
        }

        #[test]
        fn lnk_target_reads_the_local_base_path() {
            assert_eq!(_lnk_target(&minimal_lnk("C:\\Users\\me\\file.txt")).as_deref(), Some("C:\\Users\\me\\file.txt"));
        }

        #[test]
        fn lnk_target_rejects_non_shortcuts_and_pathless_data() {
            assert_eq!(_lnk_target(b"not a shortcut at all...."), None); // wrong HeaderSize
            assert_eq!(_lnk_target(&[]), None); // empty
            let mut no_info = vec![0u8; 76];
            no_info[0] = 0x4C; // valid header, but no HasLinkInfo flag
            assert_eq!(_lnk_target(&no_info), None);
        }
    }
}
