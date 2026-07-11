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
fn py_evaluates_an_expression_on_the_resolved_python() {
    // Quoted or word-split, the expression lands in python's print().
    let out = bashrs(&["py", "2**10"], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1024");
    let words = bashrs(&["py", "'a'", "*", "3"], &[]);
    assert_eq!(String::from_utf8_lossy(&words.stdout).trim(), "aaa");
}

#[test]
fn py_reports_python_errors_instead_of_pretending() {
    let out = bashrs(&["py", "1/0"], &[]);
    assert!(!out.status.success() || !out.stderr.is_empty(), "a python error must surface");
    assert!(String::from_utf8_lossy(&out.stderr).contains("ZeroDivisionError"),
        "{}", String::from_utf8_lossy(&out.stderr));
}

/// Compile-time guard that the fixture path stays valid if this file grows path-dependent tests.
#[allow(dead_code)]
fn _manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}
