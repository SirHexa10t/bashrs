//! End-to-end tests of the search commands — the `g`/`gg` families, `hg`, and `GGG` — driving the
//! real binary over a constructed directory tree. The unit tests pin each engine piece; these pin
//! the features *together* as a user meets them: argument parsing, the generated shims' pinned
//! context, the recursive walk (hidden files in, binaries out), literal-vs-regex matching, and
//! `--save`'s live-log/`_sorted`-sibling file handling.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Run the real `bashrs` with `args`, an optional working directory, and optional piped stdin.
/// Output is captured, so stdout is never a terminal — colour is off, matching piped use.
fn bashrs(args: &[&str], cwd: Option<&Path>, stdin: Option<&[u8]>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bashrs"));
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    match stdin {
        Some(bytes) => {
            cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = cmd.spawn().expect("spawn bashrs");
            child.stdin.take().expect("piped stdin").write_all(bytes).expect("write stdin");
            child.wait_with_output().expect("run bashrs")
        }
        None => {
            cmd.stdin(Stdio::null());
            cmd.output().expect("run bashrs")
        }
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A self-cleaning temp directory; [`Tree::build`] populates it as the standard search fixture.
struct Tree {
    root: PathBuf,
}

impl Tree {
    /// An empty unique temp directory (also used as a working dir for the `--save` tests).
    fn empty(tag: &str) -> Tree {
        let root = std::env::temp_dir().join(format!("bashrs_search_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Tree { root }
    }

    /// The standard fixture, one file per behaviour under test:
    /// - `match_in_name.txt` — hits the *filename* pass only (no matching content);
    /// - `data.txt` — a content hit on line 2, with context lines around it;
    /// - `sub/nested.log` — a hit below a subdirectory (recursion);
    /// - `.hidden_notes` — a hit in a hidden file (searched, unlike ripgrep's default);
    /// - `literal.txt` — contains the text `mat.h`, to tell literal from `-E` regex matching;
    /// - `binary.bin` — leads with NUL bytes, so binary detection must skip its "match".
    fn build(tag: &str) -> Tree {
        let tree = Tree::empty(tag);
        fs::create_dir(tree.root.join("sub")).unwrap();
        fs::write(tree.root.join("match_in_name.txt"), "no hits inside\n").unwrap();
        fs::write(tree.root.join("data.txt"), "above\nthe match line\nbelow\n").unwrap();
        fs::write(tree.root.join("sub/nested.log"), "nested match\n").unwrap();
        fs::write(tree.root.join(".hidden_notes"), "hidden match\n").unwrap();
        fs::write(tree.root.join("literal.txt"), "a mat.h literal\n").unwrap();
        fs::write(tree.root.join("binary.bin"), b"\x00\x00binary match after NUL\n").unwrap();
        tree
    }

    fn path(&self) -> &str {
        self.root.to_str().unwrap()
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn gg_finds_names_contents_hidden_and_nested_but_skips_binaries() {
    let tree = Tree::build("walk");
    let out = bashrs(&["gg", "match", "-d", tree.path()], None, None);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("match_in_name.txt"), "filename pass missing: {text}");
    assert!(text.contains("data.txt:2:the match line"), "content hit with line number missing: {text}");
    assert!(text.contains("nested.log:1:nested match"), "recursion missing: {text}");
    assert!(text.contains(".hidden_notes:1:hidden match"), "hidden files should be searched: {text}");
    assert!(!text.contains("binary.bin:"), "NUL binaries must be skipped: {text}");
    // The interactive section labels are present when not saving.
    assert!(text.contains("matching filenames:") && text.contains("matching file contents:"), "{text}");
}

/// `--skip-pattern` prunes what it names below the searched directory — and naming that very
/// directory searches it: the skips are matched below the root, not against the root's own name.
#[test]
fn gg_skips_below_the_named_directory_but_searches_it_when_named() {
    let tree = Tree::build("skips");
    let from_above = stdout(&bashrs(&["gg", "match", "--skip-pattern", "sub", "-d", tree.path()], None, None));
    assert!(!from_above.contains("nested.log") && from_above.contains("data.txt"), "only the skipped directory is left out: {from_above}");
    let named = tree.root.join("sub");
    let out = bashrs(&["gg", "match", "--skip-pattern", "sub", "-d", named.to_str().unwrap()], None, None);
    assert!(stdout(&out).contains("nested.log:1:nested match"), "naming it searches it: {}", stdout(&out));
}

#[test]
fn gg_ors_multiple_expressions_together() {
    let tree = Tree::build("or");
    let text = stdout(&bashrs(&["gg", "above", "below", "-d", tree.path()], None, None));
    assert!(text.contains("data.txt:1:above") && text.contains("data.txt:3:below"), "{text}");
}

#[test]
fn gg_context_comes_from_the_flag_or_the_pinned_variant() {
    let tree = Tree::build("ctx");
    let flagged = stdout(&bashrs(&["gg", "-C", "1", "match", "-d", tree.path()], None, None));
    assert!(flagged.contains("data.txt-1-above") && flagged.contains("data.txt-3-below"),
        "-C context lines use the dash separator: {flagged}");
    let pinned = stdout(&bashrs(&["gg2", "match", "-d", tree.path()], None, None));
    assert!(pinned.contains("data.txt-1-above"), "gg2 must pin -C 2 by itself: {pinned}");
}

/// A two-page PDF — the first page's lines `first`, the second's `second` — in a standard font,
/// one line per `Td` step down.
fn two_page_pdf(first: &[&str], second: &[&str]) -> Vec<u8> {
    let content = |lines: &[&str]| {
        let mut text = String::from("BT /F1 12 Tf 72 700 Td");
        for line in lines {
            text.push_str(&format!(" ({line}) Tj 0 -14 Td"));
        }
        text.push_str(" ET");
        format!("<< /Length {} >>\nstream\n{text}\nendstream", text.len())
    };
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /Resources << /Font << /F1 5 0 R >> >> >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        content(first),
        content(second),
    ];
    let mut pdf = String::from("%PDF-1.4\n");
    for (number, body) in (1..).zip(objects) {
        pdf.push_str(&format!("{number} 0 obj\n{body}\nendobj\n"));
    }
    pdf.push_str("trailer\n<< /Root 1 0 R >>\n%%EOF\n");
    pdf.into_bytes()
}

/// A PDF's text is searched by default — no `--delve` — page by page, each hit labelled with its
/// page where a line number would be, and context taken from the page around it.
#[test]
fn gg_reads_a_pdf_page_by_page_without_delve() {
    let tree = Tree::empty("pdf");
    let pdf = two_page_pdf(&["Nothing to see"], &["Above it", "The Chute arrives", "Below it"]);
    fs::write(tree.root.join("order.pdf"), pdf).unwrap();
    let text = stdout(&bashrs(&["gg", "chute", "-d", tree.path()], None, None));
    assert!(text.contains("order.pdf:page 2:The Chute arrives"), "{text}");
    assert!(!text.contains("page 1"), "only the page with the hit: {text}");
    let context = stdout(&bashrs(&["gg2", "chute", "-d", tree.path()], None, None));
    assert!(context.contains("order.pdf-page 2-Above it") && context.contains("order.pdf-page 2-Below it"),
        "context lines come from the same page, dash-separated: {context}");
}

/// An encrypted PDF's text can't be read until someone decrypts it, which `gg` leaves to the user:
/// every one it met is named once the search is done — `.PDF` as well as `.pdf` — so a decrypted
/// copy of each can be made for the next search.
#[test]
fn gg_names_every_encrypted_pdf_it_could_not_read() {
    let tree = Tree::empty("encrypted");
    let pdf = two_page_pdf(&["The Chute arrives"], &[]);
    let locked = String::from_utf8(pdf.clone()).unwrap().replace("<< /Root 1 0 R >>", "<< /Root 1 0 R /Encrypt << /Filter /Standard >> >>");
    fs::write(tree.root.join("open.pdf"), pdf).unwrap();
    fs::create_dir(tree.root.join("statements")).unwrap();
    fs::write(tree.root.join("statements/march.pdf"), &locked).unwrap();
    fs::write(tree.root.join("april.PDF"), &locked).unwrap();

    let out = bashrs(&["gg", "chute", "-d", tree.path()], None, None);
    let (found, err) = (stdout(&out), stderr(&out));
    assert!(found.contains("open.pdf:page 1:The Chute arrives"), "the readable one is searched: {found}");
    assert!(!found.contains("march") && !found.contains("april"), "the encrypted ones yield nothing: {found}");
    let listed: Vec<&str> = err.lines().skip_while(|line| !line.contains("(encrypted)")).collect();
    let (april, march) = (tree.root.join("april.PDF"), tree.root.join("statements/march.pdf"));
    assert_eq!(
        listed,
        [
            "2 PDF(s) skipped (encrypted) — decrypt a copy of each to have its text searched:".to_owned(),
            format!("  {}", april.display()),
            format!("  {}", march.display()),
        ],
    );
}

/// Every file `gg` set out to read but couldn't is named once the search is done, grouped by why:
/// what they are, why, and — where something would — what would have the next search read them.
/// A file named like a format it isn't — an INI file Microsoft's SMS named `.pdf`, a web page a
/// failed download saved as one — is searched like any other file, and named under a heading
/// saying so.
#[test]
fn gg_names_every_file_it_set_out_to_read_but_could_not() {
    let tree = Tree::empty("unsearched");
    let pdf = two_page_pdf(&["The Chute arrives"], &[]);
    let locked = String::from_utf8(pdf.clone()).unwrap().replace("<< /Root 1 0 R >>", "<< /Root 1 0 R /Encrypt << /Filter /Standard >> >>");
    fs::write(tree.root.join("open.pdf"), &pdf).unwrap();
    fs::write(tree.root.join("locked.pdf"), locked).unwrap();
    fs::write(tree.root.join("scan.pdf"), two_page_pdf(&[], &[])).unwrap();
    fs::write(tree.root.join("broken.pdf"), "%PDF-1.4\n%%EOF\n").unwrap();
    fs::write(tree.root.join("book.pdf"), "%PDF-1.4\n").unwrap();
    fs::OpenOptions::new().write(true).open(tree.root.join("book.pdf")).unwrap().set_len(300 << 20).unwrap(); // sparse: never read
    fs::write(tree.root.join("error.pdf"), "<html>404 Not Found</html>").unwrap();
    fs::write(tree.root.join("package.pdf"), "[PDF]\nVersion = 2.0\n[Setup]\nCommandLine = install the Chute\n").unwrap();
    fs::write(tree.root.join("movie.mkv"), "not a video").unwrap();

    let out = bashrs(&["gg", "--delve", "chute", "-d", tree.path()], None, None);
    let found = stdout(&out);
    assert!(found.contains("open.pdf:page 1:The Chute arrives"), "{found}");
    assert!(found.contains("package.pdf:4:CommandLine = install the Chute"), "searched as the text it is: {found}");
    let at = |name: &str| format!("  {}", tree.root.join(name).display());
    let err = stderr(&out);
    let named: Vec<&str> = err.lines().skip_while(|line| !line.contains(" skipped (")).collect();
    assert_eq!(
        named,
        [
            "1 PDF(s) skipped (encrypted) — decrypt a copy of each to have its text searched:".to_owned(),
            at("locked.pdf"),
            String::new(),
            "1 PDF(s) skipped (no text — a scan?) — run a copy of each through OCR to have its text searched:".to_owned(),
            at("scan.pdf"),
            String::new(),
            "1 file(s) skipped (over 256 MiB — too big to decode):".to_owned(),
            at("book.pdf"),
            String::new(),
            "1 file(s) skipped (damaged or cut short):".to_owned(),
            at("broken.pdf"),
            String::new(),
            "2 file(s) named as a PDF, but not one — searched like any other file:".to_owned(),
            at("error.pdf"),
            at("package.pdf"),
            String::new(),
            "1 file(s) named as a Matroska file, but not one — searched like any other file:".to_owned(),
            at("movie.mkv"),
        ],
    );
}

/// `--no-line-number` leaves out a PDF hit's page — a position, like a line number.
#[test]
fn gg_no_line_number_leaves_out_a_pdf_hit_s_page() {
    let tree = Tree::empty("pdf-unnumbered");
    fs::write(tree.root.join("order.pdf"), two_page_pdf(&["Nothing to see"], &["The Chute arrives"])).unwrap();
    let found = stdout(&bashrs(&["gg", "--no-line-number", "Chute", "-d", tree.path()], None, None));
    assert!(found.contains("order.pdf:The Chute arrives"), "{found}");
    assert!(!found.contains("page 2"), "{found}");
}

/// `--no-line-number` leaves out a subtitle hit's time — a position, like a line number — and
/// keeps its track.
#[test]
fn gg_no_line_number_leaves_out_a_subtitle_hit_s_time() {
    let tree = Tree::empty("subtitles-unnumbered");
    fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/d-webvtt.mkv"), tree.root.join("kitchen.mkv")).unwrap();
    fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tx3g-tracks.mp4"), tree.root.join("kitchen.mp4")).unwrap();
    let found = stdout(&bashrs(&["gg", "--delve", "--no-line-number", "salt", "-d", tree.path()], None, None));
    assert!(found.contains("kitchen.mkv:eng:The kitchen ran out of salt"), "{found}");
    assert!(found.contains("kitchen.mp4:English (SDH):The kitchen ran out of salt"), "{found}");
    assert!(!found.contains("0:00:00"), "{found}");
}

/// A hit in a video's subtitles says which track holds it and when the cue starts, where a text
/// file's line number would be — and its context comes from the same track.
#[test]
fn gg_delve_places_each_subtitle_hit_by_track_and_time() {
    let tree = Tree::empty("subtitles");
    fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/d-webvtt.mkv"), tree.root.join("kitchen.mkv")).unwrap();
    fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tx3g-tracks.mp4"), tree.root.join("kitchen.mp4")).unwrap();
    let found = stdout(&bashrs(&["gg", "--delve", "-C", "1", "salt", "-d", tree.path()], None, None));
    // An untitled track goes by its language, a titled one by its title.
    assert!(found.contains("kitchen.mkv:eng 0:00:00:The kitchen ran out of salt"), "{found}");
    assert!(found.contains("kitchen.mkv-eng 0:00:02-Second helping is on the house"), "{found}");
    assert!(found.contains("kitchen.mp4:English (SDH) 0:00:00:The kitchen ran out of salt"), "{found}");
    assert!(found.contains("kitchen.mp4-English (SDH) 0:00:05-Second helping is on the house"), "{found}");
}

#[test]
fn gg_matches_literally_by_default_and_as_regex_with_the_flag() {
    let tree = Tree::build("regex");
    let literal = stdout(&bashrs(&["gg", "mat.h", "-d", tree.path()], None, None));
    assert!(literal.contains("literal.txt"), "literal dot should match itself: {literal}");
    assert!(!literal.contains("data.txt"), "a literal dot must not match 'match': {literal}");
    let regex = stdout(&bashrs(&["gg", "-E", "mat.h", "-d", tree.path()], None, None));
    assert!(regex.contains("literal.txt") && regex.contains("data.txt"),
        "-E: the dot should match any character: {regex}");
}

#[test]
fn gg_reports_no_results_and_notes_denied_paths_when_non_interactive() {
    use std::os::unix::fs::PermissionsExt;
    let tree = Tree::build("denied");
    let locked = tree.root.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("secret.txt"), "locked match\n").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Under root (some sandboxes) chmod 000 doesn't deny; skip the denial assertion there.
    let denial_works = fs::read_dir(&locked).is_err();

    let out = bashrs(&["gg", "zzz_no_such_term", "-d", tree.path()], None, None);
    let err = stderr(&out);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap(); // let Drop clean up
    assert!(err.contains("NO RESULTS FOUND"), "{err}");
    if denial_works {
        assert!(err.contains("skipped (permission denied); run interactively"),
            "non-interactive runs should note the denied paths: {err}");
    }
}

#[test]
fn gg_save_tees_live_and_leaves_only_a_sorted_file() {
    let tree = Tree::build("save");
    let cwd = Tree::empty("save_cwd");
    let out = bashrs(&["gg", "-s", "match", "-d", tree.path()], Some(&cwd.root), None);

    // The tee still printed results to stdout — but no section labels in save mode.
    let text = stdout(&out);
    assert!(text.contains("data.txt:2:the match line"), "{text}");
    assert!(!text.contains("matching filenames:"), "labels are suppressed under --save: {text}");

    // Only the `deep_search_<stamp>_sorted` file remains; the live log was deleted.
    let files: Vec<PathBuf> = fs::read_dir(&cwd.root).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(files.len(), 1, "expected just the sorted file: {files:?}");
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("deep_search_") && name.ends_with("_sorted"), "{name}");

    // Header first, then the blocks ordered by path (.hidden_notes < data.txt < sub/nested.log).
    let content = fs::read_to_string(&files[0]).unwrap();
    assert!(content.starts_with("# search term used: match\n"), "{content}");
    let (hidden, data, nested) = (
        content.find(".hidden_notes").unwrap(),
        content.find("data.txt").unwrap(),
        content.find("nested.log").unwrap(),
    );
    assert!(hidden < data && data < nested, "not path-sorted: {content}");

    // And the notice names the survivor.
    let err = stderr(&out);
    assert!(err.contains("output copied in-order to") && err.contains("_sorted"), "{err}");
}

#[test]
fn ggg_forces_regex_save_and_delve_together() {
    let tree = Tree::build("ggg");
    // A real Matroska fixture: its subtitle track holds "…take it personally…", reachable only
    // through `--delve` decoding.
    fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/phm.mkv"),
        tree.root.join("phm.mkv"),
    )
    .unwrap();
    let cwd = Tree::empty("ggg_cwd");
    // The dotted pattern only hits "personally" as a regex — so a hit proves `-E` was forced, its
    // location inside the mkv proves `--delve` was forced, and the file left proves `--save` was.
    let out = bashrs(&["GGG", "person.lly", "-d", tree.path()], Some(&cwd.root), None);
    let text = stdout(&out);
    assert!(text.contains("phm.mkv") && text.contains("personally"), "{text}");
    let files: Vec<PathBuf> = fs::read_dir(&cwd.root).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(files.len(), 1, "expected just the sorted file: {files:?}");
    assert!(files[0].to_string_lossy().ends_with("_sorted"), "{files:?}");
    assert!(fs::read_to_string(&files[0]).unwrap().contains("personally"));
}

#[test]
fn pinned_variants_reject_their_pinned_flag_but_forced_flags_stay_accepted() {
    let tree = Tree::build("pinned");
    // `gg2` pins -C 2; passing -C used to be silently overridden — now it's a parse error.
    let out = bashrs(&["gg2", "-C", "5", "match", "-d", tree.path()], None, None);
    assert!(!out.status.success(), "gg2 -C must be rejected");
    assert!(stderr(&out).contains("unexpected argument"), "{}", stderr(&out));
    let out = bashrs(&["g2", "-C", "5", "match"], None, Some(b"the match line\n"));
    assert!(!out.status.success(), "g2 -C must be rejected");
    // GG/GGG merely HIDE their forced flags — passing one is an accepted no-op.
    let out = bashrs(&["GG", "--delve", "zzz_no_such_term", "-d", tree.path()], None, None);
    assert!(out.status.success(), "GG --delve should stay accepted: {}", stderr(&out));
}

#[test]
fn complete_flags_answers_per_variant_at_tab_time() {
    // What the generated `_bashrs_complete` runs on TAB: flags for the exact wrapper name.
    let gg = stdout(&bashrs(&["complete-flags", "gg"], None, None));
    assert!(gg.contains("--context") && gg.contains("--delve") && gg.contains("--help"), "{gg}");
    let gg2 = stdout(&bashrs(&["complete-flags", "gg2"], None, None));
    assert!(!gg2.contains("--context"), "gg2 must not offer its pinned -C: {gg2}");
    let ggg = stdout(&bashrs(&["complete-flags", "GGG"], None, None));
    assert!(!ggg.contains("--delve") && ggg.contains("--context"), "{ggg}");
}

#[test]
fn dash_leading_terms_search_via_the_e_flag() {
    let tree = Tree::build("eflag");
    fs::write(tree.root.join("flags.txt"), "rsync --backup-dir=/mnt daily\n").unwrap();
    // gg: `-e` protects the dash-leading expression; positional and `-e` terms are OR'd.
    let text = stdout(&bashrs(&["gg", "-e", "--backup-dir", "-d", tree.path()], None, None));
    assert!(text.contains("flags.txt:1:rsync --backup-dir"), "{text}");
    let both = stdout(&bashrs(&["gg", "nested", "-e", "--backup-dir", "-d", tree.path()], None, None));
    assert!(both.contains("flags.txt") && both.contains("nested.log"), "{both}");
    // g: with `-e`, the first positional becomes the INPUT (grep semantics)…
    let file = tree.root.join("flags.txt");
    let g = stdout(&bashrs(&["g", "-e", "--backup-dir", file.to_str().unwrap()], None, None));
    assert!(g.contains("--backup-dir=/mnt"), "{g}");
    // …repeated `-e` terms are OR'd…
    let multi = stdout(&bashrs(&["g", "-e", "alpha", "-e", "gamma"], None, Some(b"alpha\nbeta\ngamma\n")));
    assert!(multi.contains("alpha") && multi.contains("gamma") && !multi.contains("beta"), "{multi}");
    // …and two positionals alongside `-e` can't both be inputs.
    let err = bashrs(&["g", "-e", "x", "a.txt", "b.txt"], None, None);
    assert!(stderr(&err).contains("at most one input"), "{}", stderr(&err));
    // hg joins the party.
    let hg = stdout(&bashrs(&["hg", "-e", "--backup-dir"], None, Some(b"  501  ls\n  502  rsync --backup-dir=/m x\n")));
    assert!(hg.contains("rsync") && !hg.contains("501"), "{hg}");
}

#[test]
fn g_family_filters_stdin_with_numbers_context_invert_and_regex() {
    let input: &[u8] = b"above\nthe match line\nbelow\n";
    let numbered = stdout(&bashrs(&["g", "-n", "-C", "1", "match"], None, Some(input)));
    assert!(numbered.contains("2:the match line"), "{numbered}");
    assert!(numbered.contains("1-above") && numbered.contains("3-below"),
        "grep-style dash-separated context: {numbered}");
    let inverted = stdout(&bashrs(&["g", "-v", "match"], None, Some(input)));
    assert!(inverted.contains("above") && inverted.contains("below") && !inverted.contains("match line"),
        "-v keeps only the non-matching lines: {inverted}");
    let regex = stdout(&bashrs(&["g", "-E", "mat.h"], None, Some(input)));
    assert!(regex.contains("the match line"), "-E: the dot should match any character: {regex}");
    let pinned = stdout(&bashrs(&["g2", "match"], None, Some(input)));
    assert!(pinned.contains("above") && pinned.contains("below"), "g2 pins -C 2 by itself: {pinned}");
}

#[test]
fn hg_filters_piped_history_lines() {
    // The generated wrapper pipes `history` in; here the pipe is simulated on stdin.
    let history: &[u8] = b"  501  ls -la\n  502  cargo build\n";
    let text = stdout(&bashrs(&["hg", "cargo"], None, Some(history)));
    assert!(text.contains("502  cargo build") && !text.contains("ls -la"), "{text}");
}

#[test]
fn gg_re_previews_but_refuses_to_modify_non_interactively() {
    // `--re` is destructive and undo-less, so it only mutates on an interactive `y`. Here stdin is
    // a pipe (never a tty) carrying "y" — the gate must STILL refuse and leave the tree untouched.
    let tree = Tree::empty("re");
    fs::write(tree.root.join("finger.txt"), b"a fin here").unwrap();
    let out = bashrs(&["gg", "fin", "--re", "lon"], Some(&tree.root), Some(b"y\n"));
    let err = stderr(&out);
    assert!(err.contains("--re would replace"), "a preview is shown first: {err}");
    assert!(err.contains("finger.txt → "), "the rename is previewed: {err}");
    assert!(err.contains("refusing to modify files non-interactively"), "and it refuses: {err}");
    // Nothing on disk changed — neither name nor content.
    assert!(tree.root.join("finger.txt").exists(), "not renamed");
    assert!(!tree.root.join("longer.txt").exists(), "no renamed twin appeared");
    assert_eq!(fs::read(tree.root.join("finger.txt")).unwrap(), b"a fin here", "content untouched");
}
