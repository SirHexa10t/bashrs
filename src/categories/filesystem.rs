//! Filesystem commands (`fs_*`) and navigation helpers. Currently `lll` — a long `ls` listing
//! aligned into a table via `table_formatter`, under a bold-blue header from the style engine.
//! Windows `.lnk` shortcuts (which can't work under Bash) are flagged red as `name.lnk
//! (Windows)-> target`, treating them like broken links; the target is read from the shortcut
//! when we can, and omitted (still flagged) when we can't.

#[bashrs_macros::category(command = FilesystemCommand, prefix = "fs_")]
mod commands {
    use std::collections::{HashMap, HashSet};
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    use crate::support::doc_style::{self, _header};
    use crate::support::exec;
    use clap::Args;

    /// Hop one directory up
    #[name("..")]
    #[shell_body("cd .. \"$@\"")]
    pub fn dir_up() {}

    /// Custom `ls` (like `ll`): a long, all-files listing aligned into a table. Extra
    /// arguments pass straight through to `ls` — e.g. `lll -S` sorts by size.
    #[unprefixed]
    #[trailing_newline]
    pub fn lll(args: LllArgs) {
        let mut ls_args: Vec<&str> = LS_FLAGS.to_vec();
        ls_args.extend(args.passthrough.iter().map(String::as_str));
        if let Some(output) = exec::capture_stdout("ls", ls_args) {
            // Only touch the filesystem again to resolve `.lnk` targets if the listing has
            // one at all — the common case stays a single `ls` and nothing more.
            let targets =
                if output.contains(".lnk") { _lnk_targets(&args.passthrough) } else { HashMap::new() };
            for row in _format_listing(&output, &targets) {
                println!("{row}");
            }
        }
    }

    /// What `lll` accepts beyond the listing style it pins: whatever the user typed, handed to
    /// `ls` untouched. The field carries the help text — a doc comment on the struct documents
    /// the type for readers here, but clap only shows the one on the argument itself.
    #[derive(Args)]
    pub struct LllArgs {
        /// Paths and/or `ls` options, forwarded to `ls` exactly as typed (on top of the flags
        /// `lll` already applies). Any option `ls` accepts works here — `man ls` lists them all;
        /// common ones are `-S` to sort by size, `-R` to recurse, `-X` to group by extension
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "LS_ARG")]
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
    /// Column spacing for these listings — a run of this many spaces both divides the input
    /// columns and separates the output ones (matches `table_formatter`'s default).
    const TABLE_SEPARATOR: usize = 2;

    // ——— Formatting (pure) ————————————————————————————————————————————————

    /// Turn raw `ls -l` output into aligned rows under a bold-blue header, flagging any
    /// `.lnk` entry (see [`_name_cell`]). Pure given `targets`, so it's unit-tested.
    fn _format_listing(ls_output: &str, targets: &HashMap<String, String>) -> Vec<String> {
        let header = _header(&HEADER.join("\t"));
        let rows: Vec<String> = std::iter::once(header)
            .chain(
                ls_output
                    .lines()
                    .filter(|line| !line.starts_with("total ")) // drop `ls -l`'s block-count line
                    .map(|line| _row(line, targets)),
            )
            .collect();
        // The fallback is unreachable: only `sort` can error, and it's off.
        // divide_by / join_with are delimiter strings now, not counts: a run of TABLE_SEPARATOR
        // spaces both splits the input (`\s{2,}|\t+` — our rows are tab-delimited) and spaces the
        // output. Fallback stays unreachable: sort is off and the delimiter is valid.
        let gap = " ".repeat(TABLE_SEPARATOR);
        let opts = table_formatter::FormatOptions {
            divide_by: gap.clone(),
            join_with: gap,
            ..Default::default()
        };
        table_formatter::format_table(&rows, &opts).unwrap_or(rows)
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
    /// name and its target — or `?` when the target can't be read — go red via `recho`'s
    /// style, joined by a plain ` (Windows)-> ` marker so it fits `ls`'s aesthetic and
    /// reads unambiguously. Any other entry keeps `ls`'s own colouring untouched.
    fn _name_cell(name: &str, targets: &HashMap<String, String>) -> String {
        // Cheap reject first: no `.lnk` substring → not a shortcut, so skip the ANSI strip.
        // (`ls` colours a name as a single span, so `.lnk` survives contiguously in it.)
        if !name.contains(".lnk") {
            return name.to_string();
        }
        let plain = console::strip_ansi_codes(name);
        let plain = plain.strip_suffix(|c: char| "*/=>@|".contains(c)).unwrap_or(&plain); // -F indicator
        if !plain.ends_with(".lnk") {
            return name.to_string(); // `.lnk` appeared mid-name, not as the extension
        }
        let red = doc_style::broken_link_text;
        // `?` stands in when the shortcut's target can't be read (unparseable / uncorrelated).
        let target = match targets.get(plain) {
            Some(t) => t.split_whitespace().collect::<Vec<_>>().join(" "),
            None => "?".to_string(),
        };
        format!("{} (Windows)-> {}", red(plain), red(&target))
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

    // --- fs_usage ---------------------------------------------------------------

    /// Column labels for [`usage`] — title case like `lll`'s [`HEADER`], and rendered through the
    /// same bold-blue [`_header`], so the two listings read as one family. Joined with the gap
    /// [`_usage_row`] already puts between its cells, so the label row divides into columns
    /// exactly the way the data rows do.
    const USAGE_HEADER: &[&str] = &["Size", "Files", "Inodes", "Name"];

    /// Disk usage overview: each entry's allocated size, recursive file count, and inode count,
    /// largest first with a total — like `du`, hardlinked twins counted once
    pub fn usage(args: UsageArgs) {
        let UsageArgs { path, count } = args;
        if count {
            let Some(stats) = _tree_stats(&path) else { _missing(&path) };
            println!("{}", stats.files);
            return;
        }
        let Ok(root) = std::fs::canonicalize(&path) else { _missing(&path) };
        let mut seen = HashSet::new();
        let mut rows: Vec<(Stats, String)> = Vec::new();
        if root.is_dir() {
            let mut children: Vec<PathBuf> = match std::fs::read_dir(&root) {
                Ok(entries) => entries.flatten().map(|entry| entry.path()).collect(),
                Err(err) => {
                    eprintln!("fs_usage: cannot read {}: {err}", root.display());
                    std::process::exit(1);
                }
            };
            children.sort(); // deterministic hardlink attribution: first scanned owns the inode
            for child in children {
                let stats = _scan(&child, &mut seen);
                rows.push((stats, _entry_name(&child)));
            }
        } else {
            rows.push((_scan(&root, &mut seen), _entry_name(&root)));
        }
        rows.sort_by(|a, b| b.0.bytes.cmp(&a.0.bytes).then_with(|| a.1.cmp(&b.1)));

        let mut lines: Vec<String> = vec![_header(&USAGE_HEADER.join("  "))];
        lines.extend(rows.iter().map(|(stats, name)| _usage_row(stats, name)));
        if root.is_dir() {
            // The rows plus the root directory's own inode — what `du -s` would report.
            let mut total = std::fs::symlink_metadata(&root)
                .map(|meta| Stats { bytes: meta.blocks() * 512, files: 0, inodes: 1 })
                .unwrap_or_default();
            for (stats, _) in &rows {
                total.bytes += stats.bytes;
                total.files += stats.files;
                total.inodes += stats.inodes;
            }
            lines.push(_usage_row(&total, "(total)"));
        }
        // Default delimiters (2-space split/join) — the old separator:2/threshold:2. Fallback
        // unreachable: sort is off and the defaults are valid.
        let opts = table_formatter::FormatOptions { trim_trailing: true, ..Default::default() };
        for line in table_formatter::format_table(&lines, &opts).unwrap_or(lines) {
            println!("{line}");
        }
    }

    #[derive(Args)]
    pub struct UsageArgs {
        /// Directory (each entry becomes a row) or a single file to size up
        #[arg(default_value = ".")]
        pub path: PathBuf,
        /// Print only the recursive count of regular files, as a bare number (for scripts;
        /// directories and symlinks aren't files, a broken-symlink target counts 0)
        #[arg(long)]
        pub count: bool,
    }

    /// Recursive tallies for one filesystem subtree.
    #[derive(Clone, Copy, Default)]
    struct Stats {
        /// Allocated bytes (`st_blocks` × 512) like `du` — not apparent length.
        bytes: u64,
        /// Regular files only — what "how many files" usually means.
        files: u64,
        /// Every entry: files, directories, symlinks; hardlinked twins once.
        inodes: u64,
    }

    /// The whole tree's [`Stats`] at `path`. The argument itself is followed when it's a symlink
    /// (pointing at it is asking about it — no trailing-slash tricks needed); a broken one counts
    /// as the bare link it is; a missing path is `None`.
    fn _tree_stats(path: &Path) -> Option<Stats> {
        match std::fs::canonicalize(path) {
            Ok(real) => Some(_scan(&real, &mut HashSet::new())),
            Err(_) => std::fs::symlink_metadata(path)
                .ok()
                .map(|meta| Stats { bytes: meta.blocks() * 512, files: 0, inodes: 1 }),
        }
    }

    /// Walk `path`, tallying fresh [`Stats`]: symlinks are counted, never followed (matching
    /// `find`/`du`), and `seen` carries `(device, inode)` pairs so hardlinked twins — including
    /// ones met by an earlier scan sharing the set — count once, as `du` does.
    fn _scan(path: &Path, seen: &mut HashSet<(u64, u64)>) -> Stats {
        let Ok(meta) = std::fs::symlink_metadata(path) else { return Stats::default() };
        let mut stats = Stats::default();
        if seen.insert((meta.dev(), meta.ino())) {
            stats.inodes = 1;
            stats.bytes = meta.blocks() * 512;
            if meta.is_file() {
                stats.files = 1;
            }
        }
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    let sub = _scan(&entry.path(), seen);
                    stats.bytes += sub.bytes;
                    stats.files += sub.files;
                    stats.inodes += sub.inodes;
                }
            }
        }
        stats
    }

    /// One table row: humanized size, the counts, the name.
    fn _usage_row(stats: &Stats, name: &str) -> String {
        format!("{}  {}  {}  {}", _human_size(stats.bytes), stats.files, stats.inodes, name)
    }

    /// An entry's display name: its file name, `/`-marked when it's a real directory.
    fn _entry_name(path: &Path) -> String {
        let mut name =
            path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned();
        if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
            name.push('/');
        }
        name
    }

    /// `du -h`-style size: 1024-based, one decimal under 10, bare integer above.
    fn _human_size(bytes: u64) -> String {
        const UNITS: [&str; 4] = ["K", "M", "G", "T"];
        if bytes < 1024 {
            return format!("{bytes}B");
        }
        let mut value = bytes as f64 / 1024.0;
        let mut unit = 0;
        while value >= 1024.0 && unit + 1 < UNITS.len() {
            value /= 1024.0;
            unit += 1;
        }
        if value < 10.0 {
            format!("{value:.1}{}", UNITS[unit])
        } else {
            format!("{value:.0}{}", UNITS[unit])
        }
    }

    /// Report a nonexistent `path` and exit — the shared failure tail of both `fs_usage` modes.
    fn _missing(path: &Path) -> ! {
        eprintln!("fs_usage: no such path: {}", path.display());
        std::process::exit(1);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn usage_scan_counts_files_and_inodes_and_dedups_hardlinks() {
            let base = std::env::temp_dir().join(format!("bashrs_usage_scan_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(base.join("sub")).unwrap();
            std::fs::write(base.join("a.txt"), "hello").unwrap();
            std::fs::write(base.join("sub/b.txt"), "world!").unwrap();
            std::os::unix::fs::symlink("a.txt", base.join("link")).unwrap();
            std::os::unix::fs::symlink("missing", base.join("broken")).unwrap();
            std::fs::hard_link(base.join("a.txt"), base.join("twin")).unwrap();

            let stats = _tree_stats(&base).expect("the tree exists");
            assert_eq!(stats.files, 2, "regular files only: a.txt + b.txt (twin shares a.txt's inode; symlinks aren't files)");
            assert_eq!(stats.inodes, 6, "base, a.txt, sub, b.txt, link, broken — twin dedups away");
            assert!(stats.bytes > 0, "allocated blocks must register");
            let _ = std::fs::remove_dir_all(&base);
        }

        #[test]
        fn usage_follows_its_argument_but_never_inner_symlinks() {
            let base = std::env::temp_dir().join(format!("bashrs_usage_arg_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(base.join("real")).unwrap();
            std::fs::write(base.join("real/file.txt"), "data").unwrap();
            std::os::unix::fs::symlink(base.join("real"), base.join("door")).unwrap();
            std::os::unix::fs::symlink("nowhere", base.join("dangling")).unwrap();

            // The symlink argument is followed — its target's contents are the answer …
            assert_eq!(_tree_stats(&base.join("door")).unwrap().files, 1);
            // … while the same link *inside* a scanned tree stays an unfollowed entry, so the
            // target's file is not double-counted through it.
            assert_eq!(_tree_stats(&base).unwrap().files, 1);
            // A broken symlink argument is the bare link it is; a missing path is an error.
            let broken = _tree_stats(&base.join("dangling")).unwrap();
            assert_eq!((broken.files, broken.inodes), (0, 1));
            assert!(_tree_stats(&base.join("no_such_thing")).is_none());
            let _ = std::fs::remove_dir_all(&base);
        }

        #[test]
        fn usage_sizes_read_like_du() {
            assert_eq!(_human_size(0), "0B");
            assert_eq!(_human_size(512), "512B");
            assert_eq!(_human_size(2048), "2.0K");
            assert_eq!(_human_size(10 * 1024 * 1024), "10M");
            assert_eq!(_human_size(3 * 1024 * 1024 * 1024 / 2), "1.5G");
        }

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
        fn name_cell_flags_a_lnk_with_a_question_mark_when_unresolved() {
            let cell = _name_cell("mystery.lnk", &HashMap::new());
            assert!(cell.contains("(Windows)-> "), "marker missing: {cell}");
            assert!(cell.contains('?'), "unresolved target should show `?`: {cell}");
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
