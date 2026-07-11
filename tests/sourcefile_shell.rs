//! Behavioral tests of the generated `sourcefile.sh`'s shell surface, run through a real bash —
//! the parts unit tests can only assert as strings (function precedence, PATH idempotence).
//! Kept environment-independent: nothing here requires the bundled tools to actually exist.

use std::process::Command;

/// The first `lines` of a `# `-headed section of the real generated sourcefile.
fn section(header: &str, lines: usize) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs")).arg("generate").output().expect("generate");
    let script = String::from_utf8_lossy(&out.stdout).into_owned();
    let start = script.find(header).unwrap_or_else(|| panic!("section {header} missing"));
    script[start..].lines().take(lines).collect::<Vec<_>>().join("\n")
}

/// The `# bundled tools` section of the real generated sourcefile.
fn tools_section() -> String {
    section("# bundled tools", 4)
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
fn the_dotdot_function_climbs_one_directory() {
    // A function, not an alias, exactly so no `expand_aliases` dance is needed — it works even
    // in script contexts like this one, where an alias would be dead on arrival.
    let dir = std::env::temp_dir().join(format!("bashrs_dotdot_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    let script =
        format!("{}\ncd '{}'\n..\npwd", section("..() {", 1), dir.join("inner").display());
    let out = bash(&script);
    assert_eq!(out.trim(), dir.to_str().unwrap(), "`..` must land one directory up");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_dotdot_function_survives_an_active_alias() {
    // The user's own rc may still define `alias ..='cd ..'` (the definition bashrs absorbed).
    // Parsed with that alias live, `..()` would alias-expand into `cd .. ()` — a syntax error
    // that aborts the whole source (and with it PATH, python3, everything below). The emitted
    // `unalias` line ahead of the definition must defuse it.
    let script = format!(
        "shopt -s expand_aliases\nalias ..='cd .. '\n{}\ntype .. | head -1",
        section("unalias ..", 2)
    );
    let out = bash(&script);
    assert!(out.contains(".. is a function"), "alias shadow must be cleared first: {out}");
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
