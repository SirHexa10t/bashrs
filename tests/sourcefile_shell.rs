//! Behavioral tests of the generated `sourcefile.sh`'s shell surface, run through a real bash —
//! the parts unit tests can only assert as strings (function precedence, PATH idempotence).
//! Kept environment-independent: nothing here requires the bundled tools to actually exist, and
//! every bash starts through [`common::shell`] — detached from the terminal running the tests, and
//! without its bashrs session.

mod common;

use common::{shell, Captured};
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
fn bash(script: &str) -> Captured {
    let out = shell("bash").arg("-c").arg(script).output().expect("bash");
    Captured::new(&out.stdout)
}

/// Run `script` in an interactive bash — the shell the sourcefile arms itself in — returning its
/// stdout.
fn interactive_bash(script: &str) -> Captured {
    let out = shell("bash").args(["--norc", "-i", "-c", script]).output().expect("bash -i");
    Captured::new(&out.stdout)
}

/// The full generated sourcefile on disk, plus a fake `$HOME` whose `.bashrs/bashrs` exists —
/// so full-file sourcing tests trip the interactivity guard, not the missing-binary one.
fn sourcefile_and_home(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let base = std::env::temp_dir().join(format!("bashrs_guard_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("home/.bashrs")).unwrap();
    std::fs::write(base.join("home/.bashrs/bashrs"), "").unwrap();
    let generated =
        Command::new(env!("CARGO_BIN_EXE_bashrs")).arg("generate").output().expect("generate");
    let file = base.join("sourcefile.sh");
    std::fs::write(&file, &generated.stdout).unwrap();
    (file, base.join("home"))
}

/// Every shell a test starts leads a session of its own, so it has no terminal to take: an
/// interactive bash would otherwise open the terminal running the tests and take its foreground —
/// in parallel, leaving it to a group that is gone, and the run stopped. Told by the session id,
/// which is the shell's own pid exactly when it leads one, with a terminal around or not.
#[test]
fn test_shells_start_in_a_session_of_their_own() {
    let said = Captured::new(&shell("sh").args(["-c", "echo $$ $(ps -o sid= -p $$)"]).output().expect("sh").stdout);
    let ids: Vec<&str> = said.split_whitespace().collect();
    assert_eq!(ids.len(), 2, "{said}");
    assert_eq!(ids[0], ids[1], "the shell leads its own session: {said}");
}

/// And it starts without the bashrs session of whoever runs the tests: the variables a bashrs-armed
/// shell exports are removed, and its tools directory is off `PATH`.
#[test]
fn test_shells_start_without_a_bashrs_session() {
    let command = shell("sh");
    let set: std::collections::BTreeMap<String, Option<String>> = command
        .get_envs()
        .map(|(name, value)| (name.to_string_lossy().into_owned(), value.map(|value| value.to_string_lossy().into_owned())))
        .collect();
    for variable in ["GREP_COLORS", "HISTTIMEFORMAT", "PS1", "_BASHRS_BARE"] {
        assert_eq!(set.get(variable), Some(&None), "{variable} is removed");
    }
    let path = set.get("PATH").cloned().flatten().unwrap_or_default();
    assert!(!path.split(':').any(|dir| dir.ends_with(".bashrs/tools/bin")), "{path}");
}

/// The whole generated file, parsed by bash itself (`-n`: nothing runs). The tests below source it
/// with stderr discarded and check what got defined, so a syntax error in a later section — the
/// keybinds, the completion, the greeting — failed none of them.
#[test]
fn the_whole_generated_file_parses_in_bash() {
    let (file, _) = sourcefile_and_home("parse");
    let out = shell("bash").arg("-n").arg(&file).output().expect("bash -n");
    assert!(out.status.success(), "bash rejects the generated file: {}", Captured::new(&out.stderr));
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

#[test]
fn non_interactive_shells_inherit_nothing_at_all() {
    // scp, `ssh host cmd`, and wrapper scripts source bashrc too. They must see zero output
    // (stdout corrupts scp) and zero definitions: an OS maintenance script expecting the
    // system `python3` must never be rerouted onto the bundled one.
    let (file, home) = sourcefile_and_home("inert");
    let out = bash(&format!(
        "HOME='{}'; source '{}'; echo status:$?; \
         echo \"python3-is:$(type -t python3)\"; echo \"lll-is:$(type -t lll)\"",
        home.display(),
        file.display()
    ));
    assert!(out.contains("status:0"), "the early return must not look like a failure: {out}");
    assert!(!out.contains("python3-is:function"), "system python3 must stay untouched: {out}");
    assert!(!out.contains("lll-is:function"), "no definitions non-interactively: {out}");
    let silence = bash(&format!("HOME='{}'; source '{}'", home.display(), file.display()));
    assert_eq!(&*silence, "", "a non-interactive source must write nothing to stdout");
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

#[test]
fn the_sourcefile_arms_fully_in_interactive_shells() {
    // The counterpart: with `i` in `$-`, everything defines as before.
    let (file, home) = sourcefile_and_home("armed");
    let script = format!(
        "HOME='{}'; source '{}' 2>/dev/null; \
         type lll >/dev/null 2>&1 && echo defined || echo undefined",
        home.display(),
        file.display()
    );
    let out = interactive_bash(&script);
    assert!(out.contains("defined"), "interactive shells must get the full surface: {out}");
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

#[test]
fn a_bare_session_defines_nothing_and_consumes_its_flag() {
    // `session_bare` starts an interactive shell with _BASHRS_BARE exported; the sourcefile must
    // return before defining ANYTHING — and eat the flag on the way out, leaving the bare shell's
    // environment clean. One-shot semantics, exercised for real: the second source (no manual
    // unset — the guard already consumed the flag) arms the full surface again.
    let (file, home) = sourcefile_and_home("bare");
    let script = format!(
        "HOME='{0}'; _BASHRS_BARE=1; source '{1}' 2>/dev/null; echo status:$?; \
         echo \"lll-bare:$(type -t lll)\"; echo \"flag-after:[${{_BASHRS_BARE:-gone}}]\"; \
         source '{1}' 2>/dev/null; echo \"lll-back:$(type -t lll)\"",
        home.display(),
        file.display()
    );
    let text = interactive_bash(&script);
    assert!(text.contains("status:0"), "the bare-session return must be quiet success: {text}");
    assert!(!text.contains("lll-bare:function"), "a bare session must get NO bashrs surface: {text}");
    assert!(text.contains("flag-after:[gone]"), "the guard must consume the flag: {text}");
    assert!(text.contains("lll-back:function"), "the very next source arms fully — no manual unset: {text}");
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
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

/// The reload hook runs only on the reload code — and the wrapper returns the command's own status
/// either way, which a bare `[ "$?" -eq N ] && hook` did not (it left the test's: 1 after a plain
/// success and after a failure alike).
#[test]
fn the_compile_wrapper_reloads_only_on_its_code_and_keeps_its_status() {
    let (file, home) = sourcefile_and_home("reload");
    let binary = home.join(".bashrs/bashrs");
    std::fs::write(&binary, "#!/bin/sh\nexit \"${FAKE_EXIT:-0}\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let wrapper = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .find(|line| line.starts_with("brs_compile()"))
        .expect("brs_compile is defined")
        .to_string();
    let script = format!(
        "HOME='{}'; shell_new() {{ echo reloading; }}\n{wrapper}\n\
         export FAKE_EXIT=3; brs_compile; echo status:$?\n\
         export FAKE_EXIT=0; brs_compile; echo status:$?\n\
         export FAKE_EXIT=97; brs_compile",
        home.display()
    );
    let out = bash(&script);
    assert_eq!(out.lines().collect::<Vec<_>>(), ["status:3", "status:0", "reloading"], "{out}");
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

/// Alt+L binds the first LISTED desktop that runs — table order, not the order the processes
/// started in — and takes its scratch variable with it. `pgrep` is stubbed to answer as the real
/// one does (`PID NAME …` on one line), so no case needs a desktop actually running.
#[test]
fn alt_l_binds_the_first_listed_desktop_that_runs() {
    let (file, home) = sourcefile_and_home("altl");
    let bound = |running: &str, session: &str| {
        let script = format!(
            "pgrep() {{ printf '%s' '{running}'; }}; HOME='{}'; XDG_SESSION_TYPE={session}; source '{}' 2>/dev/null; \
             bind -S 2>/dev/null; echo \"left:${{__bashrs_desktops-none}}\"",
            home.display(),
            file.display()
        );
        interactive_bash(&script)
    };
    let cases = [
        ("1 openbox 2 i3", "x11", Some("i3-msg restart"), "table order, not process order"),
        ("1 gnome-shell 2 sway", "wayland", Some("swaymsg reload"), "GNOME's X11-only row falls through"),
        ("1 kwin_wayland 2 plasmashell", "wayland", Some("systemctl --user restart plasma-plasmashell.service"), "Plasma on Wayland"),
        ("", "x11", None, "nothing listed running"),
    ];
    for (running, session, expected, why) in cases {
        let out = bound(running, session);
        match expected {
            Some(command) => assert!(out.contains(&format!("\\el outputs {command}")), "{why}: {out}"),
            None => assert!(!out.contains("\\el outputs"), "{why}: nothing bound: {out}"),
        }
        assert!(out.contains("left:none"), "{why}: the chain's variable must go: {out}");
    }
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

#[test]
fn the_dotdot_function_climbs_one_directory() {
    // A function, not an alias, exactly so no `expand_aliases` dance is needed — it works even
    // in script contexts like this one, where an alias would be dead on arrival.
    let dir = std::env::temp_dir().join(format!("bashrs_dotdot_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    // Located by name, not by `..() {` verbatim: the gap between `()` and `{` belongs to the
    // category-table aligner and may be any width.
    let script =
        format!("{}\ncd '{}'\n..\npwd", section("..()", 1), dir.join("inner").display());
    let out = bash(&script);
    assert_eq!(out.trim(), dir.to_str().unwrap(), "`..` must land one directory up");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_path_prepend_is_idempotent_across_resources() {
    // Sourcing the section twice (nested shells re-source the rc) must not stack PATH entries —
    // counted exactly, under a HOME of the test's own, so no other bashrs's directory can count.
    let section = tools_section();
    let script = format!(
        "HOME=/nonexistent/bashrs-test-home\n{section}\n{section}\n\
         printf '%s' \":$PATH:\" | grep -oF \":$HOME/.bashrs/tools/bin:\" | wc -l"
    );
    assert_eq!(bash(&script).trim(), "1");
}
