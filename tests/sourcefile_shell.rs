//! Behavioral tests of the generated `sourcefile.sh`'s shell surface, run through a real bash —
//! the parts unit tests can only assert as strings (function precedence, PATH idempotence).
//! Kept environment-independent: nothing here requires the bundled tools to actually exist, and
//! every bash starts through [`common::shell`] — detached from the terminal running the tests, and
//! without its bashrs session.

mod common;

use common::{leading_shell, lines, shell, Captured, Home};
use std::process::{Command, Output};

/// The first `lines` of a `# `-headed section of the real generated sourcefile.
fn section(header: &str, lines: usize) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs")).arg("generate").output().expect("generate");
    let script = String::from_utf8_lossy(&out.stdout).into_owned();
    let start = script.find(header).unwrap_or_else(|| panic!("section {header} missing"));
    script[start..].lines().take(lines).collect::<Vec<_>>().join("\n")
}

/// The `# bundled tools` section of the real generated sourcefile.
fn tools_section() -> String {
    section("# bundled tools", 3)
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

/// Every shell a test starts is in a session of its own, so it has no terminal to take: an
/// interactive bash would otherwise open the terminal running the tests and take its foreground —
/// in parallel, leaving it to a group that is gone, and the run stopped. The `sh` above it leads
/// that session, so the shell is one started from another, which greets not; a `leading_shell`
/// leads it itself. Told by the session id — a process's own pid exactly when it leads one, with a
/// terminal around or not — against the shell's pid and its parent's.
#[test]
fn test_shells_start_in_a_session_of_their_own() {
    let ids = |mut command: Command| {
        let said = Captured::new(&command.args(["-c", "echo $$ $PPID $(ps -o sid= -p $$)"]).output().expect("sh").stdout);
        let ids: Vec<String> = said.split_whitespace().map(str::to_string).collect();
        assert_eq!(ids.len(), 3, "{said}");
        ids
    };
    let nested = ids(shell("sh"));
    assert_eq!(nested[2], nested[1], "the `sh` above the shell leads its session: {nested:?}");
    assert_ne!(nested[2], nested[0], "and the shell does not: {nested:?}");
    let leading = ids(leading_shell("sh"));
    assert_eq!(leading[2], leading[0], "a leading shell leads its own: {leading:?}");
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

/// An interactive bash under `home`, as an rc file runs one: `before` (the lines ahead of bashrs's
/// block), the sourcefile, then `after` — one started from another shell, as [`shell`] starts it.
/// Returns stdout.
fn bash_rc(home: &Home, before: &str, after: &str) -> Captured {
    rc_in(shell("bash"), home, before, after)
}

/// [`bash_rc`] in `bash`, as [`shell`] or [`leading_shell`] starts it.
fn rc_in(mut bash: Command, home: &Home, before: &str, after: &str) -> Captured {
    let script = format!("{before}\nsource \"$HOME/.bashrs/sourcefile.sh\" 2>/dev/null\n{after}");
    let out = bash.args(["--norc", "-i", "-c", &script]).env("HOME", &home.dir).output().expect("bash -i");
    Captured::new(&out.stdout)
}

/// An alias of a command's name, set before the file is sourced — `alias g=git`, as many a setup
/// has it — takes nothing over: the command's function is defined under its own name, and the
/// aliased command stays what it was. (Defined as `g() {…}`, the function was bash's `git`.)
#[test]
fn an_alias_of_a_command_s_name_hijacks_nothing() {
    let home = Home::new("bash_aliased");
    let out = bash_rc(&home, "alias g=git; alias ..='cd ..'", r#"echo "git:$(type -t git) g:$(type -t g) dotdot:$(type -t ..)""#);
    assert!(!out.contains("git:function"), "the alias's command must stay its own: {out}");
    assert!(out.contains("g:function dotdot:function"), "both defined under their own names, and the ones typed: {out}");
}

/// Every alias of a name the file defines a function for, set before the file is sourced, is set
/// aside: typing the name runs bashrs's command, and the alias's expansion is kept, which
/// `shell_def` shows with the line that brings it back. An unrelated alias stays, and one defined
/// after the file wins again — the last definition does, of either kind. Exhaustive, and read
/// from bash itself: every name the file gives bash is aliased first.
#[test]
fn aliases_of_the_commands_names_are_set_aside() {
    let home = Home::new("bash_set_aside");
    let defined = bash_rc(&home, "", "declare -F");
    let names: Vec<&str> =
        defined.lines().filter_map(|line| line.strip_prefix("declare -f ")).filter(|name| !name.starts_with("_bashrs_")).collect();
    assert!(names.len() > 100, "bash defined too little to check: {names:?}");
    let aliases: String = names.iter().map(|name| format!("alias -- {name}=false\n")).collect();
    let checks = format!(
        "for n in {}; do [ \"$(type -t -- \"$n\")\" = function ] || echo \"still an alias: $n\"; done\n\
         shell_def g\nalias keep\nalias g=git\necho \"defined after: $(type -t g)\"",
        names.join(" ")
    );
    let out = bash_rc(&home, &format!("{aliases}alias keep=true"), &checks).plain();
    assert!(!out.contains("still an alias"), "{out}");
    assert!(out.contains("set-aside ALIAS") && out.contains("\nalias g='false'\n"), "shell_def shows it, restorable: {out}");
    assert!(out.contains("alias keep='true'"), "an unrelated alias stays: {out}");
    assert!(out.ends_with("defined after: alias\n"), "an alias defined after the file wins again: {out}");
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

/// Sourcing sets the variables it means to — `GREP_COLORS` and `HISTTIMEFORMAT`, and the record
/// of aliases set aside, which `shell_def` reads (here holding the `g` a framework would define) —
/// and `PATH` it only extends; the set-aside's own loop variables are gone after it, and so are
/// the greeting's, in a terminal's first bash. Readline is set up first, as in any interactive
/// bash by its first prompt: the file's `bind` would set it up otherwise, and readline's `COLUMNS`
/// and `LINES` would count as the file's.
#[test]
fn sourcing_leaves_no_variable_of_its_own_behind() {
    let home = Home::new("bash_novars");
    let out = rc_in(leading_shell("bash"), &home, "alias g=git; bind -l >/dev/null; compgen -v; echo ---", "echo +++; compgen -v");
    let (before, greeted_then_after) = out.split_once("---\n").expect("both listings");
    let (_, after) = greeted_then_after.split_once("+++\n").expect("the listing after the greeting");
    let before: std::collections::BTreeSet<&str> = before.lines().collect();
    let new: std::collections::BTreeSet<&str> = after.lines().filter(|name| !before.contains(name)).collect();
    assert_eq!(new.into_iter().collect::<Vec<_>>(), ["GREP_COLORS", "HISTTIMEFORMAT", "__bashrs_set_aside"], "{out}");
}

/// A terminal's first bash — leading its session — sourcing the file between `before` and `after`,
/// as its rc file does. Returns stdout and stderr; the file's own stderr is kept, for the warning.
fn first_bash(home: &Home, before: &str, after: &str) -> Output {
    let script = format!("{before}\nsource \"$HOME/.bashrs/sourcefile.sh\"\n{after}");
    leading_shell("bash").args(["--norc", "-i", "-c", &script]).env("HOME", &home.dir).output().expect("bash -i")
}

/// The greeting comes in a terminal's first shell — the one leading its session — printed by the
/// file itself, styled, and not in a shell started from it, which would only repeat it; the
/// function printing it is gone once it has. The nested bash speaks before it sources the file, so
/// the output shows which bash greeted; and it is not the script's last command, which bash could
/// run in its own place — its session and all.
#[test]
fn only_a_terminal_s_first_shell_greets() {
    let home = Home::new("bash_greet");
    let out = first_bash(
        &home,
        "",
        r#"bash --norc -i -c 'echo nested; source "$HOME/.bashrs/sourcefile.sh" 2>/dev/null'; echo back; declare -F __bashrs_greet || echo gone"#,
    );
    let said = Captured::new(&out.stdout);
    assert!(said.contains('\x1b') && !said.contains("\\033"), "styled: its escapes printed as such, not spelled: {said}");
    let said = said.plain();
    let said = lines(&said);
    assert_eq!(said.len(), 4, "{said:?}");
    assert!(said[0].starts_with("Loaded Bashrs! Press ALT+H"), "{said:?}");
    assert_eq!(said[1..], ["nested", "back", "gone"]);
}

/// The warning comes, on stderr, exactly while an archived config waits in `~/.bashrs` — one
/// named `configrs*.old`, as an archive is now or was before the name gained `.toml.`; once, however
/// many wait — and not for the config itself or for another `.old`. The same under the user's
/// `failglob`, which fails a glob matching nothing: the greeting still comes, and its function
/// still goes.
#[test]
fn the_warning_comes_while_an_archived_config_waits() {
    let home = Home::new("bash_warn");
    let bashrs = home.dir.join(".bashrs");
    let warnings = |before: &str| {
        let out = first_bash(&home, before, "declare -F __bashrs_greet || echo gone");
        assert!(Captured::new(&out.stdout).plain().ends_with("gone\n"), "{before}: greeted, its function gone: {}", Captured::new(&out.stdout));
        Captured::new(&out.stderr).plain().matches("an archived config (~/.bashrs/configrs*.old) awaits merging").count()
    };
    std::fs::write(bashrs.join("configrs.toml"), "").unwrap();
    std::fs::write(bashrs.join("other.old"), "").unwrap();
    for before in ["", "shopt -s failglob"] {
        assert_eq!(warnings(before), 0, "{before}: the config itself, or another archive, is no cue");
    }
    std::fs::write(bashrs.join("configrs.toml.2026102112.old"), "").unwrap();
    assert_eq!(warnings(""), 1, "an archive waits");
    std::fs::write(bashrs.join("configrs.old"), "").unwrap();
    assert_eq!(warnings("shopt -s failglob"), 1, "one from before the name gained .toml. too — one warning for both");
    std::fs::remove_file(bashrs.join("configrs.toml.2026102112.old")).unwrap();
    assert_eq!(warnings(""), 1, "and that one alone");
    // With globbing switched off, the function switches it on for its glob, and back off after.
    let off = first_bash(&home, "set -f", "[[ -o noglob ]] && echo still-off");
    assert_eq!(Captured::new(&off.stderr).plain().matches("awaits merging").count(), 1, "globbing off, the archive is found");
    assert!(Captured::new(&off.stdout).ends_with("still-off\n"), "and globbing is off again: {}", Captured::new(&off.stdout));
}

/// With no alias set aside, the record is still there, empty — `shell_def`'s lookup in it needs it
/// to be an associative array: undeclared, bash would read a name like `..` there as arithmetic,
/// and the report would never come.
#[test]
fn shell_def_reads_an_empty_record() {
    let home = Home::new("bash_def_empty");
    let out = bash_rc(&home, "", "shell_def ..; echo status:$?").plain();
    assert!(out.starts_with(".. is a FUNCTION") && out.ends_with("status:0\n"), "{out}");
}

#[test]
fn the_python3_function_survives_an_active_alias() {
    // A python3 alias — a common one — set aside with every other bashrs name's, at the top of the
    // whole file: the function is what typing `python3` runs.
    let home = Home::new("python3_alias");
    let out = bash_rc(&home, "alias python3='/usr/bin/python3'", "type -t python3");
    assert_eq!(out.trim(), "function", "the alias shadow must be set aside: {out}");
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
        .find(|line| line.starts_with("function brs_compile "))
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

#[test]
fn the_dotdot_function_climbs_one_directory() {
    // A function, not an alias, exactly so no `expand_aliases` dance is needed — it works even
    // in script contexts like this one, where an alias would be dead on arrival.
    let dir = std::env::temp_dir().join(format!("bashrs_dotdot_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    // Located by name, not by `function .. {` verbatim: the gap between the name and `{` belongs
    // to the category-table aligner and may be any width.
    let script =
        format!("{}\ncd '{}'\n..\npwd", section("function .. ", 1), dir.join("inner").display());
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
