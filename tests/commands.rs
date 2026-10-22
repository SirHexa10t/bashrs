//! End-to-end tests of assorted non-search commands, driving the real binary.


use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Run the real `bashrs` with `args` and extra environment overrides.
fn bashrs(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bashrs"));
    cmd.args(args);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("run bashrs")
}

/// Run the real `bashrs` with `args` from `cwd`.
fn bashrs_in(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bashrs")).args(args).current_dir(cwd).output().expect("run bashrs")
}

#[test]
fn brs_configure_creates_the_config_from_the_template_and_opens_it() {
    // A private HOME, so the test can never touch the user's real ~/.bashrs.
    let home = std::env::temp_dir().join(format!("bashrs_cfg_home_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();

    // First use: the file is created from the template, then opened via $EDITOR (cat prints it).
    let out = bashrs(&["brs_configure"], &[("HOME", home.to_str().unwrap()), ("EDITOR", "cat")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let shown = String::from_utf8_lossy(&out.stdout);
    // Fields must be present with SOME boolean value; the defaults themselves are the
    // template's business (the unit test checks legality by parsing the TOML).
    assert!(
        shown.contains("[tools]")
            && shown.contains("always_bundle_languages = ")
            && shown.contains("always_bundle_utilities = "),
        "{shown}"
    );

    let file = home.join(".bashrs").join("configrs.toml");
    assert!(file.exists(), "config file must be created under $HOME/.bashrs");

    // A current-shaped file with edited VALUES is opened as-is — user settings survive. The
    // edit flips whichever value the template currently ships, so this test never cares what
    // the default is.
    let template = std::fs::read_to_string(&file).unwrap();
    let flipped = if template.contains("always_bundle_utilities = true") {
        template.replace("always_bundle_utilities = true", "always_bundle_utilities = false")
    } else {
        template.replace("always_bundle_utilities = false", "always_bundle_utilities = true")
    };
    let edited = format!("# my edited config\n{flipped}");
    std::fs::write(&file, &edited).unwrap();
    let again = bashrs(&["brs_configure"], &[("HOME", home.to_str().unwrap()), ("EDITOR", "cat")]);
    assert!(String::from_utf8_lossy(&again.stdout).contains("# my edited config"),
        "a current-shaped config must never be overwritten");

    // A file with an OUTDATED shape (missing template keys) is archived as `.old` + rewritten.
    std::fs::write(&file, "[tools]\nalways_bundle = true\n").unwrap();
    let migrated = bashrs(&["brs_configure"], &[("HOME", home.to_str().unwrap()), ("EDITOR", "cat")]);
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

/// A config the user symlinked (a dotfiles repo, say) survives the outdated-shape migration: the
/// fresh template goes into the file the link points at, the link is still a link, and the old
/// settings are archived beside it in `~/.bashrs` — where the shell's merge reminder looks.
/// Renaming used to move the link itself to the archive name, leaving a regular file behind.
#[test]
fn a_symlinked_config_is_migrated_through_the_link() {
    let home = std::env::temp_dir().join(format!("bashrs_cfg_link_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".bashrs")).unwrap();
    std::fs::create_dir_all(home.join("dotfiles")).unwrap();
    let dotfile = home.join("dotfiles/configrs.toml");
    std::fs::write(&dotfile, "[tools]\nalways_bundle = true\n").unwrap();
    let link = home.join(".bashrs/configrs.toml");
    std::os::unix::fs::symlink(&dotfile, &link).unwrap();

    let out = bashrs(&["brs_configure"], &[("HOME", home.to_str().unwrap()), ("EDITOR", "cat")]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("archived"), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(link.is_symlink(), "the link must survive the migration");
    assert!(std::fs::read_to_string(&dotfile).unwrap().contains("always_bundle_languages"),
        "the fresh template goes where the link points");
    let olds: Vec<_> = std::fs::read_dir(home.join(".bashrs")).unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".old"))
        .collect();
    assert_eq!(olds.len(), 1, "archived once, in ~/.bashrs");
    assert!(!olds[0].path().is_symlink(), "the archive is a copy, not the moved link");
    assert!(std::fs::read_to_string(olds[0].path()).unwrap().contains("always_bundle = true"),
        "and it holds the old settings");
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

/// Every media command that writes a file, as `(command, its arguments, the file they write)`,
/// each run from one directory of synthesized inputs — with `-y` appended, or without.
const MEDIA_WRITERS: &[(&str, &[&str], &str)] = &[
    ("media_convert", &["tone.wav", "convert.wav"], "convert.wav"),
    ("media_convert_quality", &["tone.wav", "quality.flac"], "quality.flac"),
    ("media_convert_compact", &["tone.wav", "compact.flac"], "compact.flac"),
    ("media_trim", &["tone.wav", "0.2", "--output", "trim.wav"], "trim.wav"),
    ("media_remove_vocals", &["tone.wav", "devocal.wav"], "devocal.wav"),
    ("media_hmerge_imgs", &["--output", "hmerge.png", "red.png", "blue.png"], "hmerge.png"),
    ("media_vmerge_imgs", &["--output", "vmerge.png", "red.png", "blue.png"], "vmerge.png"),
];

#[test]
fn every_command_offering_overwrite_has_a_media_writers_row() {
    // The CLI's own definition says which commands take `-y`, so none can be added without the
    // test below exercising it — and no row outlives its command.
    use clap::CommandFactory;
    let offering: std::collections::BTreeSet<String> = bashrs::cli::Cli::command()
        .get_subcommands()
        .filter(|command| command.get_arguments().any(|arg| arg.get_long() == Some("overwrite")))
        .map(|command| command.get_name().to_string())
        .collect();
    let rows = MEDIA_WRITERS.iter().map(|(command, ..)| command.to_string()).collect();
    assert_eq!(offering, rows);
}

#[test]
fn media_commands_leave_an_existing_output_alone_unless_told_to_overwrite() {
    // Each MEDIA_WRITERS row runs twice against a stale output: refused and untouched without
    // `-y`, replaced with it. Needs a real ffmpeg/ffprobe, looked up on PATH — where a bashrs
    // shell puts the bundled copy first (`~/.bashrs/tools/bin`), the one the commands run.
    let works = ["ffmpeg", "ffprobe"]
        .iter()
        .all(|tool| Command::new(tool).arg("-version").output().is_ok_and(|out| out.status.success()));
    if !works {
        eprintln!("SKIPPED media overwrite: no ffmpeg/ffprobe on PATH");
        return;
    }
    let dir = std::env::temp_dir().join(format!("bashrs_media_overwrite_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (source, shape, file) in [
        ("sine=d=1", &["-ac", "2"][..], "tone.wav"), // stereo, as vocal removal requires
        ("color=c=red:s=8x8", &["-frames:v", "1", "-update", "1"], "red.png"),
        ("color=c=blue:s=8x8", &["-frames:v", "1", "-update", "1"], "blue.png"),
    ] {
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", source])
            .args(shape)
            .arg(file)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        assert!(made, "could not synthesize {file}");
    }
    let run = |command: &str, args: &[&str], overwrite: bool| {
        Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .arg(command)
            .args(args)
            .args(overwrite.then_some("-y"))
            .current_dir(&dir)
            .output()
            .expect("run bashrs")
    };

    for &(command, args, output) in MEDIA_WRITERS {
        let output = dir.join(output);
        std::fs::write(&output, "stale").unwrap();
        let refused = run(command, args, false);
        assert!(!refused.status.success(), "{command} without -y must refuse");
        assert_eq!(std::fs::read(&output).unwrap(), b"stale", "{command} without -y must leave the file alone");
        let replaced = run(command, args, true);
        assert!(replaced.status.success(), "{command} -y: {}", String::from_utf8_lossy(&replaced.stderr));
        assert!(std::fs::read(&output).unwrap() != b"stale", "{command} -y must replace the file");
    }
    let _ = std::fs::remove_dir_all(&dir);
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

#[test]
fn offline_provisioning_hands_its_problems_to_the_file_compile_sh_prints_last() {
    // COMPILE.sh's contract with install-stainless, run for real with nowhere to connect to.
    let home = std::env::temp_dir().join(format!("bashrs_offline_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let problems = home.join("problems");
    std::fs::write(&problems, "").unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let (ok, said) = install_stainless_offline(&home, &path, &[OsStr::new("--problems-file"), problems.as_os_str()]);

    let written = std::fs::read_to_string(&problems).unwrap();
    assert!(ok, "offline is no failure of the compile: {said}");
    assert!(written.contains("Provisioning problems"), "{written}");
    assert!(
        written.lines().any(|line| line.contains("uv") && line.contains("could not reach GitHub")),
        "a tool lookup, with its reason: {written}"
    );
    assert!(written.contains("tools: could not create the python environment"), "{written}");
    assert!(written.lines().any(|line| line.contains("stainless/") && line.contains("git failed")), "{written}");
    // Each problem was also printed where it happened; the summary itself went to the file.
    assert!(!said.contains("Provisioning problems"), "{said}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn a_tool_is_looked_up_until_the_system_has_its_own_and_then_its_bundle_goes() {
    // Every `always_bundle_*` setting off, with an earlier compile's bundles still in place, shims
    // and all. While its shim is the only ffmpeg on PATH, that is no system copy: the bundle stays
    // and is looked up, as from any shell. Once the system has its own (a stand-in), no lookup is
    // made for it, and the bundle goes; so does uv's, a language's, once the system has uv. Two
    // bundles stay whatever the settings say: the python environment, which holds the user's
    // packages, and git-lfs, an extra the system lacks — the only copy there is.
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::temp_dir().join(format!("bashrs_system_has_it_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let tools = home.join(".bashrs/tools");
    let template =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/conf/templates/configrs.toml")).unwrap();
    let config = template
        .replace("always_bundle_utilities = true", "always_bundle_utilities = false")
        .replace("always_bundle_languages = true", "always_bundle_languages = false");
    assert_eq!(config.matches("= false").count(), template.matches("= false").count() + 2, "both settings still exist to turn off");
    let os_bin = home.join("os/bin");
    for dir in ["ffmpeg/bin", "python/bin", "uv", "git-lfs", "bin"].map(|dir| tools.join(dir)).into_iter().chain([os_bin.clone()]) {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(home.join(".bashrs/configrs.toml"), config).unwrap();
    std::fs::write(tools.join("python/pyvenv.cfg"), "home = /somewhere\n").unwrap();
    for (program, body) in [
        (tools.join("ffmpeg/bin/ffmpeg"), "#!/bin/sh\n"),
        (tools.join("ffmpeg/bin/ffprobe"), "#!/bin/sh\n"),
        (tools.join("uv/uv"), "#!/bin/sh\n"),
        (tools.join("uv/uvx"), "#!/bin/sh\n"),
        (tools.join("git-lfs/git-lfs"), "#!/bin/sh\n"),
        (tools.join("python/bin/python3"), "#!/bin/sh\necho 'Python 3.14.6'\n"),
        (os_bin.join("python3"), "#!/bin/sh\necho 'Python 3.14.6'\n"),
    ] {
        std::fs::write(&program, body).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for (name, bundled) in
        [("ffmpeg", "ffmpeg/bin/ffmpeg"), ("ffprobe", "ffmpeg/bin/ffprobe"), ("uv", "uv/uv"), ("uvx", "uv/uvx"), ("git-lfs", "git-lfs/git-lfs")]
    {
        std::os::unix::fs::symlink(Path::new("..").join(bundled), tools.join("bin").join(name)).unwrap();
    }
    std::fs::write(
        tools.join("ffmpeg/.source_url"),
        "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n9.0-latest-linux64-gpl-9.0.tar.xz",
    )
    .unwrap();
    // The inherited PATH minus every directory with an ffmpeg or a git-lfs of its own — a real
    // system copy, or the real ~/.bashrs's shim — so the only ones left are this home's bundles,
    // or the stand-ins.
    let inherited: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|dir| !dir.join("ffmpeg").exists() && !dir.join("git-lfs").exists())
        .collect();
    let path_with = |first: &[PathBuf]| std::env::join_paths(first.iter().chain(&inherited)).unwrap();
    let looked_up = |said: &str| said.lines().any(|line| line.contains("recorded ffmpeg pin") || line.contains("newer ffmpeg build"));

    let (ok, said) = install_stainless_offline(&home, &path_with(&[os_bin.clone(), tools.join("bin")]), &[]);
    assert!(ok && looked_up(&said), "the shim is the bundle, not the system's — so it is kept current: {said}");
    assert!(tools.join("ffmpeg/bin/ffmpeg").exists(), "and kept: {said}");

    for program in ["ffmpeg", "uv"] {
        std::fs::write(os_bin.join(program), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(os_bin.join(program), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (ok, said) = install_stainless_offline(&home, &path_with(&[tools.join("bin"), os_bin.clone()]), &[]);
    assert!(ok, "{said}");
    let os_ffmpeg = os_bin.join("ffmpeg");
    assert!(said.contains(&format!("tools: ffmpeg is already on this system ({}) — not bundling", os_ffmpeg.display())), "{said}");
    assert!(!looked_up(&said), "no lookup for what the system has: {said}");
    for (bundle, shim) in [("ffmpeg", "ffmpeg"), ("uv", "uv"), ("uv", "uvx")] {
        assert!(tools.join(bundle).symlink_metadata().is_err(), "the old {bundle} bundle is gone: {said}");
        assert!(tools.join("bin").join(shim).symlink_metadata().is_err(), "and its {shim} shim with it");
    }
    assert!(os_ffmpeg.exists() && os_bin.join("uv").exists(), "the system's own are never touched");
    assert!(tools.join("python/bin/python3").exists(), "the python environment stays — the user's packages: {said}");
    assert!(tools.join("git-lfs/git-lfs").exists(), "the system lacks git-lfs, so its bundle stays: {said}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn the_install_moves_the_old_user_data_to_the_new_homes() {
    // The real install-shell, over the layout from before the split, in a private HOME with no rc
    // files to wire and fish never run: what the apps keep ends up in user-appdata, the user's
    // binds profiles in user-custom, and the install says what it moved.
    let home = std::env::temp_dir().join(format!("bashrs_user_dirs_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let old = home.join(".bashrs/user-data");
    std::fs::create_dir_all(old.join("sequencer")).unwrap();
    std::fs::create_dir_all(old.join("browser_cookies/youtube")).unwrap();
    std::fs::write(old.join("sequencer/binds.toml"), "[binds]").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .arg("install-shell")
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("run bashrs");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{said}");
    let bashrs = home.join(".bashrs");
    assert!(bashrs.join("user-appdata/browser_cookies/youtube").is_dir(), "{said}");
    assert_eq!(std::fs::read_to_string(bashrs.join("user-custom/sequencer/binds.toml")).unwrap(), "[binds]", "{said}");
    assert!(old.symlink_metadata().is_err(), "the old name is gone: {said}");
    assert!(said.contains("Moved ") && said.contains("user-custom/sequencer"), "it says so: {said}");
    let _ = std::fs::remove_dir_all(&home);
}

/// `install-stainless --use-stable-carstay`, run for real in the private `home` with `path` as its
/// `PATH` and nowhere to connect to: every proxy variable aimed at a closed port, and nothing
/// routing around it — NO_PROXY goes, and so does the system git config, which could name a proxy
/// of its own. Stable mode is the one that never rewrites the project's Carstay.toml; were it ever
/// to, the file is put back before the assertion says so. Returns whether the run succeeded, and
/// everything it printed.
fn install_stainless_offline(home: &Path, path: &OsStr, extra: &[&OsStr]) -> (bool, String) {
    let carstay = Path::new(env!("CARGO_MANIFEST_DIR")).join("Carstay.toml");
    let recorded = std::fs::read(&carstay).ok();
    let dead = "http://127.0.0.1:9";
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["install-stainless", "--use-stable-carstay"])
        .args(extra)
        .env("HOME", home)
        .env("PATH", path)
        .envs(["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY", "ALL_PROXY"].map(|var| (var, dead)))
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("UV_HTTP_RETRIES", "0") // uv's retries would stretch one refused download into seconds
        .output()
        .expect("run bashrs");
    let now = std::fs::read(&carstay).ok();
    if let (Some(bytes), true) = (&recorded, now != recorded) {
        let _ = std::fs::write(&carstay, bytes);
    }
    assert_eq!(now, recorded, "the project's Carstay.toml must be left exactly as it was");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// A fresh project directory with an empty sibling to run from: whatever a `pro_*` command then
/// finds can only have come from the project named on its command line. Returns
/// `(parent, project, elsewhere)`; the test removes `parent` when it is done.
fn project_beside_an_empty_dir(test: &str) -> (PathBuf, PathBuf, PathBuf) {
    let parent = std::env::temp_dir().join(format!("bashrs_{test}_{}", std::process::id()));
    let (project, elsewhere) = (parent.join("proj"), parent.join("elsewhere"));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    (parent, project, elsewhere)
}

#[test]
fn pro_todo_reads_the_project_it_is_given_not_the_directory_it_runs_in() {
    let (parent, project, elsewhere) = project_beside_an_empty_dir("pro_todo");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("TODO.md"), "buy milk\n").unwrap();
    std::fs::write(project.join("src/main.rs"), "// TODO: wire it up\n").unwrap();
    let out = bashrs_in(&elsewhere, &["pro_todo", project.to_str().unwrap()]);
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(shown.contains("buy milk"), "the project's TODO file, printed whole: {shown}");
    assert!(shown.contains("wire it up"), "the project's TODO mention: {shown}");
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn pro_readme_shows_the_project_it_is_given_not_the_directory_it_runs_in() {
    let (parent, project, elsewhere) = project_beside_an_empty_dir("pro_readme");
    std::fs::write(project.join("README.md"), "# proj\nDoes one thing well.\n").unwrap();
    let out = bashrs_in(&elsewhere, &["pro_readme", project.to_str().unwrap()]);
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(shown.contains("Does one thing well."), "the project's README: {shown}");
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn pro_compile_builds_the_project_it_is_given_not_the_directory_it_runs_in() {
    // Make leaves a mark without any compiler: the recipe touches a file, which only a build
    // run in the named project can put there.
    if !Command::new("make").arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("SKIPPED pro_compile project dir: no make on this machine");
        return;
    }
    let (parent, project, elsewhere) = project_beside_an_empty_dir("pro_compile");
    std::fs::write(project.join("Makefile"), "all:\n\ttouch built\n").unwrap();
    let out = bashrs_in(&elsewhere, &["pro_compile", project.to_str().unwrap()]);
    let all =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(project.join("built").exists(), "the build ran in the named project: {all}");
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn pro_update_packages_acts_on_the_project_it_is_given_not_the_directory_it_runs_in() {
    // A Makefile project's update step is a note, not a package manager, so nothing is run or
    // installed — and its toolchain is detected only if the named directory is the one read.
    let (parent, project, elsewhere) = project_beside_an_empty_dir("pro_update_packages");
    std::fs::write(project.join("Makefile"), "all:\n").unwrap();
    let out = bashrs_in(&elsewhere, &["pro_update_packages", project.to_str().unwrap()]);
    let all =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(all.contains("Detected C/C++ (Make)"), "the named project's toolchain: {all}");
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn diff_prints_gits_own_diff_of_two_files_and_exits_as_git_does() {
    let dir = std::env::temp_dir().join(format!("bashrs_diff_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (old, new) = (dir.join("old.txt"), dir.join("new.txt"));
    std::fs::write(&old, "same\nlet x = 1;\n").unwrap();
    std::fs::write(&new, "same\nlet x = 2;\n").unwrap();
    let path = |file: &Path| file.to_str().unwrap().to_owned();

    // Coloured even into a pipe — and under the colour, byte for byte what git prints, with
    // git's exit code: 1, "they differ".
    let out = bashrs(&["diff", &path(&old), &path(&new)], &[]);
    let git = Command::new("git")
        .args(["diff", "--no-index", "--no-ext-diff", "--color=never", "--"])
        .args([&old, &new])
        .output()
        .expect("run git");
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(shown.contains("\x1b["), "colour is on, terminal or not: {shown}");
    let plain = console::strip_ansi_codes(&shown);
    assert!(plain.contains("\n-let x = 1;\n+let x = 2;\n"), "a real diff: {plain}");
    assert_eq!(plain.as_bytes(), git.stdout);

    // `-n` puts each line's number in both files before it: `same` is line 1 of both, the removed
    // line is 2 of the old file only, its replacement 2 of the new file only.
    let numbered = bashrs(&["diff", "-n", &path(&old), &path(&new)], &[]);
    let numbered = console::strip_ansi_codes(&String::from_utf8_lossy(&numbered.stdout)).into_owned();
    assert!(numbered.contains("\n1 1 │ same\n2   │-let x = 1;\n  2 │+let x = 2;\n"), "{numbered}");

    let same = bashrs(&["diff", &path(&old), &path(&old)], &[]);
    assert_eq!((same.status.code(), same.stdout.as_slice()), (Some(0), &b""[..]), "a match is silent");

    // A side that does not exist is git's to report — and never a match.
    let missing = bashrs(&["diff", &path(&old), &path(&dir.join("absent"))], &[]);
    let complaint = String::from_utf8_lossy(&missing.stderr);
    assert_ne!(missing.status.code(), Some(0), "{complaint}");
    assert!(complaint.contains("absent"), "git names the missing side: {complaint}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diff_reads_a_dash_side_from_stdin() {
    let dir = std::env::temp_dir().join(format!("bashrs_diff_stdin_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let new = dir.join("new.txt");
    std::fs::write(&new, "same\nlet x = 2;\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["diff", "-", new.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run bashrs");
    // Written, then dropped: the pipe closes and git reads to its end.
    child.stdin.take().unwrap().write_all(b"same\nlet x = 1;\n").unwrap();
    let out = child.wait_with_output().unwrap();
    let shown = console::strip_ansi_codes(&String::from_utf8_lossy(&out.stdout)).into_owned();
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(shown.contains("\n-let x = 1;\n+let x = 2;\n"), "stdin reached git as the old side: {shown}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diff_compares_two_directories_one_level_deep_unless_recursive() {
    let dir = std::env::temp_dir().join(format!("bashrs_diff_dirs_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (old, new) = (dir.join("a"), dir.join("b"));
    for (file, text) in [
        (old.join("f.txt"), "let x = 1;\n"), (new.join("f.txt"), "let x = 2;\n"),
        (old.join("sub/deep.txt"), "old\n"), (new.join("sub/deep.txt"), "new\n"),
        (new.join("only.txt"), "new\n"),
    ] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let path = |side: &Path| side.to_str().unwrap().to_owned();
    let shown = |out: &Output| console::strip_ansi_codes(&String::from_utf8_lossy(&out.stdout)).into_owned();

    // One level, GNU's way: git's diff for the pair of files, GNU's lines for the rest.
    let shallow = bashrs(&["diff", &path(&old), &path(&new)], &[]);
    let text = shown(&shallow);
    assert_eq!(shallow.status.code(), Some(1), "{}", String::from_utf8_lossy(&shallow.stderr));
    assert!(text.contains("\n-let x = 1;\n+let x = 2;\n"), "{text}");
    assert!(text.contains(&format!("Only in {}: only.txt\n", path(&new))), "{text}");
    assert!(text.contains(&format!("Common subdirectories: {}/sub and {}/sub\n", path(&old), path(&new))), "{text}");
    assert!(!text.contains("deep.txt"), "not walked without -r: {text}");

    // `-r`: the subdirectory is compared where it was listed.
    let deep = bashrs(&["diff", "-r", &path(&old), &path(&new)], &[]);
    let text = shown(&deep);
    assert_eq!(deep.status.code(), Some(1), "{}", String::from_utf8_lossy(&deep.stderr));
    assert!(text.contains("\n-old\n+new\n") && !text.contains("Common subdirectories"), "{text}");

    let same = bashrs(&["diff", "-r", &path(&old), &path(&old)], &[]);
    assert_eq!((same.status.code(), same.stdout.as_slice()), (Some(0), &b""[..]), "a match is silent");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `lll` and the `cd` listing read a device as the real `ls` writes it: `/dev/null`, character
/// device 1, 3 on every Linux — its numbers its size, then its date, then its name. (Taken for
/// one word, the size once pushed the date into the name's place.) In a `/dev` whose minors vary
/// in width, `ls` pads between the numbers too, which must not split them either.
#[test]
fn lll_and_the_cd_listing_read_a_device_as_ls_writes_it() {
    let plain = |out: &Output| console::strip_ansi_codes(&String::from_utf8_lossy(&out.stdout)).into_owned();
    let listing = plain(&bashrs(&["lll", "/dev"], &[]));
    let row = listing.lines().find(|row| row.ends_with("  null")).unwrap_or_else(|| panic!("no `null` row:\n{listing}"));
    let cells: Vec<&str> = row.split("  ").map(str::trim).filter(|cell| !cell.is_empty()).collect();
    let date = |cell: &str| cell.len() == 19 && cell.as_bytes()[4] == b'-' && cell.as_bytes()[10] == b'_';
    assert!(cells[5] == "1, 3" && date(cells[6]) && cells[7] == "null", "size, date, name: {row}");
    let landing = Command::new(env!("CARGO_BIN_EXE_bashrs")).arg("cd-listing").current_dir("/dev").output().expect("cd-listing");
    assert!(plain(&landing).lines().any(|name| name == "null"), "{}", plain(&landing));
}

/// Compile-time guard that the fixture path stays valid if this file grows path-dependent tests.
#[allow(dead_code)]
fn _manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}
