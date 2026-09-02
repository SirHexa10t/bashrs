//! Commands for dealing with AI-generated content on your own machine (`anti_ai_*`).
//!
//! One command, `detect_ai_textual_fingerprint`, asking one question — does this file show
//! signs of AI? — of three kinds of evidence in a single walk:
//!
//! 1. **Hidden characters** in text: the invisible codepoints that carry edit-based watermarks,
//!    and the same ones pasted in by accident that then break diffs, greps and search.
//! 2. **Provenance labels** in file metadata: frontmatter and `<meta>` generator fields, XMP
//!    CreatorTool, C2PA containers, vendor names.
//! 3. **Writing tells**: excess vocabulary, stock phrases, sentence shapes, formatting habits.
//!
//! The three are graded, not pooled. The first two are *verifiable* — a codepoint is there or it
//! is not — and they alone set the exit code, so `… && publish` still refuses to chain past one.
//! The third is style: reported with an evidence tier and a caveat on every finding, and never
//! able to fail a build. `--skip-tells` drops it entirely.
//!
//! Each engine also sees a different thing, and that is the point of keeping them apart: the
//! character and prose scans read the file's *text*, while the metadata scan reads its raw bytes
//! and opens only the metadata containers. So a bare "AI" is a marker in a `Generator` field and
//! unremarkable in a sentence — the word is suspicious in a label, not in prose.
//!
//! **The detection itself is not here.** All three engines live in the [`ai_detection`] crate,
//! which knows nothing of this shell — no walking, no skipping, no colour — so anything can use
//! them. What stays in this module is the half that is bashrs's: the clap surface, the tree walk
//! under this shell's shared `--skip`/`--lean` conventions ([`crate::support::args`]), reading a
//! file safely (binary sniff, size cap, and [`delve`](crate::support::treegrep::delve) for
//! containers with a text section), telling the crate whether a surface renders Markdown, and
//! rendering the report in this shell's palette.
//!
//! Deliberately **detect only**. Reporting where something is is a different act from altering
//! a file, and only the first is safe to run over a whole tree by default. What the output is
//! good for is deciding — the codepoints are printed by name so a reader can judge whether a
//! given one is a carrier, an emoji joiner, or ordinary orthography.
//!
//! What is *not* claimed: statistical (token-sampling) watermarks live in word choice with
//! nothing to scan for, and no vendor is ever named from prose. See
//! [`ai_detection::tells::pending::GAPS`] for the detectors the reference document describes and
//! this does not yet implement.

#[bashrs_macros::category(command = AiFingerprintCommand, prefix = "detect_ai_")]
mod commands {
    use ai_detection::{hidden, metadata, tells};
    use crate::support::doc_style::{approved, notice, problematic};
    use clap::Args;
    use ignore::WalkBuilder;
    use std::io::{self, BufWriter, Write};
    use std::path::{Path, PathBuf};

    /// Find AI marks in a file, or in every file under a directory: hidden characters in text
    /// (zero-width joiners, bidi controls, tag chars — with the line and codepoint of each), AI
    /// provenance in metadata (frontmatter, `<meta>` generator, XMP CreatorTool, C2PA, vendor
    /// names) for md/html/svg/png/jpeg, and writing tells in prose (excess vocabulary, stock
    /// phrases, sentence shapes, formatting habits — each with its evidence tier and its reason
    /// for doubt). Only the first two are verifiable and only they set the exit code;
    /// `--skip-tells` drops the third. Media data is never decoded; changes nothing
    pub fn textual_fingerprint(args: FingerprintArgs) {
        _detect(args);
    }

    /// All three scans, one walk.
    ///
    /// The engines are fed different things on purpose, and it matters: [`hidden`] and [`tells`]
    /// see the file's *text*, while [`metadata`] sees its raw bytes and reads only the metadata
    /// containers. That separation is why a bare "AI" counts as a marker in a `Generator` field
    /// but not in a sentence — the word is suspicious in a label and unremarkable in prose, and
    /// neither engine's vocabulary leaks into the other's.
    fn _detect(args: FingerprintArgs) {
        let FingerprintArgs {
            target,
            skip,
            skip_spaces,
            emoji,
            skip_metadata,
            skip_tells,
            max_file_size,
        } = args;
        let skips = skip.skips();
        if !target.exists() {
            eprintln!("detect_ai_textual_fingerprint: no such path: {}", target.display());
            std::process::exit(1);
        }
        // One locked, buffered writer for the whole report: a per-line `println!` would take
        // the stdout lock and syscall for every hit, and a tree scan can produce thousands.
        let mut out = BufWriter::new(io::stdout().lock());
        let mut files_with_marks = 0_usize;
        let mut files_with_tells = 0_usize;
        let mut total_hidden = 0_usize;
        let mut total_meta = 0_usize;
        let mut fired: Vec<tells::Family> = Vec::new();

        for path in _files(&target, &skips) {
            // Read once, scan twice: both text engines want the same decoded string.
            let text = _read_text(&path, max_file_size);
            let hits = text
                .as_deref()
                .map(|text| hidden::scan(text, !skip_spaces, emoji))
                .unwrap_or_default();
            // Metadata is a second look at the same file, gated by whether the engine has an
            // extractor at all — no point reading a .rs file's bytes twice to learn nothing.
            let findings = if !skip_metadata && metadata::handles(&path) {
                _read_capped(&path, max_file_size)
                    .and_then(|bytes| metadata::extract(&path, &bytes, true))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let prose = match (skip_tells, text.as_deref()) {
                (false, Some(text)) => Some(tells::scan_with(text, _surface_of(&path))),
                _ => None,
            };
            let prose_tells = prose.as_ref().map_or(0, |report| report.tells.len());
            if hits.is_empty() && findings.is_empty() && prose_tells == 0 {
                continue;
            }

            let verifiable = !hits.is_empty() || !findings.is_empty();
            files_with_marks += usize::from(verifiable);
            files_with_tells += usize::from(prose_tells > 0);
            total_hidden += hits.iter().map(|line| line.hits.len()).sum::<usize>();
            total_meta += findings.len();
            let _ = writeln!(out, "\n{}", notice(&path.display().to_string()));
            let _ = _report_file(&mut out, &hits);
            let _ = _report_metadata(&mut out, &findings);
            if let Some(report) = &prose {
                fired.extend(report.tells.iter().map(|tell| tell.family));
                let _ = _report_tells(&mut out, report);
            }
        }

        // Named on both paths: "nothing found" means something different when the walk was
        // allowed to skip files, and the reader cannot tell from the output otherwise.
        let scope = _scope_note(&skip);
        let _ = if files_with_marks == 0 && files_with_tells == 0 {
            writeln!(out, "{}{scope}", approved("no AI marks, metadata or writing tells found"))
        } else {
            writeln!(
                out,
                "\n{}",
                problematic(&format!(
                    "{total_hidden} hidden character(s), {total_meta} metadata finding(s) \
                     across {files_with_marks} file(s); writing tells in \
                     {files_with_tells} file(s){scope}"
                ))
            )
        };
        if !skip_tells {
            _tells_epilogue(&mut out, files_with_tells, &mut fired);
        }
        let _ = out.flush();
        if files_with_marks > 0 {
            // Non-zero on the VERIFIABLE half only, so `detect_ai_textual_fingerprint &&
            // publish` still refuses to chain past a hidden character or an AI metadata label —
            // artifacts, worth cleaning. Writing tells never set it: they are style, they prove
            // nothing, and an exit code would invite gating a pipeline on prose, which is the
            // harm the source document records (essays flagged, writers self-censoring
            // punctuation they had used for decades).
            std::process::exit(1);
        }
    }

    /// What only this side knows about a file: whether its surface renders Markdown, which
    /// decides if a visible `**` is residue or just markup. The extension is bashrs's to read —
    /// the same division as the walk itself.
    fn _surface_of(path: &Path) -> tells::Options {
        let markdown = path.extension().is_some_and(|kind| {
            kind.eq_ignore_ascii_case("md") || kind.eq_ignore_ascii_case("markdown")
        });
        tells::Options { surface_is_plain: !markdown }
    }

    /// One file's writing tells: the evidence mark, the family, where, and why it matched.
    fn _report_tells(out: &mut impl Write, report: &tells::Report) -> io::Result<()> {
        for tell in &report.tells {
            let place = tell.line.map_or_else(String::new, |line| format!("line {line}: "));
            writeln!(
                out,
                "  {} {} — {place}{}",
                tell.family.evidence().mark(),
                tell.family.title(),
                tell.detail
            )?;
            if !tell.excerpt.is_empty() {
                writeln!(out, "       {}", tell.excerpt)?;
            }
        }
        writeln!(
            out,
            "  {}",
            notice(&format!(
                "{} of {} tell families, in {} words",
                report.families().len(),
                tells::Family::ALL.len(),
                report.words
            ))
        )
    }

    /// The part of the report that must never be omitted: which families fired, the reason to
    /// doubt each one, and the standing warning that none of this identifies a document.
    fn _tells_epilogue(out: &mut impl Write, files: usize, fired: &mut Vec<tells::Family>) {
        if files == 0 {
            let _ = writeln!(out, "{}", approved("no writing tells found"));
            let _ = writeln!(
                out,
                "  {}",
                notice("absence means nothing either — these markers are trivially removable")
            );
            return;
        }
        fired.sort_unstable();
        fired.dedup();
        let _ = writeln!(out, "\n{}", problematic(&format!("tells in {files} file(s)")));
        let _ = writeln!(out, "\nwhy each family might be innocent:");
        for family in fired.iter() {
            let _ = writeln!(
                out,
                "  {} {} (§{}) — {}",
                family.evidence().mark(),
                family.title(),
                family.section(),
                family.caveat()
            );
        }
        let _ = writeln!(
            out,
            "\n{}",
            problematic(
                "None of this is proof of AI authorship. Every marker has a legitimate human \
                 use, the strongest are corpus-level statistics being read against single \
                 documents, and a text ABOUT these markers looks exactly like one exhibiting \
                 them. Weigh clusters of independent families, never a single hit."
            )
        );
    }

    /// A file's raw bytes, size-capped the same way the textual scan is. No text sniff here:
    /// the metadata extractor wants binary formats too, and does its own routing.
    fn _read_capped(path: &Path, max_bytes: u64) -> Option<Vec<u8>> {
        let size = std::fs::metadata(path).ok()?.len();
        if size > max_bytes {
            eprintln!(
                "detect_ai_textual_fingerprint: skipping {} ({} bytes > --max-file-size {})",
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
    fn _report_metadata(out: &mut impl Write, findings: &[metadata::Finding]) -> io::Result<()> {
        for finding in findings {
            // Matches and provenance fields are tagged; the rest of the inventory is plain —
            // an untagged line means exactly "metadata, listed by default, nothing matched".
            let why = match &finding.why {
                metadata::Why::Marker(marker) => format!("  {}", problematic(&format!("[marker: {marker}]"))),
                metadata::Why::ProvenanceField => format!("  {}", notice("[provenance field]")),
                metadata::Why::Everything => String::new(),
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
    pub struct FingerprintArgs {
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
        /// Don't report writing tells — the verifiable marks only. Tells are prose habits
        /// (vocabulary, stock phrases, sentence shapes, formatting); they prove nothing on
        /// their own and never affect the exit code, so this silences the noisiest half
        #[arg(long)]
        pub skip_tells: bool,
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
                "detect_ai_textual_fingerprint: skipping {} ({} bytes > --max-file-size {})",
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

    /// Print one file's hits: each offending line's codepoints by name, then the line itself
    /// rendered with those characters made visible in place. The rendering is the engine's
    /// ([`hidden::render_line`], so markers can never disagree with the list above them); the
    /// colour it wraps them in is ours.
    fn _report_file(out: &mut impl Write, lines: &[hidden::LineHits]) -> io::Result<()> {
        for line in lines {
            let names: Vec<String> = line
                .hits
                .iter()
                .map(|hit| format!("col {}: U+{:04X} [{}]", hit.column, hit.ch as u32, hit.kind))
                .collect();
            writeln!(out, "  {}: {}", line.number, names.join(", "))?;
            writeln!(out, "     {}", hidden::render_line(line, problematic))?;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // What the detection engine decides — which codepoints are carriers, when a joiner is
        // emoji glue, how a marked line renders — is tested in `ai_detection`, where it now
        // lives. What remains here is this command's own half: which files the walk visits,
        // what it will and won't read, and how it says what it skipped.

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
            assert!(!hidden::scan(&text, false, true).is_empty(), "and it is reported");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
