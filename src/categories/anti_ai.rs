//! Commands for dealing with AI-generated content on your own machine (`anti_ai_*`).
//!
//! `anti_ai_textual_watermark_detect` is the first: it finds *hidden characters* — the invisible
//! codepoints that carry edit-based text watermarks, and the same ones that get pasted in by
//! accident and then break diffs, greps and search.
//!
//! Deliberately **detect only**. Reporting where something is is a different act from altering
//! a file, and only the first is safe to run over a whole tree by default. What the output is
//! good for is deciding — the codepoints are printed by name so a reader can judge whether a
//! given one is a carrier, an emoji joiner, or ordinary orthography.
//!
//! Scope, which the `txt_` in the name is there to state: this is the *edit-based text* class
//! only, the one that is verifiable by looking. Statistical (token-sampling) watermarks live in word choice with
//! nothing to scan for, and provenance metadata lives in container headers rather than text —
//! neither is visible to a character scan, and neither is claimed here.

#[bashrs_macros::category(command = AntiAiCommand, prefix = "anti_ai_")]
mod commands {
    use crate::support::ai_meta;
    use crate::support::doc_style::{approved, notice, problematic};
    use clap::Args;
    use ignore::WalkBuilder;
    use std::io::{self, BufWriter, Write};
    use std::path::{Path, PathBuf};

    /// Find verifiable AI marks in a file, or in every file under a directory: hidden
    /// characters in text (zero-width joiners, bidi controls, tag chars, variation selectors —
    /// with the line and codepoint of each), and AI provenance in metadata (frontmatter and
    /// `<meta>` generator fields, XMP CreatorTool, C2PA containers, vendor names) for
    /// md/html/svg/png/jpeg. Media data itself is never decoded; changes nothing
    pub fn textual_watermark_detect(args: WatermarkDetectArgs) {
        _detect(args);
    }

    /// Both scans, one walk.
    fn _detect(args: WatermarkDetectArgs) {
        let WatermarkDetectArgs { target, skip, skip_spaces, emoji, skip_metadata, max_file_size } =
            args;
        let skips = skip.skips();
        if !target.exists() {
            eprintln!("anti_ai_textual_watermark_detect: no such path: {}", target.display());
            std::process::exit(1);
        }
        // One locked, buffered writer for the whole report: a per-line `println!` would take
        // the stdout lock and syscall for every hit, and a tree scan can produce thousands.
        let mut out = BufWriter::new(io::stdout().lock());
        let mut files_with_hits = 0_usize;
        let mut total_hidden = 0_usize;
        let mut total_meta = 0_usize;

        for path in _files(&target, &skips) {
            let hits = _read_text(&path, max_file_size)
                .map(|text| _scan(&text, !skip_spaces, emoji))
                .unwrap_or_default();
            // Metadata is a second look at the same file, gated by whether the engine has an
            // extractor at all — no point reading a .rs file's bytes twice to learn nothing.
            let findings = if !skip_metadata && ai_meta::handles(&path) {
                _read_capped(&path, max_file_size)
                    .and_then(|bytes| ai_meta::extract(&path, &bytes, true))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if hits.is_empty() && findings.is_empty() {
                continue;
            }
            files_with_hits += 1;
            total_hidden += hits.iter().map(|line| line.chars.len()).sum::<usize>();
            total_meta += findings.len();
            let _ = writeln!(out, "\n{}", notice(&path.display().to_string()));
            let _ = _report_file(&mut out, &hits);
            let _ = _report_metadata(&mut out, &findings);
        }

        // Named on both paths: "nothing found" means something different when the walk was
        // allowed to skip files, and the reader cannot tell from the output otherwise.
        let scope = _scope_note(&skip);
        let _ = if files_with_hits == 0 {
            writeln!(out, "{}{scope}", approved("no hidden characters or AI metadata found"))
        } else {
            writeln!(
                out,
                "\n{}",
                problematic(&format!(
                    "{total_hidden} hidden character(s), {total_meta} metadata finding(s) \
                     across {files_with_hits} file(s){scope}"
                ))
            )
        };
        let _ = out.flush();
        if files_with_hits > 0 {
            // Non-zero so `anti_ai_textual_watermark_detect && publish` refuses to chain past
            // a find of either kind.
            std::process::exit(1);
        }
    }

    /// A file's raw bytes, size-capped the same way the textual scan is. No text sniff here:
    /// the metadata extractor wants binary formats too, and does its own routing.
    fn _read_capped(path: &Path, max_bytes: u64) -> Option<Vec<u8>> {
        let size = std::fs::metadata(path).ok()?.len();
        if size > max_bytes {
            eprintln!(
                "anti_ai_textual_watermark_detect: skipping {} ({} bytes > --max-file-size {})",
                path.display(),
                size,
                max_bytes
            );
            return None;
        }
        std::fs::read(path).ok()
    }

    /// One file's metadata findings: the field, the (display-truncated) value, and why it
    /// made the report.
    fn _report_metadata(out: &mut impl Write, findings: &[ai_meta::Finding]) -> io::Result<()> {
        for finding in findings {
            // Matches and provenance fields are tagged; the rest of the inventory is plain —
            // an untagged line means exactly "metadata, listed by default, nothing matched".
            let why = match &finding.why {
                ai_meta::Why::Marker(marker) => format!("  {}", problematic(&format!("[marker: {marker}]"))),
                ai_meta::Why::ProvenanceField => format!("  {}", notice("[provenance field]")),
                ai_meta::Why::Everything => String::new(),
            };
            writeln!(out, "  {}: {}{why}", finding.field, _display_value(&finding.value))?;
        }
        Ok(())
    }

    /// Values fit on a report line: XMP packets run to kilobytes, and past a screenful the
    /// reader has the field name and the file to go look.
    fn _display_value(value: &str) -> String {
        const LIMIT: usize = 160;
        if value.chars().count() <= LIMIT {
            value.to_string()
        } else {
            let cut: String = value.chars().take(LIMIT).collect();
            format!("{cut}… ({} chars)", value.chars().count())
        }
    }

    #[derive(Args)]
    pub struct WatermarkDetectArgs {
        /// File or directory to scan; a directory is walked recursively
        #[arg(default_value = ".")]
        pub target: PathBuf,
        /// Paths to leave out of the walk.
        #[command(flatten)]
        pub skip: crate::support::args::SkipArgs,
        /// Don't report exotic spaces (NBSP, em space, ideographic space). They are on by
        /// default as a real carrier class; silence them on prose that uses them as typography
        #[arg(long)]
        pub skip_spaces: bool,
        /// Don't scan file metadata at all — hidden characters only. By default every
        /// metadata string found is reported, tagged with why it's there
        #[arg(long)]
        pub skip_metadata: bool,
        /// Also report characters that build emoji — variation selectors, flag tag characters,
        /// and joiners after a non-ASCII character. Off by default: `⚠️` and `👨‍👩‍👧` are
        /// spelled with them, so reporting them buries real carriers in noise
        #[arg(short, long)]
        pub emoji: bool,
        /// Largest file to read, as bytes or with a K/M/G suffix (e.g. `512K`, `16M`,
        /// `1G` — binary multiples). Bigger files are named and skipped rather than loaded
        #[arg(long, value_name = "SIZE", default_value = "16M", value_parser = _parse_size)]
        pub max_file_size: u64,
    }

    /// The paths to scan: the file itself, or every file under a directory — unfiltered
    /// (`standard_filters(false)`, like `gg`), skipping only what the command line stated.
    ///
    /// Gitignore-based filtering was tried and removed: it skipped by rules written elsewhere
    /// (a parent repo's `.gitignore` hid whole projects), and the walk root was exempt while
    /// descendants weren't — so a tree scan could report a *disjoint* set from a scan of its
    /// own subdirectory. A detector's "nothing found" has to mean it.
    fn _files(target: &Path, skips: &[String]) -> Vec<PathBuf> {
        if target.is_file() {
            return vec![target.to_path_buf()];
        }
        WalkBuilder::new(target)
            .standard_filters(false)
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
            .map(ignore::DirEntry::into_path)
            .filter(|path| !crate::support::args::is_skipped(path, skips))
            .collect()
    }

    /// How the summary describes what the walk was allowed to miss. Silence about a skip is
    /// what made the old gitignore filtering so confusing — but the lean list outgrew a
    /// summary line, so it is referred to the same way `--help` refers to it, while the
    /// user's own patterns stay named in full (they are few, and they chose them).
    fn _scope_note(skip: &crate::support::args::SkipArgs) -> String {
        let mut parts: Vec<String> = Vec::new();
        if skip.lean {
            parts.push("--lean set (see _arg_lean_spec)".to_string());
        }
        parts.extend(skip.skip_pattern.iter().cloned());
        if parts.is_empty() { String::new() } else { format!(" (skipped: {})", parts.join(", ")) }
    }

    /// A file's scannable text: whole file when valid UTF-8, the decoded text section for
    /// containers [`delve`](crate::support::treegrep::delve) understands (.torrent names,
    /// video subtitles), `None` otherwise. Never a lossy decode of raw binary — pixel data
    /// contains carrier byte sequences by coincidence, and reporting those is crying wolf.
    fn _read_text(path: &Path, max_bytes: u64) -> Option<String> {
        use std::io::Read as _;

        let size = std::fs::metadata(path).ok()?.len();
        let mut file = std::fs::File::open(path).ok()?;
        // Sniff first, read second. A NUL byte in the first few KB is the classic binary test
        // (git and grep use it), and settling the question from a small prefix is the whole
        // point: reading a 300 MB `.rlib` in full only to fail a UTF-8 check is how a scan of
        // `target/` exhausts memory. Cost is bounded at SNIFF_BYTES per file either way.
        let mut head = vec![0_u8; SNIFF_BYTES.min(usize::try_from(size).unwrap_or(usize::MAX))];
        let read = file.read(&mut head).ok()?;
        head.truncate(read);
        if head.contains(&0) {
            // Binary. Only a format we can decode has text worth scanning.
            let embedded = crate::support::treegrep::delve::extract(path)?;
            return String::from_utf8(embedded).ok().filter(|text| !text.is_empty());
        }
        if size > max_bytes {
            // Text, but too big to hold. Skipping is the honest outcome — and it is announced,
            // because a file silently missed is exactly the failure this command must not have.
            eprintln!(
                "anti_ai_textual_watermark_detect: skipping {} ({} bytes > --max-file-size {})",
                path.display(),
                size,
                max_bytes
            );
            return None;
        }
        let mut bytes = head;
        bytes.reserve(usize::try_from(size).unwrap_or(0).saturating_sub(bytes.len()));
        file.read_to_end(&mut bytes).ok()?;
        String::from_utf8(bytes).ok()
    }

    /// How much of a file decides whether it is text. A NUL inside this prefix means binary.
    const SNIFF_BYTES: usize = 8 * 1024;

    /// A `--max-file-size` value: bytes, or a number with a `K`/`M`/`G` suffix.
    ///
    /// Binary multiples (1K = 1024), because the number's job is bounding memory and that is
    /// what an allocator counts in. A bare number is bytes, so `--max-file-size 0` is a legal
    /// way to say "read nothing", and the suffix is case-insensitive since nobody wants to be
    /// corrected about `16m`.
    fn _parse_size(raw: &str) -> Result<u64, String> {
        let trimmed = raw.trim();
        let (digits, scale) = match trimmed.chars().last() {
            Some(last) if last.is_ascii_alphabetic() => {
                let scale = match last.to_ascii_uppercase() {
                    'K' => 1_u64 << 10,
                    'M' => 1 << 20,
                    'G' => 1 << 30,
                    other => return Err(format!("unknown size suffix `{other}` (use K, M or G)")),
                };
                (&trimmed[..trimmed.len() - 1], scale)
            }
            _ => (trimmed, 1),
        };
        let value: u64 = digits
            .trim()
            .parse()
            .map_err(|_| format!("`{raw}` is not a size (try 512K, 16M, 1G, or a byte count)"))?;
        value
            .checked_mul(scale)
            .ok_or_else(|| format!("`{raw}` is larger than this machine can address"))
    }

    /// Every hidden character on one line, with the column it sits at.
    struct LineHits {
        number: usize,
        text: String,
        chars: Vec<(usize, char, &'static str)>, // (column, char, kind)
    }

    /// Scan `text` line by line.
    fn _scan(text: &str, spaces: bool, emoji: bool) -> Vec<LineHits> {
        let mut found = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let mut previous: Option<char> = None;
            let mut chars: Vec<(usize, char, &'static str)> = Vec::new();
            for (column, ch) in line.chars().enumerate() {
                let Some(kind) = _kind(ch) else {
                    previous = Some(ch);
                    continue;
                };
                let reportable = match kind {
                    SPACE => spaces,
                    _ if !emoji && _is_emoji_use(ch, previous) => false,
                    _ => true,
                };
                if reportable {
                    chars.push((column + 1, ch, kind));
                }
                previous = Some(ch);
            }
            if !chars.is_empty() {
                found.push(LineHits { number: index + 1, text: line.to_string(), chars });
            }
        }
        found
    }

    const SPACE: &str = "space";

    /// Whether this character is here to build an emoji rather than to hide a payload.
    ///
    /// Variation selectors and tag characters are emoji machinery almost everywhere they
    /// appear — `⚠️` is U+26A0 plus VS16, a flag is a base plus tag chars. A joiner counts as
    /// emoji use only when it *follows a non-ASCII character*, which covers 👨‍👩‍👧 and Persian
    /// `می‌روم` alike, while `a<ZWJ>b` between plain letters stays a carrier.
    ///
    /// A heuristic, deliberately: the alternative is shipping Unicode's emoji tables. It errs
    /// toward silence, which is why it is what `--emoji` turns OFF rather than what it turns on.
    fn _is_emoji_use(ch: char, previous: Option<char>) -> bool {
        match ch as u32 {
            0xFE00..=0xFE0F | 0xE0100..=0xE01EF | 0xE0000..=0xE007F => true,
            0x200C | 0x200D => previous.is_some_and(|prev| !prev.is_ascii()),
            _ => false,
        }
    }

    /// What class of hidden character this is, or `None` if it is ordinary text.
    ///
    /// An explicit table rather than a Unicode general-category lookup, which the standard
    /// library does not expose and which would cost a dependency to get. The ranges below are
    /// the format/invisible blocks actually used as carriers — everything here renders as
    /// nothing (or as a plain space) while still occupying a codepoint.
    fn _kind(ch: char) -> Option<&'static str> {
        match ch as u32 {
            0x200B | 0x200C | 0x200D | 0x2060 | 0xFEFF => Some("zero-width"),
            0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069 | 0x061C => Some("bidi"),
            0xE0000..=0xE007F => Some("tag-char"),
            0xFE00..=0xFE0F | 0xE0100..=0xE01EF => Some("variation-selector"),
            0x00AD | 0x034F | 0x180B..=0x180E | 0x2061..=0x2064 | 0x206A..=0x206F => {
                Some("format-control")
            }
            0xFFF9..=0xFFFB => Some("annotation"),
            // Zl/Zp, not Zs — and never silenceable, not even by `--skip-spaces`. `str::lines()` splits
            // on \n only, so one of these sits *inside* what this scan calls a line while many
            // editors and JS engines break there: the file displays with lines the report never
            // mentions. Nothing in ordinary prose needs them, so there is no legitimate use to
            // weigh against saying so.
            0x2028 | 0x2029 => Some("line-separator"),
            // The whole Zs category except U+0020 itself, which is the character the rest would
            // be normalised *to* — flagging it would report every space in every file.
            0x00A0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000 => Some(SPACE),
            _ => None,
        }
    }

    /// Print one file's hits: the path, then each offending line rendered with its hidden
    /// characters made visible in place, then the codepoints by name.
    fn _report_file(out: &mut impl Write, hits: &[LineHits]) -> io::Result<()> {
        for line in hits {
            let names: Vec<String> = line
                .chars
                .iter()
                .map(|(column, ch, kind)| format!("col {column}: U+{:04X} [{kind}]", *ch as u32))
                .collect();
            writeln!(out, "  {}: {}", line.number, names.join(", "))?;
            writeln!(out, "     {}", _visible(&line.text, &line.chars))?;
        }
        Ok(())
    }

    /// The line with every *reported* hidden character replaced by its codepoint in angle
    /// brackets, so the reader can see where it sits relative to the visible text. Rendering
    /// the raw line would show exactly nothing, which is the whole problem being reported.
    ///
    /// Driven by the hit list rather than by re-testing each character, so the markers and the
    /// list above them can never disagree: with spaces silenced, an exotic space is neither
    /// listed nor marked, instead of being silently marked as something the report never
    /// mentioned.
    fn _visible(line: &str, hits: &[(usize, char, &'static str)]) -> String {
        line.chars()
            .enumerate()
            .map(|(index, ch)| {
                if hits.iter().any(|(column, _, _)| *column == index + 1) {
                    problematic(&format!("<U+{:04X}>", ch as u32))
                } else {
                    ch.to_string()
                }
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_invisible_carriers_are_recognised_and_ordinary_text_is_not() {
            for (ch, expected) in [
                ('\u{200B}', "zero-width"),
                ('\u{200D}', "zero-width"),
                ('\u{202E}', "bidi"),
                ('\u{E0041}', "tag-char"),
                ('\u{FE0F}', "variation-selector"),
                ('\u{00AD}', "format-control"),
            ] {
                assert_eq!(_kind(ch), Some(expected), "U+{:04X}", ch as u32);
            }
            for ordinary in ['a', 'Z', ' ', '\t', 'é', '中', '🙂'] {
                assert_eq!(_kind(ordinary), None, "{ordinary:?} is ordinary text");
            }
        }

        /// Exotic spaces report by default (they are a real carrier class) and
        /// `--skip-spaces` silences them for prose that uses them as typography.
        #[test]
        fn exotic_spaces_can_be_silenced_but_default_on() {
            let text = "a\u{00A0}b";
            assert!(_scan(text, false, true).is_empty(), "silenced when the flag says so");
            let hits = _scan(text, true, true);
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].chars[0].2, SPACE);
        }

        /// The space class means the whole Zs category, not a handful of it. Enumerated rather than
        /// sampled, because the gap that prompted this test (U+1680) was a member nobody
        /// thought of — a range plus a few literals reads complete without being complete.
        #[test]
        fn every_unicode_space_separator_is_covered_except_the_ordinary_one() {
            let zs = [
                0x00A0_u32, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006,
                0x2007, 0x2008, 0x2009, 0x200A, 0x202F, 0x205F, 0x3000,
            ];
            for cp in zs {
                let ch = char::from_u32(cp).expect("a real codepoint");
                assert_eq!(_kind(ch), Some(SPACE), "U+{cp:04X} is a space separator");
            }
            assert_eq!(_kind(' '), None, "U+0020 is what the others normalise TO, not a carrier");
        }

        /// Line and paragraph separators are reported even under `--skip-spaces`: `str::lines()` does
        /// not split on them, so they hide *inside* a reported line while an editor shows a
        /// break there — and unlike an em space, no ordinary prose needs one.
        #[test]
        fn line_separators_are_always_reported_and_do_not_split_our_lines() {
            for cp in ['\u{2028}', '\u{2029}'] {
                assert_eq!(_kind(cp), Some("line-separator"), "{:04X}", cp as u32);
            }
            let text = "before\u{2028}after";
            assert_eq!(text.lines().count(), 1, "our line splitting does not see the break");
            let hits = _scan(text, false, true);
            assert_eq!(hits.len(), 1, "yet it is reported, with spaces silenced");
            assert_eq!(hits[0].chars[0].2, "line-separator");
        }

        #[test]
        fn a_hit_carries_its_line_number_and_column() {
            let text = "clean line\nhas a \u{200B}carrier\nalso clean";
            let hits = _scan(text, false, true);
            assert_eq!(hits.len(), 1, "only the middle line");
            assert_eq!(hits[0].number, 2, "line numbers count from one");
            assert_eq!(hits[0].chars[0].0, 7, "and so do columns");
            assert_eq!(hits[0].chars[0].1, '\u{200B}');
        }

        /// The reason the report renders lines at all: printing the raw line would show
        /// nothing where the carrier is, which is exactly what makes it hard to find.
        #[test]
        fn rendering_makes_the_invisible_visible_in_place() {
            let line = "hi\u{200B}there";
            let hits = _scan(line, false, true);
            let shown = _visible(line, &hits[0].chars);
            assert!(shown.contains("<U+200B>"), "{shown}");
            assert!(shown.contains("hi") && shown.contains("there"), "{shown}");
        }

        /// The markers and the listed hits are one decision, not two: a character the report
        /// chose not to list must not appear marked in the line beneath it.
        #[test]
        fn rendering_marks_exactly_what_was_reported() {
            let line = "a\u{00A0}b\u{200B}c";
            let quiet = _scan(line, false, true);
            let shown = _visible(line, &quiet[0].chars);
            assert!(shown.contains("<U+200B>"), "the carrier is marked: {shown}");
            assert!(!shown.contains("<U+00A0>"), "the unlisted space is not: {shown}");

            let loud = _scan(line, true, true);
            let shown = _visible(line, &loud[0].chars);
            assert!(shown.contains("<U+00A0>"), "with spaces on it is both listed and marked");
        }

        /// The lesson from the `--tidy` bug: a scan of a tree must never report *fewer*
        /// files than a scan of something inside it. The old gitignore-based filtering broke
        /// this — a directory an outer repo ignored vanished from above while still being
        /// scannable by name, producing disjoint results from the same tree.
        #[test]
        fn a_parent_scan_is_always_a_superset_of_a_child_scan() {
            let root = std::env::temp_dir().join(format!("bashrs_wm_sup_{}", std::process::id()));
            let buried = root.join("outer").join("proj").join("src");
            std::fs::create_dir_all(&buried).unwrap();
            // Every filtering rule that used to hide things, all at once.
            std::fs::write(root.join("outer").join(".gitignore"), "proj/\n").unwrap();
            std::fs::write(root.join("outer").join(".ignore"), "proj/\n").unwrap();
            std::fs::write(buried.join("carrier.md"), "x\u{200B}y").unwrap();

            let child = _files(&buried, &[]);
            assert!(!child.is_empty(), "the child scan sees its own file");
            for path in &child {
                assert!(
                    _files(&root, &[]).contains(path),
                    "{} is visible from the child but not from the root",
                    path.display()
                );
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        /// Skips come from the command line and nowhere else — no file on disk, anywhere in
        /// any parent directory, changes what gets scanned.
        #[test]
        fn only_the_stated_rules_skip_anything() {
            let root = std::env::temp_dir().join(format!("bashrs_wm_rule_{}", std::process::id()));
            let build = root.join("target").join("release");
            std::fs::create_dir_all(&build).unwrap();
            std::fs::create_dir_all(root.join(".hidden")).unwrap();
            std::fs::write(build.join("a.md"), "x\u{200B}y").unwrap();
            std::fs::write(root.join(".hidden").join("b.md"), "x\u{200B}y").unwrap();
            std::fs::write(root.join("src.md"), "x\u{200B}y").unwrap();

            assert_eq!(_files(&root, &[]).len(), 3, "by default nothing is skipped");
            assert_eq!(
                _files(&root, &["target/release".to_string()]).len(),
                2,
                "a skip drops the build output"
            );
            assert_eq!(
                _files(&root, &[".hidden".to_string(), "target".to_string()]).len(),
                1,
                "and skips compose"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        /// Silence about a skip is what made the old behaviour so confusing: "nothing found"
        /// has to say whether the walk was allowed to miss things.
        /// "Nothing found" has to say what the walk was allowed to miss: the user's own
        /// patterns by name (few, and chosen), the lean set by referral (it outgrew a line).
        #[test]
        fn the_summary_names_user_skips_and_refers_the_lean_set() {
            use crate::support::args::SkipArgs;
            let none = SkipArgs { skip_pattern: Vec::new(), lean: false };
            assert_eq!(_scope_note(&none), "", "a full scan says nothing extra");

            let user = SkipArgs { skip_pattern: vec!["mine/".to_string()], lean: false };
            assert!(_scope_note(&user).contains("mine/"));

            let lean = SkipArgs { skip_pattern: vec!["mine/".to_string()], lean: true };
            let note = _scope_note(&lean);
            assert!(note.contains("_arg_lean_spec"), "the lean set is referred, not dumped: {note}");
            assert!(note.contains("mine/"), "the user's own patterns stay named: {note}");
        }

        #[test]
        fn a_named_file_is_scanned_even_when_a_skip_would_hide_it() {
            let dir = std::env::temp_dir().join(format!("bashrs_wm_{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let hidden = dir.join(".hidden.md");
            std::fs::write(&hidden, "x\u{200B}y").unwrap();
            // Naming a file IS the decision: skip rules apply to a walk, not to a named path.
            assert_eq!(_files(&hidden, &[".hidden".to_string()]), vec![hidden.clone()]);
            assert!(_files(&dir, &[]).contains(&hidden), "a default walk finds it");
            let _ = std::fs::remove_dir_all(&dir);
        }

        /// Emoji are spelled with the same codepoints carriers use, and there are far more
        /// emoji in a normal repo than carriers — so reporting them by default buries the
        /// signal. On this project they were the majority of all hits.
        #[test]
        fn emoji_machinery_is_quiet_unless_asked_for() {
            for text in ["warn \u{26A0}\u{FE0F} here", "fam \u{1F468}\u{200D}\u{1F469}", "\u{1F3F4}\u{E0067}\u{E0062}"] {
                assert!(_scan(text, false, false).is_empty(), "quiet by default: {text:?}");
                assert!(!_scan(text, false, true).is_empty(), "--emoji reports it: {text:?}");
            }
        }

        /// The other half: the same joiner between plain ASCII is a carrier, not emoji glue,
        /// and stays reported without `--emoji`. Persian `می‌روم` is the case the rule protects.
        #[test]
        fn a_joiner_between_ascii_is_still_a_carrier() {
            assert_eq!(_scan("a\u{200D}b", false, false).len(), 1, "ASCII neighbours: a carrier");
            assert!(_scan("\u{0645}\u{200C}\u{0631}", false, false).is_empty(), "Persian ZWNJ is not");
            // And the carriers that are never emoji stay on by default.
            assert_eq!(_scan("a\u{200B}b", false, false).len(), 1, "ZWSP is always reported");
        }

        /// The scan that ran the machine out of memory: `std::fs::read` loaded every file in
        /// full — including a 300 MB `.rlib` — before the UTF-8 check could reject it. Deciding
        /// from a prefix is what bounds the cost.
        #[test]
        fn a_binary_file_is_rejected_from_its_prefix_not_its_whole_length() {
            let dir = std::env::temp_dir().join(format!("bashrs_wm_big_{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let blob = dir.join("big.rlib");
            // A NUL early, then far more than the sniff window. Reading it all would work too —
            // the point is that it must not need to.
            let mut bytes = vec![0_u8; 4];
            bytes.extend(std::iter::repeat_n(b'x', SNIFF_BYTES * 4));
            std::fs::write(&blob, &bytes).unwrap();
            assert!(_read_text(&blob, 16 << 20).is_none(), "binary, and no decoder for .rlib");
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sizes_parse_in_the_spellings_people_actually_type() {
            assert_eq!(_parse_size("16M"), Ok(16 << 20), "the default spelling");
            assert_eq!(_parse_size("16m"), Ok(16 << 20), "suffixes are case-insensitive");
            assert_eq!(_parse_size("512K"), Ok(512 << 10));
            assert_eq!(_parse_size("1G"), Ok(1 << 30));
            assert_eq!(_parse_size("2048"), Ok(2048), "a bare number is bytes");
            assert_eq!(_parse_size(" 4M "), Ok(4 << 20), "surrounding space is not an error");
            assert_eq!(_parse_size("0"), Ok(0), "a legal way to say `read nothing`");

            for bad in ["", "M", "16T", "sixteen", "16MB", "-1"] {
                assert!(_parse_size(bad).is_err(), "{bad:?} should be rejected");
            }
            assert!(_parse_size("99999999999999999999G").is_err(), "overflow is an error, not a wrap");
        }

        /// A text file too large to hold is named and skipped, never silently dropped.
        #[test]
        fn an_oversized_text_file_is_skipped_rather_than_loaded() {
            // The cap is enforced against real bytes, both ways.
            assert_eq!(_read_text(Path::new("/nonexistent"), 1 << 20), None);
            let dir = std::env::temp_dir().join(format!("bashrs_wm_cap_{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let small = dir.join("ok.md");
            std::fs::write(&small, "x\u{200B}y").unwrap();
            assert!(_read_text(&small, 16 << 20).is_some(), "an ordinary file is still read whole");
            assert!(_read_text(&small, 2).is_none(), "and the cap really refuses one over it");
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn binary_files_are_skipped_rather_than_reported_as_noise() {
            let dir = std::env::temp_dir().join(format!("bashrs_wm_bin_{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let binary = dir.join("blob.bin");
            // Bytes that WOULD decode to a carrier under a lossy read: E2 80 8B is U+200B.
            std::fs::write(&binary, [0xFF_u8, 0xFE, 0xE2, 0x80, 0x8B, 0x00]).unwrap();
            assert!(
                _read_text(&binary, 16 << 20).is_none(),
                "raw binary is skipped, not lossily decoded — coincidental bytes are not carriers"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        /// The other half of that rule: a binary container with a real text section IS read,
        /// through the format decoder, so a carrier hidden in a torrent's file names is found.
        #[test]
        fn a_container_with_a_text_section_is_read_through_its_decoder() {
            let dir = std::env::temp_dir().join(format!("bashrs_wm_tor_{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let torrent = dir.join("x.torrent");
            // Minimal bencode: a dict whose "name" carries a zero-width space, plus a binary
            // "pieces" blob the decoder must leave out.
            let mut data = b"d4:name14:clean\xe2\x80\x8bname6:pieces4:".to_vec();
            data.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x01]);
            data.push(b'e');
            std::fs::write(&torrent, &data).unwrap();
            assert!(String::from_utf8(data).is_err(), "the file as a whole is not valid UTF-8");

            let text = _read_text(&torrent, 16 << 20).expect("the text section is decoded");
            assert!(text.contains('\u{200B}'), "the carrier in the name is reachable: {text:?}");
            assert!(!_scan(&text, false, true).is_empty(), "and it is reported");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
