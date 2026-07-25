//! End-to-end tests of assorted non-search commands, driving the real binary.

use std::path::Path;
use std::process::{Command, Output};

/// Run the real `bashrs` with `args` and extra environment overrides.
fn bashrs(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bashrs"));
    cmd.args(args);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("run bashrs")
}

#[test]
fn bashrs_configure_creates_the_config_from_the_template_and_opens_it() {
    // A private HOME, so the test can never touch the user's real ~/.bashrs.
    let home = std::env::temp_dir().join(format!("bashrs_cfg_home_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();

    // First use: the file is created from the template, then opened via $EDITOR (cat prints it).
    let out = bashrs(&["bashrs_configure"], &[("HOME", home.to_str().unwrap()), ("EDITOR", "cat")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(
        shown.contains("[tools]")
            && shown.contains("always_bundle_languages = true")
            && shown.contains("always_bundle_utilities = false"),
        "{shown}"
    );

    let file = home.join(".bashrs").join("configrs.toml");
    assert!(file.exists(), "config file must be created under $HOME/.bashrs");

    // A current-shaped file with edited VALUES is opened as-is — user settings survive.
    let template = std::fs::read_to_string(&file).unwrap();
    let edited = format!("# my edited config\n{}", template.replace("always_bundle_utilities = false", "always_bundle_utilities = true"));
    std::fs::write(&file, &edited).unwrap();
    let again = bashrs(&["bashrs_configure"], &[("HOME", home.to_str().unwrap()), ("EDITOR", "cat")]);
    assert!(String::from_utf8_lossy(&again.stdout).contains("# my edited config"),
        "a current-shaped config must never be overwritten");

    // A file with an OUTDATED shape (missing template keys) is archived as `.old` + rewritten.
    std::fs::write(&file, "[tools]\nalways_bundle = true\n").unwrap();
    let migrated = bashrs(&["bashrs_configure"], &[("HOME", home.to_str().unwrap()), ("EDITOR", "cat")]);
    assert!(String::from_utf8_lossy(&migrated.stderr).contains("archived"), "{}", String::from_utf8_lossy(&migrated.stderr));
    assert!(String::from_utf8_lossy(&migrated.stdout).contains("always_bundle_languages"), "fresh template opened");
    let olds: Vec<_> = std::fs::read_dir(home.join(".bashrs")).unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".old"))
        .collect();
    assert_eq!(olds.len(), 1, "the outdated file must be archived, once");
    assert!(
        olds[0].file_name().to_string_lossy().starts_with("configrs.toml."),
        "the archive keeps the full original name: {:?}", olds[0].file_name()
    );
    assert!(std::fs::read_to_string(olds[0].path()).unwrap().contains("always_bundle = true"),
        "the user's old settings must survive inside the archive");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn fs_usage_count_prints_a_bare_number_and_fails_on_missing_paths() {
    let dir = std::env::temp_dir().join(format!("bashrs_usage_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("nested")).unwrap();
    std::fs::write(dir.join("one.txt"), "x").unwrap();
    std::fs::write(dir.join("nested/two.txt"), "y").unwrap();
    let out = bashrs(&["fs_usage", "--count", dir.to_str().unwrap()], &[]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "2", "recursive, files only");
    let missing = bashrs(&["fs_usage", "--count", "/no/such/dir"], &[]);
    assert!(!missing.status.success(), "a missing path must exit non-zero");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dl_page_links_finds_resolves_and_downloads_from_a_local_page() {
    // Fully offline end-to-end: curl reads file:// pages and downloads file:// links, so the
    // whole scan → resolve → fetch pipeline runs against a local fixture site.
    let base = std::env::temp_dir().join(format!("bashrs_dl_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("site/assets")).unwrap();
    std::fs::create_dir_all(base.join("out")).unwrap();
    std::fs::write(base.join("site/assets/song.mp3"), "MP3DATA").unwrap();
    std::fs::write(base.join("site/cover.png"), "PNGDATA").unwrap();
    std::fs::write(
        base.join("site/page.html"),
        r#"<html><a href="assets/song.mp3">song</a><img src="cover.png"><a href="skip.txt">no</a></html>"#,
    )
    .unwrap();
    let url = format!("file://{}/site/page.html", base.display());
    let dl = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .args(args)
            .current_dir(base.join("out"))
            .output()
            .expect("run bashrs")
    };

    let listed = dl(&["dl_page_links", &url, "mp3", ".PNG", "--list"]);
    let lines = String::from_utf8_lossy(&listed.stdout).into_owned();
    assert!(lines.contains("assets/song.mp3") && lines.contains("cover.png"), "{lines}");
    assert!(!lines.contains("skip.txt"), "unrequested types must be ignored: {lines}");
    assert!(std::fs::read_dir(base.join("out")).unwrap().next().is_none(), "--list downloads nothing");

    let run = dl(&["dl_page_links", &url, "mp3", "png"]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(std::fs::read_to_string(base.join("out/song.mp3")).unwrap(), "MP3DATA");
    assert_eq!(std::fs::read_to_string(base.join("out/cover.png")).unwrap(), "PNGDATA");

    let none = dl(&["dl_page_links", &url, "zip"]);
    assert!(!none.status.success(), "no matches at all must exit non-zero");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn media_convert_refuses_to_convert_a_file_onto_itself() {
    // The guard fires on the resolved paths alone — before ffmpeg, before any filesystem access.
    let out = bashrs(&["media_convert", "clip.mp4", "mp4"], &[]);
    assert!(!out.status.success(), "self-conversion must fail");
    assert!(String::from_utf8_lossy(&out.stderr).contains("the output is the input itself"));
}

#[test]
fn media_commands_propagate_failure_as_a_nonzero_exit() {
    // Deterministic with or without ffmpeg installed: a missing input fails either way, and the
    // command must pass that on (ffmpeg keeps warnings out of its exit status, so a clean run
    // with ignorable warnings still exits 0).
    let convert = bashrs(&["media_convert", "/no/such/clip.mp4", "/tmp/bashrs_never_written.mkv"], &[]);
    assert!(!convert.status.success(), "a failed conversion must exit non-zero");
    let metadata = bashrs(&["media_metadata", "/no/such/clip.mp4"], &[]);
    assert!(!metadata.status.success(), "an unreadable file must exit non-zero");
}

#[test]
fn pro_run_forwards_every_arg_to_the_program_except_its_own_help() {
    // A minimal crate whose program echoes its argv and cwd — whatever pro_run forwards must
    // come out the other side, one argv entry each (the spaced value proves the quoting-proof
    // "$@" ride; the spaced PROJECT path proves the {dir} substitution's quoting).
    let dir = std::env::temp_dir().join(format!("bashrs pro_run {}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"argecho\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.rs"),
        "fn main() { println!(\"ARGS={:?} CWD={:?}\", std::env::args().skip(1).collect::<Vec<_>>(), std::env::current_dir().unwrap()); }",
    )
    .unwrap();
    let run_in = |cwd: &Path, args: &[&str]| -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("run bashrs");
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), all)
    };
    let run = |args: &[&str]| run_in(&dir, args);

    // Everything — flags included — lands in the program's argv…
    let (ok, out) = run(&["pro_run", "-x", "--flag", "value with space"]);
    assert!(ok, "{out}");
    assert!(out.contains(r#"ARGS=["-x", "--flag", "value with space"]"#), "{out}");
    // …except -h/--help, which stay pro_run's own…
    let (ok, help) = run(&["pro_run", "-h"]);
    assert!(ok, "{help}");
    assert!(help.contains("Usage:") && !help.contains("ARGS="), "-h is pro_run's help: {help}");
    // …unless escaped past clap with `--`, which forwards even those.
    let (ok, out) = run(&["pro_run", "--", "-h"]);
    assert!(ok, "{out}");
    assert!(out.contains(r#"ARGS=["-h"]"#), "{out}");
    // `--h2` is the shorthand for that: it forwards a leading `-h` so the PROGRAM's help prints,
    // not pro_run's. It's pro_run's own flag, so it never reaches the program's argv itself.
    let (ok, out) = run(&["pro_run", "--h2"]);
    assert!(ok, "{out}");
    assert!(out.contains(r#"ARGS=["-h"]"#), "--h2 forwards -h to the program: {out}");

    // A leading --pdir picks the project from anywhere; the rest still forwards — and the
    // PROGRAM runs in the caller's own directory, not the project's (the regression that bit:
    // a cwd-sensitive program must behave exactly as if its binary were invoked directly).
    let neutral = std::env::temp_dir().canonicalize().unwrap();
    let (ok, out) = run_in(&neutral, &["pro_run", "--pdir", dir.to_str().unwrap(), "-x"]);
    assert!(ok, "{out}");
    assert!(out.contains(r#"ARGS=["-x"]"#), "ran via --pdir from elsewhere: {out}");
    assert!(
        out.contains(&format!("CWD={:?}", neutral)),
        "the program keeps the caller's cwd, not the project's: {out}"
    );
    // After the first forwarded token, even a `--pdir` belongs to the program.
    let (ok, out) = run(&["pro_run", "foo", "--pdir", "zzz"]);
    assert!(ok, "{out}");
    assert!(out.contains(r#"ARGS=["foo", "--pdir", "zzz"]"#), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn table_fancy_is_the_framed_terminal_width_table_preset() {
    // `table_fancy` must stay exactly `table -j " | " --split-lines --space-rows '-' --emit-frame`.
    // Both are run over the same input at a pinned $COLUMNS (the width --split-lines resolves to),
    // so the comparison is deterministic and pins the preset to the invocation it stands for.
    let input = "name  role  note\nada  pioneer  wrote the first algorithm for a machine\nbob  builder  yes";
    let width = ("COLUMNS", "60");
    let fancy = bashrs(&["table_fancy", input], &[width]);
    let spelled_out = bashrs(
        &["table", input, "-j", " | ", "--split-lines", "--space-rows", "-", "--emit-frame"],
        &[width],
    );
    assert!(fancy.status.success(), "{}", String::from_utf8_lossy(&fancy.stderr));
    let shown = String::from_utf8_lossy(&fancy.stdout);
    assert_eq!(shown, String::from_utf8_lossy(&spelled_out.stdout), "preset drifted from its flags");
    // …and that shape is: framed rows, none wider than the window, wrapped where needed.
    for line in shown.lines() {
        assert!(line.starts_with('|') || line.starts_with('-'), "framed or a rule: {line:?}");
        assert!(line.chars().count() <= 60, "wider than $COLUMNS: {line:?}");
    }
    assert!(shown.lines().count() > 3, "the long row wrapped: {shown}");
}

#[test]
fn pro_test_survives_a_relative_dir_across_its_two_steps() {
    // CMake is the build-then-test toolchain: pro_test cd's per step (process-global), and a
    // RELATIVE dir once resolved the second cd against the first's result (myproj/myproj) —
    // the build ran, then "cannot enter" killed ctest. Reproduced and fixed; this pins it.
    let works = Command::new("cmake").arg("--version").output().is_ok_and(|o| o.status.success())
        && Command::new("ctest").arg("--version").output().is_ok_and(|o| o.status.success());
    if !works {
        eprintln!("SKIPPED pro_test relative-dir: no cmake/ctest on this machine");
        return;
    }
    let parent = std::env::temp_dir().join(format!("bashrs_pro_test_{}", std::process::id()));
    let project = parent.join("cmproj");
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&project).unwrap();
    // Compiler-free on purpose (LANGUAGES NONE): the double-cd defect is about directories,
    // not compilation, and this keeps the test runnable on machines without a C toolchain.
    std::fs::write(
        project.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.16)\nproject(smoke LANGUAGES NONE)\nenable_testing()\nadd_test(NAME smoke COMMAND /bin/true)\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["pro_test", "cmproj"])
        .current_dir(&parent)
        .output()
        .expect("run bashrs");
    let all =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!all.contains("cannot enter"), "the second step must re-enter the SAME dir: {all}");
    assert!(all.contains("100% tests passed"), "ctest actually ran: {all}");
    let _ = std::fs::remove_dir_all(&parent);
}

/// Compile-time guard that the fixture path stays valid if this file grows path-dependent tests.
#[allow(dead_code)]
fn _manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}
