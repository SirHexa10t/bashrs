//! Filesystem commands (`fs_*`) and navigation helpers: `lll`, a long `ls` listing aligned into a
//! table via `table_formatter` under a bold-blue header from the style engine; `fs_usage`, what
//! each entry costs on disk when measured recursively; and `LLL`, the two joined into one table
//! ordered by that recursive cost.
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

    /// `lll` and `fs_usage` in one table: what each entry costs on disk — recursive size, file
    /// count, inode count — joined onto its listing row, largest first
    ///
    /// **Shape.** [`usage`]'s figures lead, minus its own `Name`, which `lll` already supplies.
    /// `lll`'s `Size` drops out in turn ([`SIZE_COL`]): the recursive size beside it says strictly
    /// more. The filename stays last, where [`_row`] needs it — it is the one field that may
    /// contain anything at all.
    ///
    /// **The join rests entirely on `--zero`.** Names arrive literal and exact (see [`LS_FLAGS`]),
    /// so every row keys straight onto the entry [`_scan`] measured: nothing to un-escape, nothing
    /// to guess at. Quoting happens afterwards and for display only ([`_display_name`]), never to
    /// the key — which is why a file whose name holds a newline still gets measured.
    ///
    /// **What it refuses, and why** — [`_measured_root`]: an ordering flag it would silently
    /// overrule, a recursive listing whose repeated names it cannot attribute, or several paths
    /// whose entries could collide. `.` and `..` are listed but never walked ([`_entry_stats`]).
    ///
    /// **Cost.** Unlike `lll`, this stats every file beneath every entry — [`usage`]'s price, paid
    /// once per entry instead of once in total. On a large tree that is seconds, not milliseconds.
    #[name("LLL")]
    #[trailing_newline]
    pub fn lll_usage(args: LllArgs) {
        let root = _measured_root(&args.passthrough);
        let mut ls_args: Vec<&str> = LS_FLAGS.to_vec();
        ls_args.extend(args.passthrough.iter().map(String::as_str));
        let Some(output) = exec::capture_stdout("ls", ls_args) else { return };
        let stats = _entry_stats(&root, &_entry_keys(&output));
        let targets =
            if output.contains(".lnk") { _lnk_targets(&args.passthrough) } else { HashMap::new() };
        for row in _usage_listing(&output, &stats, &targets) {
            println!("{row}");
        }
    }

    /// The single directory `LLL` measures, or a refusal explaining why it cannot.
    ///
    /// Every rejection here is a case where `LLL` would otherwise appear to obey and quietly not:
    /// an ordering flag it overrules, a recursive listing whose names repeat across sections with
    /// nothing to attribute them to, or several paths whose entries share names.
    fn _measured_root(passthrough: &[String]) -> PathBuf {
        let refuse = |reason: String| -> ! {
            eprintln!("LLL: {reason}");
            std::process::exit(2);
        };
        if let Some(flag) = passthrough.iter().find(|arg| _is_sort_flag(arg)) {
            refuse(format!(
                "`{flag}` chooses an order, but LLL always sorts by recursive size — drop it, or use `lll` for that ordering"
            ));
        }
        if let Some(flag) = passthrough.iter().find(|arg| _is_recursive_flag(arg)) {
            refuse(format!("`{flag}` lists each subdirectory separately, repeating names with nothing to tell them apart — LLL measures one directory"));
        }
        let paths: Vec<&str> =
            passthrough.iter().map(String::as_str).filter(|arg| !arg.starts_with('-')).collect();
        let root = match paths.as_slice() {
            [] => PathBuf::from("."),
            [one] => PathBuf::from(one),
            _ => refuse("one directory at a time — entries from several could share a name".into()),
        };
        if !root.is_dir() {
            refuse(format!("{} is not a directory — LLL measures a directory's entries", root.display()));
        }
        root
    }

    /// `ls` options that choose an order. `LLL` fixes the order itself, so one of these would be
    /// accepted and then silently overruled. `-r` counts too: reversing an order `LLL` replaces
    /// means nothing.
    const SORT_FLAGS: &str = "SXUtvr";

    /// Whether `arg` asks `ls` to order the listing — either long form, or a short cluster
    /// carrying one of [`SORT_FLAGS`] (`-lS` as much as `-S`).
    fn _is_sort_flag(arg: &str) -> bool {
        arg == "--sort"
            || arg.starts_with("--sort=")
            || arg == "--reverse"
            || (arg.len() > 1
                && arg.starts_with('-')
                && !arg.starts_with("--")
                && arg.chars().skip(1).any(|ch| SORT_FLAGS.contains(ch)))
    }

    /// The plain on-disk name each listing record refers to: colour stripped, `ls -F`'s type
    /// marker removed, and a symlink's ` -> target` dropped. This is the join key, and `--zero`
    /// is what makes it exact — the bytes here are the bytes in the directory entry.
    ///
    /// The marker is only removed when the mode says one was added, so a regular file genuinely
    /// named `report@` keeps its `@`.
    fn _entry_key(meta: &[&str], name: &str) -> String {
        let mode = meta.get(1).copied().unwrap_or_default();
        let head = match name.split_once(" -> ").filter(|_| mode.starts_with('l')) {
            Some((link, _)) => link,
            None => name,
        };
        let plain = console::strip_ansi_codes(head);
        let marked = matches!(mode.as_bytes().first(), Some(b'd' | b'l' | b'p' | b's'))
            || mode.contains('x');
        match marked {
            true => plain.strip_suffix(|ch: char| TYPE_MARKERS.contains(ch)).unwrap_or(&plain).to_string(),
            false => plain.into_owned(),
        }
    }

    /// Every entry named by the listing, in listing order.
    fn _entry_keys(ls_output: &str) -> Vec<String> {
        _records(ls_output)
            .map(|record| {
                let (meta, name) = _split_record(record);
                _entry_key(&meta, name)
            })
            .collect()
    }

    /// The data records of a listing: NUL-separated, minus the trailing empty one and `ls -l`'s
    /// block-count line.
    fn _records(ls_output: &str) -> impl Iterator<Item = &str> {
        ls_output.split('\0').filter(|record| !record.is_empty() && !record.starts_with("total "))
    }

    /// Measure every listed entry under `root`.
    ///
    /// `.` and `..` are deliberately never scanned: `lll` passes `-a`, so both are listed, and
    /// walking `..` would measure the entire parent tree — on a home directory, most of the disk.
    /// Scanning runs in name order so hardlink attribution is deterministic (the first entry to
    /// claim an inode keeps it), exactly as [`usage`] does it.
    fn _entry_stats(root: &Path, keys: &[String]) -> HashMap<String, Stats> {
        let mut ordered: Vec<&String> =
            keys.iter().filter(|key| key.as_str() != "." && key.as_str() != "..").collect();
        ordered.sort();
        ordered.dedup();
        let mut seen = HashSet::new();
        ordered
            .into_iter()
            .map(|key| (key.clone(), _scan(&root.join(key), &mut seen)))
            .collect()
    }

    /// `LLL`'s column labels: [`usage`]'s figures without its `Name`, then `lll`'s without its
    /// `Size`. Derived from both rather than restated, so neither can drift away from it.
    fn _lll_header() -> Vec<&'static str> {
        USAGE_HEADER
            .iter()
            .copied()
            .filter(|label| *label != "Name")
            .chain(HEADER.iter().enumerate().filter(|(i, _)| *i != SIZE_COL).map(|(_, label)| *label))
            .collect()
    }

    /// Index of `lll`'s own `Size` within a record's metadata — the column `LLL` drops, because
    /// the recursive size beside it says strictly more.
    const SIZE_COL: usize = 5;

    /// Build the joined table: each `ls` row prefixed with what its entry measures, ordered
    /// largest first. Pure given `stats` and `targets`, so it is unit-tested.
    ///
    /// An unmeasured entry (`.`, `..`, or anything that vanished between listing and scanning)
    /// shows `-` for its figures and sorts below everything measured — including a genuinely
    /// empty file, which has a real size of zero and belongs above "no answer". `None` ordering
    /// gives that for free: it compares below every `Some`.
    fn _usage_listing(
        ls_output: &str,
        stats: &HashMap<String, Stats>,
        targets: &HashMap<String, String>,
    ) -> Vec<String> {
        let mut rows: Vec<(Option<u64>, String, String)> = Vec::new();
        for record in _records(ls_output) {
            let (meta, name) = _split_record(record);
            if meta.is_empty() || name.is_empty() {
                continue;
            }
            let key = _entry_key(&meta, name);
            let measured = stats.get(&key);
            let figures = measured.map_or_else(
                || ["-".to_string(), "-".to_string(), "-".to_string()],
                |s| [_human_size(s.bytes), s.files.to_string(), s.inodes.to_string()],
            );
            let kept: Vec<&str> =
                meta.iter().enumerate().filter(|(i, _)| *i != SIZE_COL).map(|(_, f)| *f).collect();
            let row = format!(
                "{}\t{}\t{}",
                figures.join("\t"),
                kept.join("\t"),
                _link_cell(&meta, name, targets)
            );
            rows.push((measured.map(|s| s.bytes), key, row));
        }
        rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let lines: Vec<String> = std::iter::once(_header(&_lll_header().join("\t")))
            .chain(rows.into_iter().map(|(_, _, row)| row))
            .collect();
        let gap = " ".repeat(TABLE_SEPARATOR);
        let opts = table_formatter::FormatOptions {
            divide_by: gap.clone(),
            join_with: gap,
            ..Default::default()
        };
        table_formatter::format_table(&lines, &opts).unwrap_or(lines)
    }

    /// The `ls` flags that define an `lll` listing: long, all, mtime-sorted (reversed), block +
    /// human sizes, type indicators, coloured, directories first — and `--zero`, which ends each
    /// entry with a NUL instead of a newline.
    ///
    /// `--zero` is load-bearing, not cosmetic. A filename may contain any byte except `/` and NUL,
    /// newlines included, so a newline-delimited listing splits such an entry across two rows and
    /// the tail is then parsed as a whole separate file. NUL is the one byte that cannot occur in
    /// a name, so it is the only safe record separator. It also forces literal names, which keeps
    /// each name usable as an identity — nothing to un-escape and guess at — where `-b` would
    /// backslash every space and, outside a UTF-8 locale, octal-escape every accented character.
    const LS_FLAGS: &[&str] =
        &["-tarlushF", "--zero", "--color=always", "--time-style=+%F_%T", "--group-directories-first"];
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
    ///
    /// Records are NUL-separated, per [`LS_FLAGS`]'s `--zero`; the trailing NUL leaves an empty
    /// final record, which drops out with the block-count line.
    fn _format_listing(ls_output: &str, targets: &HashMap<String, String>) -> Vec<String> {
        let header = _header(&HEADER.join("\t"));
        let rows: Vec<String> = std::iter::once(header)
            .chain(
                _records(ls_output).map(|record| _row(record, targets)),
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

    /// One listing row: the [`META_COLS`] leading metadata fields tab-delimited, then the name as
    /// a single trailing column (see [`_name_cell`]).
    ///
    /// The name is the exact remainder of the record, never re-split and rejoined: it can hold
    /// spaces, tabs and newlines, and a symlink's ` -> target` rides in the same field. Only the
    /// metadata is consumed field by field, and only on spaces — `ls` pads those columns with
    /// spaces, so a tab that belongs to the name is never mistaken for a delimiter.
    fn _row(record: &str, targets: &HashMap<String, String>) -> String {
        if record.is_empty() {
            return String::new();
        }
        let (meta, name) = _split_record(record);
        if meta.is_empty() {
            return _display_name(name); // a fragment, not a listing row
        }
        if name.is_empty() {
            return meta.join("\t");
        }
        format!("{}\t{}", meta.join("\t"), _link_cell(&meta, name, targets))
    }

    /// A record's [`META_COLS`] metadata fields and the exact name remainder. Split only on
    /// spaces, and only for the metadata: `ls` pads those columns with spaces, so a tab belonging
    /// to the name is never taken for a delimiter, and the name is handed back whole.
    fn _split_record(record: &str) -> (Vec<&str>, &str) {
        let mut rest = record;
        let mut meta: Vec<&str> = Vec::with_capacity(META_COLS);
        for _ in 0..META_COLS {
            rest = rest.trim_start_matches(' ');
            let Some(end) = rest.find(' ') else { break };
            meta.push(&rest[..end]);
            rest = &rest[end..];
        }
        (meta, rest.trim_start_matches(' '))
    }

    /// The name column, aware that a symlink's field is *two* paths and an arrow rather than one
    /// name — so each side is spelled separately (`'a link' -> 'real target.txt'`), the way `ls`'s
    /// own shell quoting does it. Quoting the pair as a single word would claim a file exists
    /// under that whole string, which is a lie.
    ///
    /// The permissions column decides: only an `l` mode makes the arrow a separator, so a plain
    /// file merely *named* `evil -> fake.txt` stays whole. When a symlink's own name also contains
    /// ` -> ` the split is genuinely ambiguous — `ls` gives us no delimiter to trust — and the
    /// first arrow wins.
    fn _link_cell(meta: &[&str], name: &str, targets: &HashMap<String, String>) -> String {
        let is_symlink = meta.get(1).is_some_and(|mode| mode.starts_with('l'));
        match name.split_once(" -> ").filter(|_| is_symlink) {
            Some((link, target)) => {
                format!("{} -> {}", _name_cell(link, targets), _display_name(target))
            }
            None => _name_cell(name, targets),
        }
    }

    /// Characters a name may hold and still be printed bare: bash reads every one of them back
    /// literally. Anything else — a space, a glob, a quote, a redirection, a control character —
    /// means the name has to be spelled out to be unambiguous.
    const BARE_SAFE: &str =
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-+,:@%";

    /// `ls -F`'s type markers, appended after the name (and after its colour reset).
    const TYPE_MARKERS: &str = "*/=>@|";

    /// How a name has to be spelled for bash to read back exactly the bytes on disk.
    #[derive(PartialEq, Eq, Debug)]
    enum _Quoting {
        /// Only [`BARE_SAFE`] characters — print it as it is.
        Bare,
        /// `'…'`: everything inside is literal, which covers spaces, globs, `$`, `"` and `\`.
        Single,
        /// `$'…'`: the only form that can express a control character, so it also takes names
        /// holding a `'` (which `'…'` cannot escape) and space runs (which the column divider
        /// would otherwise cut the name at).
        Ansi,
    }

    /// Which spelling `plain` (already stripped of colour and of its type marker) needs.
    fn _quoting_for(plain: &str) -> _Quoting {
        if plain.chars().any(char::is_control) || plain.contains('\'') || plain.contains("  ") {
            return _Quoting::Ansi;
        }
        if plain.is_empty() || plain.chars().any(|ch| !BARE_SAFE.contains(ch)) {
            return _Quoting::Single;
        }
        _Quoting::Bare
    }

    /// Render `name` so that it is unambiguous, safe in a table cell, and still a valid bash word
    /// — paste it into a command and you get the file you were looking at.
    ///
    /// Marking only appears where it carries meaning: an ordinary name prints untouched. When it
    /// is needed, the form itself distinguishes cases the eye cannot. A file whose name really
    /// contains a newline prints `$'aa\nbb.txt'`; one literally called `aa\nbb.txt` prints
    /// `'aa\nbb.txt'` — the `$` is the difference, exactly as bash reads them.
    ///
    /// The quotes also solve a layout problem: a run of two or more spaces is this table's column
    /// divider ([`TABLE_SEPARATOR`]), so a name containing one would be cut in half. In `$'…'`
    /// those spaces become `\x20` and the run disappears without the name losing a character.
    ///
    /// `ls`'s colour sequences ride through untouched — they are control characters too, and
    /// escaping them would print the codes instead of colouring the name. The type marker `ls -F`
    /// appends stays outside the quotes, where it belongs: `'a dir'/`, not `'a dir/'`.
    fn _display_name(name: &str) -> String {
        let (body, marker) = match name.strip_suffix(|ch: char| TYPE_MARKERS.contains(ch)) {
            Some(body) => (body, &name[body.len()..]),
            None => (name, ""),
        };
        let plain = console::strip_ansi_codes(body);
        let escape_spaces = plain.contains("  ");
        match _quoting_for(&plain) {
            _Quoting::Bare => name.to_string(),
            _Quoting::Single => format!("'{body}'{marker}"),
            _Quoting::Ansi => {
                let inner = _map_outside_ansi(body, |ch, out| match ch {
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    '\r' => out.push_str("\\r"),
                    '\\' => out.push_str("\\\\"),
                    '\'' => out.push_str("\\'"),
                    ' ' if escape_spaces => out.push_str("\\x20"),
                    other if other.is_control() => {
                        out.push_str(&format!("\\x{:02x}", other as u32));
                    }
                    other => out.push(other),
                });
                format!("$'{inner}'{marker}")
            }
        }
    }

    /// Copy `text`, handing every ordinary character to `escape` while letting ANSI colour
    /// sequences through verbatim. A sequence runs from `ESC` to its final letter (`m`, for the
    /// colour codes `ls` emits), and nothing inside it is a filename character.
    fn _map_outside_ansi(text: &str, mut escape: impl FnMut(char, &mut String)) -> String {
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch != '\u{1b}' {
                escape(ch, &mut out);
                continue;
            }
            out.push(ch);
            for code in chars.by_ref() {
                out.push(code);
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        }
        out
    }

    /// Render the name column. A `.lnk` shortcut is treated as a broken Windows link: the
    /// name and its target — or `?` when the target can't be read — go red via `recho`'s
    /// style, joined by a plain ` (Windows)-> ` marker so it fits `ls`'s aesthetic and
    /// reads unambiguously. Any other entry keeps `ls`'s own colouring untouched.
    fn _name_cell(name: &str, targets: &HashMap<String, String>) -> String {
        // Takes the name as `ls` gave it, before [`_display_name`] can wrap it in quotes — a
        // quoted `'shortcut.lnk'` would no longer end in `.lnk` and the check below would miss it.
        // Cheap reject first: no `.lnk` substring → not a shortcut, so skip the ANSI strip.
        // (`ls` colours a name as a single span, so `.lnk` survives contiguously in it.)
        if !name.contains(".lnk") {
            return _display_name(name);
        }
        let plain = console::strip_ansi_codes(name);
        let plain = plain.strip_suffix(|c: char| TYPE_MARKERS.contains(c)).unwrap_or(&plain); // -F indicator
        if !plain.ends_with(".lnk") {
            return _display_name(name); // `.lnk` appeared mid-name, not as the extension
        }
        let red = doc_style::broken_link_text;
        // `?` stands in when the shortcut's target can't be read (unparseable / uncorrelated).
        let target = match targets.get(plain) {
            Some(t) => t.split_whitespace().collect::<Vec<_>>().join(" "),
            None => "?".to_string(),
        };
        format!("{} (Windows)-> {}", red(&_display_name(plain)), red(&target))
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

        /// A symlink row, whose permissions column is what marks the arrow as a separator.
        fn link_row(name: &str) -> String {
            format!("4 lrwxrwxrwx 1 user group 1.2K 2024-01-01_12:00:00 {name}")
        }

        #[test]
        fn row_spells_a_spaced_name_as_one_shell_word() {
            let out = _row(&row("my report final.txt"), &HashMap::new());
            assert!(out.ends_with("\t'my report final.txt'"), "{out}");
        }

        #[test]
        fn row_spells_each_side_of_a_symlink_separately() {
            // Two paths, not one name — quoting the pair whole would claim a file exists under
            // the entire `a link -> real target.txt` string.
            let out = _row(&link_row("a link -> real target.txt"), &HashMap::new());
            assert!(out.ends_with("\t'a link' -> 'real target.txt'"), "{out}");
        }

        #[test]
        fn row_leaves_an_arrow_in_an_ordinary_name_whole() {
            // Same text, but the mode says regular file: it is one name that merely looks like a
            // link, and splitting it would invent a target that does not exist.
            let out = _row(&row("evil -> fake.txt"), &HashMap::new());
            assert!(out.ends_with("\t'evil -> fake.txt'"), "{out}");
        }

        #[test]
        fn row_spells_out_control_characters_and_space_runs() {
            // A run of two spaces is this table's column divider, so it cannot stand literally.
            // `$'…'` removes the run without the name losing a single byte — where the old
            // behaviour quietly collapsed it and reported a filename that did not exist.
            let runs = _row(&row("two  spaces"), &HashMap::new());
            assert!(runs.ends_with("\t$'two\\x20\\x20spaces'"), "{runs}");
            let tabs = _row("4 -rw-r--r-- 1 u g 1K date a\tb\tc", &HashMap::new());
            assert!(tabs.ends_with("\t$'a\\tb\\tc'"), "a tab is escaped, not turned into a space: {tabs}");
        }

        /// The whole point of the `$` prefix: `$'…'` means bash *interprets* the escape, `'…'`
        /// means it does not. Without that distinction a file containing a newline and a file
        /// literally named `aa\nbb.txt` would print identically, and neither could be acted on.
        #[test]
        fn a_real_newline_and_a_literal_backslash_n_are_told_apart() {
            let real = _row(&row("aa\nbb.txt"), &HashMap::new());
            let literal = _row(&row("aa\\nbb.txt"), &HashMap::new());
            assert!(real.ends_with("\t$'aa\\nbb.txt'"), "a real newline: {real}");
            assert!(literal.ends_with("\t'aa\\nbb.txt'"), "a literal backslash-n: {literal}");
            assert_ne!(real, literal, "the two must never render the same");
        }

        /// Quotes in a name are the case single-quoting cannot express, so `'` forces `$'…'`
        /// (where it escapes as `\'`) while `"` is content like any other inside `'…'`.
        #[test]
        fn quotes_in_names_are_spelled_so_bash_reads_them_back() {
            let single = _row(&row("quo'te.txt"), &HashMap::new());
            assert!(single.ends_with("\t$'quo\\'te.txt'"), "{single}");
            let double = _row(&row("dou\"ble.txt"), &HashMap::new());
            assert!(double.ends_with("\t'dou\"ble.txt'"), "{double}");
            let both = _row(&row("both'and\".txt"), &HashMap::new());
            assert!(both.ends_with("\t$'both\\'and\".txt'"), "{both}");
        }

        /// Marking is only for names that need it — an ordinary one must stay bare, or every
        /// listing turns into a wall of quotes.
        #[test]
        fn ordinary_names_are_left_unquoted() {
            assert!(_row(&row("plain.txt"), &HashMap::new()).ends_with("\tplain.txt"));
            assert!(_row(&row("dotted.name-v2_final.tar.gz"), &HashMap::new()).ends_with("\tdotted.name-v2_final.tar.gz"));
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
            let out = _format_listing(&format!("total 0\0{}\0", row("go.lnk")), &one_target("go.lnk", "C:\\tools\\go.exe"));
            assert_eq!(out.len(), 2, "header + one row");
            assert!(out[1].contains("(Windows)->") && out[1].contains("C:\\tools\\go.exe"), "row: {}", out[1]);
        }

        #[test]
        fn format_listing_leads_with_a_header() {
            let out = _format_listing(&format!("total 8\0{}\0{}\0", row("a.txt"), row("b.txt")), &HashMap::new());
            assert_eq!(out.len(), 3, "header + two files");
            assert!(out[0].contains("Blocks") && out[0].contains("Filename"), "first row is the header");
        }

        /// A filename may contain any byte but `/` and NUL. Each of these once corrupted the
        /// listing: a newline split one entry into two rows (the tail parsed as its own file), a
        /// tab was collapsed into a space, and every name was re-joined rather than kept whole.
        /// One row per entry, whatever the name, is the contract.
        #[test]
        fn hostile_filenames_each_stay_one_row() {
            let names =
                ["two\nlines.txt", "tab\there.txt", "spaces in name.txt", "trailing space .txt"];
            let listing =
                format!("total 0\0{}\0", names.iter().map(|n| row(n)).collect::<Vec<_>>().join("\0"));
            let out = _format_listing(&listing, &HashMap::new());

            assert_eq!(out.len(), names.len() + 1, "header + exactly one row per entry: {out:#?}");
            assert!(
                out.iter().all(|row| !row.contains('\n')),
                "a raw newline anywhere would break the table apart again: {out:#?}"
            );
            assert!(out[1].contains("two\\nlines.txt"), "newline shown, not obeyed: {}", out[1]);
            assert!(out[2].contains("tab\\there.txt"), "tab shown, not collapsed: {}", out[2]);
            assert!(out[3].contains("spaces in name.txt"), "spaces kept verbatim: {}", out[3]);
        }

        /// The name field also carries a symlink's ` -> target`, and a file can legitimately be
        /// named with that same arrow. Both must survive whole — the arrow is `ls`'s to interpret
        /// (the mode column says which is which), never something to re-split on here.
        #[test]
        fn arrows_in_names_and_symlink_targets_both_survive() {
            let listing = format!("total 0\0{}\0{}\0", row("evil -> fake.txt"), row("a link -> real target.txt"));
            let out = _format_listing(&listing, &HashMap::new());
            assert_eq!(out.len(), 3, "header + two entries: {out:#?}");
            assert!(out[1].contains("evil -> fake.txt"), "a name that looks like a link: {}", out[1]);
            assert!(out[2].contains("a link -> real target.txt"), "an actual link: {}", out[2]);
        }

        // ——— LLL: the joined listing ——————————————————————————————————————

        fn stats(bytes: u64, files: u64, inodes: u64) -> Stats {
            Stats { bytes, files, inodes }
        }

        /// An `LLL` table from raw records plus what each entry measured, colour stripped so the
        /// assertions read plainly.
        fn joined(records: &[String], measured: &[(&str, Stats)]) -> Vec<String> {
            let map: HashMap<String, Stats> =
                measured.iter().map(|(name, s)| ((*name).to_string(), *s)).collect();
            _usage_listing(&format!("total 0\0{}\0", records.join("\0")), &map, &HashMap::new())
                .into_iter()
                .map(|line| console::strip_ansi_codes(&line).into_owned())
                .collect()
        }

        /// The header is derived from both sources rather than restated, and this pins what that
        /// derivation must produce: `fs_usage`'s figures without its `Name`, `lll`'s columns
        /// without its `Size`, and the filename last — where [`_row`] needs it.
        #[test]
        fn lll_header_drops_the_two_redundant_columns() {
            let header = _lll_header();
            assert_eq!(
                header,
                ["Size", "Files", "Inodes", "Blocks", "Permissions", "Hard-Links", "Owner", "Group", "Date_Modified", "Filename"]
            );
            assert_eq!(HEADER[SIZE_COL], "Size", "SIZE_COL must still point at the column being dropped");
            assert_eq!(header.len(), USAGE_HEADER.len() - 1 + HEADER.len() - 1);
        }

        #[test]
        fn lll_orders_by_recursive_size_largest_first() {
            let out = joined(
                &[row("small.bin"), row("huge.bin"), row("middling.bin")],
                &[
                    ("small.bin", stats(1024, 1, 1)),
                    ("huge.bin", stats(9_000_000, 40, 44)),
                    ("middling.bin", stats(50_000, 3, 3)),
                ],
            );
            let names: Vec<&str> = out[1..].iter().map(|r| r.split_whitespace().last().unwrap()).collect();
            assert_eq!(names, ["huge.bin", "middling.bin", "small.bin"], "{out:#?}");
            assert!(out[1].trim_start().starts_with("8.6M"), "the recursive size leads the row: {}", out[1]);
        }

        /// `.` and `..` are listed but must never be walked — measuring `..` would climb into the
        /// parent tree. They carry no figures, and sit below even an empty file, which has a real
        /// answer of zero rather than no answer at all.
        #[test]
        fn unmeasured_entries_show_a_dash_and_sink_to_the_bottom() {
            let out = joined(
                &[row("."), row(".."), row("empty.txt"), row("full.bin")],
                &[("empty.txt", stats(0, 1, 1)), ("full.bin", stats(4096, 1, 1))],
            );
            let names: Vec<&str> = out[1..].iter().map(|r| r.split_whitespace().last().unwrap()).collect();
            assert_eq!(names, ["full.bin", "empty.txt", ".", ".."], "{out:#?}");
            assert!(out[3].trim_start().starts_with('-'), "an unmeasured entry reports no figures: {}", out[3]);
            assert!(out[2].trim_start().starts_with("0B"), "an empty file reports a real zero: {}", out[2]);
        }

        #[test]
        fn lll_keeps_lll_columns_but_not_its_size() {
            // The record's own size (`1.2K`, per `row`) must not appear; the recursive one does.
            let out = joined(&[row("f.bin")], &[("f.bin", stats(2048, 1, 1))]);
            assert!(out[1].trim_start().starts_with("2.0K"), "{}", out[1]);
            assert!(!out[1].contains("1.2K"), "lll's own Size column is dropped: {}", out[1]);
            assert!(out[1].contains("-rw-r--r--") && out[1].contains("2024-01-01_12:00:00"), "{}", out[1]);
        }

        /// The join key is the name on disk — not what is displayed. Colour, `ls -F`'s marker and
        /// a symlink's target all have to come off, or the entry never matches what was scanned.
        #[test]
        fn entry_key_recovers_the_name_on_disk() {
            assert_eq!(_entry_key(&["4", "drwxr-xr-x"], "\u{1b}[01;34ma dir\u{1b}[0m/"), "a dir");
            assert_eq!(_entry_key(&["4", "lrwxrwxrwx"], "a link -> real target.txt"), "a link");
            assert_eq!(_entry_key(&["4", "-rw-r--r--"], "evil -> fake.txt"), "evil -> fake.txt");
            assert_eq!(_entry_key(&["4", "-rw-r--r--"], "plain.txt"), "plain.txt");
            // A marker is only stripped when the mode says one was appended.
            assert_eq!(_entry_key(&["4", "-rw-r--r--"], "report@"), "report@");
            assert_eq!(_entry_key(&["4", "-rwxr-xr-x"], "run.sh*"), "run.sh");
        }

        #[test]
        fn sort_flags_are_recognised_wherever_they_hide() {
            for flag in ["-S", "-t", "-lS", "--sort=size", "--sort", "--reverse", "-r", "-lXh"] {
                assert!(_is_sort_flag(flag), "`{flag}` orders the listing and must be refused");
            }
            for flag in ["-l", "-la", "--human-readable", "--color=always", "-h", "notes.txt"] {
                assert!(!_is_sort_flag(flag), "`{flag}` does not choose an order");
            }
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
