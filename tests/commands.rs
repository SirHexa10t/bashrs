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

/// Compile-time guard that the fixture path stays valid if this file grows path-dependent tests.
#[allow(dead_code)]
fn _manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}
