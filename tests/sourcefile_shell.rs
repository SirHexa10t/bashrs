//! Behavioral tests of the generated `sourcefile.sh`'s shell surface, run through a real bash —
//! the parts unit tests can only assert as strings (function precedence, PATH idempotence).
//! Kept environment-independent: nothing here requires the bundled tools to actually exist.

use std::process::Command;

/// The `# bundled tools` section of the real generated sourcefile.
fn tools_section() -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs")).arg("generate").output().expect("generate");
    let script = String::from_utf8_lossy(&out.stdout).into_owned();
    let start = script.find("# bundled tools").expect("section missing");
    script[start..].lines().take(4).collect::<Vec<_>>().join("\n")
}

/// Run `script` in bash and return its stdout.
fn bash(script: &str) -> String {
    let out = Command::new("bash").arg("-c").arg(script).output().expect("bash");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn the_python3_function_replaces_earlier_rc_definitions() {
    // A leftover `python3` function from the user's own rc, then the sourcefile sourced last —
    // the exact shadowing that once resurfaced a stale pydev definition. The bashrs function must
    // end up defined (not the old one), routing through the shim dir.
    let script = format!(
        "python3() {{ echo OLD RC FUNCTION; }}\n{}\ntype python3 | head -1\ndeclare -f python3",
        tools_section()
    );
    let out = bash(&script);
    assert!(out.contains("python3 is a function"), "{out}");
    assert!(out.contains(".bashrs/tools/bin/python3"), "must route through the shim dir: {out}");
    assert!(!out.contains("OLD RC FUNCTION"), "the old definition must be fully replaced: {out}");
}

#[test]
fn the_python3_function_survives_an_active_alias() {
    // With an alias active, bash alias-expands a function's NAME at parse time — the reason the
    // sourcefile's `unalias` sits on its own line. Alias expansion needs to be on, as in
    // interactive shells.
    let script = format!(
        "shopt -s expand_aliases\nalias python3='/usr/bin/python3'\n{}\ntype python3 | head -1",
        tools_section()
    );
    let out = bash(&script);
    assert!(out.contains("python3 is a function"), "alias shadow must be cleared first: {out}");
}

#[test]
fn the_path_prepend_is_idempotent_across_resources() {
    // Sourcing the section twice (nested shells re-source the rc) must not stack PATH entries.
    let section = tools_section();
    let script = format!(
        "{section}\n{section}\nprintf '%s' \":$PATH:\" | grep -o ':[^:]*\\.bashrs/tools/bin:' | wc -l"
    );
    assert_eq!(bash(&script).trim(), "1");
}
