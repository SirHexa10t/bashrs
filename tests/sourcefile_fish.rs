//! Behavioral tests of the generated `sourcefile.fish`, run through a real fish — the fish
//! counterpart of `sourcefile_shell.rs`: the same contracts (nothing for non-interactive shells,
//! everything for interactive ones, a one-shot bare session, a PATH that doesn't stack), plus what
//! fish does its own way — the `cd` override keeping fish's own `cd -`, completions replacing the
//! ones fish ships, history read in bash's order.
//!
//! Fish starts with its stock startup — `--no-config` would also skip the part that sets
//! `$fish_complete_path` — but with nothing personal: each test points `HOME` at a fresh directory,
//! which is where fish keeps user config, history and universal variables.

use std::path::PathBuf;
use std::process::{Command, Output};

/// A generated sourcefile, as `bashrs generate --shell <shell>` prints it.
fn generated(shell: &str) -> Vec<u8> {
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs")).args(["generate", "--shell", shell]).output().expect("generate");
    assert!(out.status.success(), "generate --shell {shell} failed");
    out.stdout
}

/// A stand-in for the installed binary: enough behaviour to watch a wrapper work — `hg` echoes its
/// stdin, `brs_compile` exits with `$FAKE_EXIT`, `errcho` writes its words to stderr — and silent
/// otherwise, so the load greeting (a call to `gecho`) adds nothing to what the tests read. The
/// commands a test needs for real are handed to the real binary by `exec`, which leaves the shell
/// it was called from as its parent, as the installed binary would have it.
fn stand_in() -> String {
    format!(
        "#!/bin/sh\ncase \"$1\" in\n\
         \x20 hg) exec cat ;;\n\
         \x20 brs_compile) exit \"${{FAKE_EXIT:-0}}\" ;;\n\
         \x20 errcho) shift; printf '%s\\n' \"$*\" >&2 ;;\n\
         \x20 complete-flags|brs_sourcefile|shell_def|shell_args_print|lll) exec '{}' \"$@\" ;;\n\
         esac\n",
        env!("CARGO_BIN_EXE_bashrs")
    )
}

/// A scratch `HOME` holding both generated sourcefiles and — unless the test is about its absence —
/// the stand-in binary at `.bashrs/bashrs`. Removed when dropped, so a failing assertion leaves
/// nothing behind.
struct Home {
    dir: PathBuf,
}

impl Home {
    fn new(tag: &str) -> Self {
        let home = Self::without_binary(tag);
        let binary = home.dir.join(".bashrs/bashrs");
        std::fs::write(&binary, stand_in()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        home
    }

    fn without_binary(tag: &str) -> Self {
        let home = Self::empty(tag);
        std::fs::write(home.dir.join(".bashrs/sourcefile.fish"), generated("fish")).unwrap();
        std::fs::write(home.dir.join(".bashrs/sourcefile.sh"), generated("bash")).unwrap();
        home
    }

    /// Just the directory and its `.bashrs` — for the install to fill.
    fn empty(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("bashrs_fish_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".bashrs")).unwrap();
        Home { dir }
    }

    fn fish_sourcefile(&self) -> PathBuf {
        self.dir.join(".bashrs/sourcefile.fish")
    }

    /// Run `script` in fish under this `HOME` — interactive (`-i`) or not — with the fish
    /// sourcefile's path in `$sourcefile`, so no script has to quote it.
    fn fish(&self, interactive: bool, script: &str) -> Output {
        let mut fish = Command::new("fish");
        fish.env("HOME", &self.dir)
            .env("sourcefile", self.fish_sourcefile())
            .env("TZ", "UTC")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("XDG_CACHE_HOME");
        if interactive {
            fish.arg("-i");
        }
        fish.args(["-c", script]).output().expect("fish is needed for the fish sourcefile's tests")
    }

    /// [`Self::fish`], interactive, returning stdout.
    fn interactive(&self, script: &str) -> String {
        String::from_utf8_lossy(&self.fish(true, script).stdout).into_owned()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

// ——— The file ————————————————————————————————————————————————————————————

/// Fish itself judges the whole file (`--no-execute`: nothing runs). The tests below source it and
/// check what got defined, so a syntax error in a later section would fail none of them.
#[test]
fn the_whole_file_parses_in_fish() {
    let home = Home::new("parse");
    let parse = Command::new("fish").arg("--no-execute").arg(home.fish_sourcefile()).output().expect("fish");
    assert!(parse.status.success(), "fish rejects the file: {}", String::from_utf8_lossy(&parse.stderr));
}

/// Every section is a table, as bash's are: one function per line, the descriptions starting in
/// one column — checked per section against the real file, since each section aligns on its own.
#[test]
fn every_section_aligns_its_descriptions_into_a_column() {
    let file = String::from_utf8(generated("fish")).unwrap();
    let mut sections_checked = 0;
    for section in file.split("\n\n").filter(|section| section.starts_with("# ")) {
        let columns: std::collections::BTreeSet<usize> = section
            .lines()
            .filter(|line| line.starts_with("function ") && !line.ends_with(" -a key cmd"))
            .filter_map(|line| line.find(" -d "))
            .collect();
        assert!(columns.len() <= 1, "the descriptions in `{}` are not one column", section.lines().next().unwrap_or_default());
        sections_checked += usize::from(columns.len() == 1);
    }
    assert!(sections_checked >= 15, "sections actually checked: {sections_checked}");
    let multi_line: Vec<&str> = file.lines().filter(|line| line.starts_with("function ") && !line.contains("; end")).collect();
    assert_eq!(
        multi_line,
        ["function __bashrs_prompt_clock --on-event fish_prompt", "function __bashrs_bind -a key cmd"],
        "every command is a one-line function; only the two pieces of machinery are blocks"
    );
}

// ——— Who gets bashrs ———————————————————————————————————————————————————————

/// Fish reads conf.d for every fish it starts, scripts and `fish -c` included. Those must see no
/// output and no definitions — above all not the `python3` function.
#[test]
fn non_interactive_fish_inherits_nothing_at_all() {
    let home = Home::new("inert");
    let out = home.fish(false, "source $sourcefile; echo status:$status; functions -q lll; and echo lll-defined; functions -q python3; and echo python3-defined");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "status:0\n", "{}", String::from_utf8_lossy(&out.stderr));
    let silence = home.fish(false, "source $sourcefile");
    assert_eq!(silence.stdout, b"", "a non-interactive source must write nothing to stdout");
}

/// Arming bashrs runs nothing that can complain: an interactive source is silent on stderr. (Fish
/// code that bashrs merely triggers counts too — loading another tool's completions can run that
/// tool, and complain where it is missing.)
#[test]
fn interactive_sourcing_writes_nothing_to_stderr() {
    let home = Home::new("quiet");
    let out = home.fish(true, "source $sourcefile");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "", "sourcing must not complain");
}

#[test]
fn the_file_arms_fully_in_interactive_fish() {
    let home = Home::new("armed");
    let out = home.interactive("source $sourcefile; functions -q lll; and echo lll-defined");
    assert!(out.contains("lll-defined"), "interactive fish must get the full surface: {out}");
}

/// `shell_bare`'s flag: the source defines NOTHING and consumes the flag on the way out, and the
/// very next source arms everything again — no manual unset.
#[test]
fn a_bare_session_defines_nothing_and_consumes_its_flag() {
    let home = Home::new("bare");
    let out = home.interactive(
        "set -gx _BASHRS_BARE 1; source $sourcefile; echo status:$status
         functions -q lll; and echo lll-bare
         set -q _BASHRS_BARE; and echo flag-left; or echo flag-gone
         source $sourcefile; functions -q lll; and echo lll-back",
    );
    assert_eq!(lines(&out), ["status:0", "flag-gone", "lll-back"], "{out}");
}

#[test]
fn the_file_bails_early_when_the_binary_is_missing() {
    let home = Home::without_binary("nobinary");
    let out = home.interactive("source $sourcefile; echo status:$status; functions -q lll; and echo lll-defined");
    assert_eq!(lines(&out), ["status:0"], "not installed is a quiet no-op: {out}");
}

/// Whatever bash's file defines, fish's does too — read from the two real shells, not from a list.
/// Only bash's own completion helpers have no fish counterpart: fish completes declaratively.
#[test]
fn every_function_bash_gets_fish_gets_too() {
    let home = Home::new("parity");
    let bash = Command::new("bash")
        .args(["--norc", "-i", "-c", "source \"$HOME/.bashrs/sourcefile.sh\" 2>/dev/null; declare -F"])
        .env("HOME", &home.dir)
        .output()
        .expect("bash");
    let bash = String::from_utf8_lossy(&bash.stdout).into_owned();
    // `--all`: without it fish hides every name that starts with `_`, `_arg_lean_spec` among them.
    let fish = home.interactive("functions --all -n | string split ', '; echo ---; source $sourcefile; functions --all -n | string split ', '");
    let (before, after) = fish.split_once("---\n").expect("both listings");
    let fish_new: std::collections::BTreeSet<&str> = after.lines().filter(|name| !before.lines().any(|old| old == *name)).collect();
    let bash_names: Vec<&str> = bash
        .lines()
        .filter_map(|line| line.strip_prefix("declare -f "))
        .filter(|name| !name.starts_with("_bashrs_"))
        .collect();
    assert!(bash_names.len() > 100, "bash defined too little to compare: {bash_names:?}");
    let missing: Vec<&&str> = bash_names.iter().filter(|name| !fish_new.contains(**name) && !before.lines().any(|old| old == **name)).collect();
    assert!(missing.is_empty(), "defined in bash's file but not in fish's: {missing:?}");
}

/// Sourcing defines functions and sets the one variable it means to, `GREP_COLORS` (`PATH` it only
/// extends) — nothing else is left behind, a loop variable included.
#[test]
fn sourcing_leaves_no_variable_of_its_own_behind() {
    let home = Home::new("novars");
    let out = home.interactive(
        "set -l before (set -n); source $sourcefile
         for name in (set -n); contains -- $name $before; or echo $name; end",
    );
    // `before` is this probe's own; everything else must be the intended export.
    assert_eq!(lines(&out), ["GREP_COLORS", "before"], "new variables after sourcing: {out}");
}

// ——— The bundled tools ————————————————————————————————————————————————————

#[test]
fn the_python3_function_replaces_an_earlier_definition() {
    let home = Home::new("python3");
    let out = home.interactive("function python3; echo OLD; end; source $sourcefile; functions python3");
    assert!(out.contains(".bashrs/tools/bin/python3"), "must route through the shim dir: {out}");
    assert!(!out.contains("OLD"), "the old definition must be fully replaced: {out}");
}

/// Nested shells re-source the file; the tools' directory must still appear once.
#[test]
fn the_path_prepend_is_idempotent_across_resources() {
    let home = Home::new("path");
    let out = home.interactive("source $sourcefile; source $sourcefile; string match -e .bashrs/tools/bin -- $PATH | count");
    assert_eq!(out.trim(), "1");
}

// ——— What fish does its own way ———————————————————————————————————————————————

/// `cd` lists where it lands — and keeps fish's own `cd -`, which lives in fish's `cd` *function*
/// and that `builtin cd` would have lost.
#[test]
fn the_cd_override_lists_and_keeps_fishs_own_cd_minus() {
    let home = Home::new("cd");
    std::fs::create_dir_all(home.dir.join("from")).unwrap();
    std::fs::create_dir_all(home.dir.join("to/inner")).unwrap();
    std::fs::write(home.dir.join("to/marker.txt"), "").unwrap();
    // Sourced twice, as a nested shell's start or a manual reload does: the original must still be
    // fish's own `cd`, not the override copied over it, nor the builtin standing in for it.
    let out = home.fish(
        true,
        "source $sourcefile; source $sourcefile; cd $HOME/from; cd $HOME/to; cd -; echo back:(path basename $PWD)
         cd $HOME/to/inner; ..; echo up:(path basename $PWD)",
    );
    let (said, complained) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(said.contains("marker.txt"), "landing in `to` lists it: {said}");
    assert!(said.contains("back:from"), "`cd -` still returns: {said}{complained}");
    assert!(said.contains("up:to"), "`..` climbs one directory: {said}");
    assert_eq!(complained, "", "sourcing again keeps the saved original without a word");
}

/// The reload hook runs only on the reload code, and — unlike bash's one-line `&&` — the wrapper
/// returns the command's own status either way.
#[test]
fn the_compile_wrapper_reloads_only_on_its_code_and_keeps_its_status() {
    let home = Home::new("reload");
    let out = home.interactive(
        "source $sourcefile; function shell_new; echo reloading; end
         FAKE_EXIT=3 brs_compile; echo status:$status
         FAKE_EXIT=0 brs_compile; echo status:$status
         FAKE_EXIT=97 brs_compile",
    );
    assert_eq!(lines(&out), ["status:3", "status:0", "reloading"], "{out}");
}

/// `hg` gets fish's history the way bash's `history` gives it: oldest first, each entry stamped
/// in `HISTTIMEFORMAT`'s shape.
#[test]
fn hg_reads_fish_history_oldest_first_with_bashs_timestamps() {
    let home = Home::new("hg");
    std::fs::create_dir_all(home.dir.join(".local/share/fish")).unwrap();
    std::fs::write(
        home.dir.join(".local/share/fish/fish_history"),
        "- cmd: echo first\n  when: 1760000000\n- cmd: echo second\n  when: 1760000100\n",
    )
    .unwrap();
    let out = home.interactive("source $sourcefile; hg echo");
    assert_eq!(lines(&out), ["2025-10-09_08:53:20  echo first", "2025-10-09_08:55:00  echo second"], "{out}");
}

/// `shell_def` reports fish's meanings of a name through the same report as bash's: a function
/// over a builtin ranked, a variable beside it, a name that is nothing an error — and the probes
/// themselves say nothing on stderr, whatever a name lacks.
#[test]
fn shell_def_ranks_fish_s_meanings_of_a_name() {
    let home = Home::new("def");
    let plain = |out: &Output| console::strip_ansi_codes(&String::from_utf8_lossy(&out.stdout)).into_owned();
    let cd = home.fish(true, "source $sourcefile; shell_def cd");
    assert_eq!(String::from_utf8_lossy(&cd.stderr), "", "nothing from the probes");
    let report = plain(&cd);
    assert!(report.starts_with("cd is a FUNCTION - this is what runs:\n"), "{report}");
    assert!(report.contains("__bashrs_original_cd $argv && command ls"), "the override's own source: {report}");
    assert!(report.contains("outranked BUILTIN - runs via: builtin cd"), "{report}");
    let variable = home.fish(true, "source $sourcefile; set -g lll value; shell_def lll");
    assert!(plain(&variable).contains("also a VARIABLE, unrelated to running 'lll': $lll: set in global scope"), "{}", plain(&variable));
    // Nothing but a variable: the empty fields ahead of it must stay fields, or the variable's
    // text lands where the function's belongs.
    let only = home.fish(true, "source $sourcefile; set -g only_a_variable 1; shell_def only_a_variable");
    assert!(plain(&only).starts_with("also a VARIABLE, unrelated to running 'only_a_variable': $only_a_variable: set in global scope"), "{}", plain(&only));
    assert!(!plain(&only).contains("FUNCTION") && only.status.success(), "{only:?}");
    let nothing = home.fish(true, "source $sourcefile; shell_def no_such_name_here");
    assert_eq!(nothing.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&nothing.stderr),
        "shell_def: 'no_such_name_here' is nothing here - no alias, function, builtin, binary, or variable\n",
        "the report alone: each probe that finds nothing stays silent"
    );
}

/// The words `shell_args_print` is handed, quoted in fish's syntax when fish ran it — the one
/// shell where bash's `$'…'` would not paste back.
#[test]
fn shell_args_print_quotes_in_fish_s_own_syntax() {
    let home = Home::new("args");
    let out = home.interactive(r#"source $sourcefile; shell_args_print "it's" '' a\ \ b"#);
    assert_eq!(lines(&out), ["3 args:", r"  arg1: 'it\'s'", "  arg2: ''", "  arg3: 'a  b'"], "{out}");
}

/// `lll`'s names paste back into the shell it ran in: run from fish, the name column is spelled in
/// fish's syntax, and fish reads every name in it back as the file it names.
#[test]
fn lll_spells_names_so_fish_reads_them_back() {
    let home = Home::new("lll");
    let dir = home.dir.join("names");
    std::fs::create_dir_all(&dir).unwrap();
    let names = ["it's.txt", r"back\slash.txt", "two  spaces.txt", "new\nline.txt", "plain.txt"];
    for name in names {
        std::fs::write(dir.join(name), "").unwrap();
    }
    let out = home.interactive(&format!("source $sourcefile; builtin cd {}; lll", dir.display()));
    let shown = console::strip_ansi_codes(&out).into_owned();
    assert!(shown.contains(r"'it\'s.txt'") && !shown.contains("$'"), "fish's quoting, not bash's: {shown}");
    // The name is each row's last column; the header names it `Filename`, and `.`/`..` lead.
    let words: Vec<&str> = shown.lines().skip(1).filter_map(|row| row.trim_end().rsplit("  ").next()).map(str::trim).collect();
    let script: String = words.iter().map(|word| format!("printf '%s\\0' {word}\n")).collect();
    let read = home.fish(false, &script);
    let mut read: Vec<String> = String::from_utf8_lossy(&read.stdout).split('\0').filter(|name| !name.is_empty()).map(str::to_string).collect();
    read.sort();
    let mut expected: Vec<String> = names.iter().map(|name| name.to_string()).chain(["./".into(), "../".into()]).collect();
    expected.sort();
    assert_eq!(read, expected, "every spelling reads back as its file: {words:?}");
}

/// A flag completes with its description beside it, asked of the binary at TAB-time, and so does
/// a command's name; a word that is no flag keeps fish's own completion, files.
#[test]
fn flags_and_commands_complete_with_their_descriptions() {
    let home = Home::new("complete");
    let gg = home.interactive("source $sourcefile; complete -C 'gg --del'");
    assert!(gg.starts_with("--delve\t"), "a flag completes with its description: {gg}");
    let aliased = home.interactive("source $sourcefile; complete -C 'upup --he'");
    assert!(aliased.starts_with("--help\tPrint help"), "an alias completes as its command: {aliased}");
    let command = home.interactive("source $sourcefile; complete -C 'shell_ne'");
    assert!(command.contains("shell_new\tStart a fresh shell session"), "and so does a command's name: {command}");
    std::fs::write(home.dir.join("a_file_here.txt"), "").unwrap();
    let file = home.interactive("source $sourcefile; cd $HOME; complete -C 'gg a_file'");
    assert!(file.contains("a_file_here.txt"), "a non-flag word completes files: {file}");
}

/// The real install, end to end, into a home where fish has been run (its configuration
/// directory exists): fish is wired through conf.d, the names fish completes too get bashrs's
/// stand-in files, and a fish started the ordinary way — nothing sourced by hand — has bashrs,
/// with bashrs's completions in place of fish's own, and says nothing on stderr.
#[test]
fn the_install_wires_fish_so_a_new_fish_has_bashrs_and_its_completions_win() {
    let home = Home::empty("install");
    std::fs::create_dir_all(home.dir.join(".config/fish")).unwrap();
    let install = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .arg("install-shell")
        .env("HOME", &home.dir)
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("install-shell");
    assert!(install.status.success(), "{}", String::from_utf8_lossy(&install.stderr));
    assert!(home.dir.join(".config/fish/conf.d/bashrs.fish").is_file(), "fish is wired through its conf.d");
    assert!(!home.dir.join(".config/fish/config.fish").exists(), "and the user's config.fish is never touched");

    // Exactly the names this fish has completions for get a stand-in — whatever this fish ships.
    let ships_diff = home.interactive("path filter -f -- $fish_complete_path/diff.fish");
    let stand_in = home.dir.join(".bashrs/fish_completions/diff.fish");
    assert_eq!(stand_in.is_file(), !ships_diff.is_empty(), "a stand-in for diff exactly when fish ships diff's completions");

    let started = home.fish(true, "functions -q lll; and echo bashrs-armed; complete -C 'diff --'");
    let out = String::from_utf8_lossy(&started.stdout);
    assert!(out.contains("bashrs-armed"), "the conf.d hook arms bashrs in a new interactive fish: {out}");
    assert!(out.contains("--line-number\t") && out.contains("--recursive\t"), "bashrs's diff flags: {out}");
    assert!(!out.contains("--brief") && !out.contains("--unified"), "GNU diff's never joined them: {out}");
    assert_eq!(String::from_utf8_lossy(&started.stderr), "", "a new fish starts in silence");
}

/// A pure-shell command registers nothing, so fish's own completion stays — `cd` still completes
/// directories.
#[test]
fn pure_shell_commands_keep_fishs_own_completion() {
    let home = Home::new("cdcomplete");
    std::fs::create_dir_all(home.dir.join("only_dir_here")).unwrap();
    let out = home.interactive("source $sourcefile; cd $HOME; complete -C 'cd only_'");
    assert!(out.contains("only_dir_here/"), "{out}");
}

/// Each key runs its command as if typed — in the default mode and in vi's insert mode — and the
/// helper that bound them is gone once they are.
#[test]
fn the_keybinds_run_their_commands_as_if_typed() {
    let home = Home::new("binds");
    let out = home.interactive("source $sourcefile; bind alt-q; bind ctrl-alt-n; bind -M insert alt-h; functions -q __bashrs_bind; and echo helper-left");
    assert!(out.contains("bind alt-q 'commandline -r brs_compile; commandline -f execute'"), "{out}");
    assert!(out.contains("bind ctrl-alt-n 'commandline -r shell_bare; commandline -f execute'"), "{out}");
    assert!(out.contains("bind -M insert alt-h 'commandline -r brs_sourcefile; commandline -f execute'"), "vi's insert mode too: {out}");
    assert!(!out.contains("helper-left"), "{out}");
}

/// Alt+H's command shows each shell its own file: fish's in fish, bash's in bash — told by the shell
/// the binary runs under, nothing passed to it.
#[test]
fn brs_sourcefile_shows_the_file_of_the_shell_it_runs_in() {
    let home = Home::new("alth");
    let last_line = |out: &[u8]| String::from_utf8_lossy(out).lines().last().unwrap_or_default().to_string();
    let fish = home.fish(true, "source $sourcefile; brs_sourcefile");
    assert_eq!(last_line(&fish.stdout), format!("# source file: {}", home.fish_sourcefile().display()));
    assert!(console::strip_ansi_codes(&String::from_utf8_lossy(&fish.stdout)).contains("status is-interactive; or return 0"), "fish's text");
    let bash = Command::new("bash")
        .args(["--norc", "-i", "-c", "source \"$HOME/.bashrs/sourcefile.sh\" 2>/dev/null; brs_sourcefile; :"])
        .env("HOME", &home.dir)
        .output()
        .expect("bash");
    assert_eq!(last_line(&bash.stdout), format!("# source file: {}", home.dir.join(".bashrs/sourcefile.sh").display()));
}

/// The stale-config nag speaks exactly while an archived config waits to be merged.
#[test]
fn the_stale_config_nag_speaks_only_while_an_archive_waits() {
    let home = Home::new("stale");
    let quiet = home.fish(true, "source $sourcefile; echo status:$status");
    assert_eq!(String::from_utf8_lossy(&quiet.stderr), "");
    assert_eq!(String::from_utf8_lossy(&quiet.stdout), "status:0\n", "a clean start ends the source in success");
    std::fs::write(home.dir.join(".bashrs/configrs.toml.20260101.old"), "").unwrap();
    let nagged = home.fish(true, "source $sourcefile");
    assert!(String::from_utf8_lossy(&nagged.stderr).contains("an archived config (~/.bashrs/configrs*.old) awaits merging"), "{nagged:?}");
}

/// Alt+L binds the first LISTED desktop that runs — table order, not the order the processes
/// started in. `pgrep` is stubbed to answer as the real one does (a `PID NAME` line each), so no
/// case needs a desktop actually running.
#[test]
fn alt_l_binds_the_first_listed_desktop_that_runs() {
    let home = Home::new("altl");
    let bound = |running: &str, session: &str| {
        home.interactive(&format!(
            "function pgrep; for name in (string split -n ' ' -- '{running}'); echo \"1 $name\"; end; end
             set -gx XDG_SESSION_TYPE {session}; source $sourcefile; bind alt-l 2>/dev/null
             set -q desktops; and echo left-behind"
        ))
    };
    let cases = [
        ("openbox i3", "x11", Some("i3-msg restart"), "table order, not process order"),
        ("gnome-shell sway", "wayland", Some("swaymsg reload"), "GNOME's X11-only row falls through"),
        ("kwin_wayland plasmashell", "wayland", Some("systemctl --user restart plasma-plasmashell.service"), "Plasma on Wayland"),
        ("", "x11", None, "nothing listed running"),
    ];
    for (running, session, expected, why) in cases {
        let out = bound(running, session);
        // What is bound, not how fish quotes it when listing (it re-quotes in its own style).
        let binding = out.lines().find(|line| line.starts_with("bind alt-l "));
        match expected {
            Some(command) => assert!(binding.is_some_and(|line| line.contains(command)), "{why}: {out}"),
            None => assert!(binding.is_none(), "{why}: nothing bound: {out}"),
        }
        assert!(!out.contains("left-behind"), "{why}: the chain's list must not outlive the source: {out}");
    }
}

/// The prompt clock, with the first prompt stood in for by its event: the clock goes in front of the
/// prompt in place by then — fish's own, or one set up after the file (as config.fish does) — which
/// sees the last command's status as if unwrapped; and a second source wraps nothing twice.
#[test]
fn the_utc_clock_goes_in_front_of_whatever_prompt_is_in_place() {
    let home = Home::new("clock");
    // The event also runs fish's own first-prompt setup, which greets; the prompts follow that.
    let prompts = |script: &str| -> Vec<String> {
        let out = console::strip_ansi_codes(&home.interactive(script)).into_owned();
        out.lines().filter(|line| line.contains("(UTC)]") || line.contains("left")).map(str::to_string).collect()
    };
    let theirs = prompts("source $sourcefile; function fish_prompt; printf 'mine> '; end; emit fish_prompt; fish_prompt");
    assert_eq!(theirs.len(), 1, "{theirs:?}");
    assert!(theirs[0].starts_with('[') && theirs[0].ends_with("(UTC)]mine> "), "the clock, then their prompt: {theirs:?}");
    assert_eq!(theirs[0].len(), "[00:00:00(UTC)]mine> ".len(), "{theirs:?}");
    let status = prompts(
        "source $sourcefile; function fish_prompt; printf 's=%s p=%s' $status \"$pipestatus\"; end; emit fish_prompt
         false | true; fish_prompt; echo; false; fish_prompt",
    );
    let after_clock: Vec<&str> = status.iter().filter_map(|line| line.split_once("(UTC)]").map(|(_, prompt)| prompt)).collect();
    assert_eq!(after_clock, ["s=0 p=1 0", "s=1 p=1"], "the prompt sees the last command's status: {status:?}");
    let resourced = "source $sourcefile; emit fish_prompt; source $sourcefile; emit fish_prompt; fish_prompt; functions -q __bashrs_prompt_clock; and echo handler-left";
    let again = prompts(resourced);
    assert_eq!(again.iter().map(|line| line.matches("(UTC)]").count()).sum::<usize>(), 1, "one clock however often the file is sourced: {again:?}");
    assert!(!again.iter().any(|line| line.contains("handler-left")), "{again:?}");
    assert_eq!(String::from_utf8_lossy(&home.fish(true, resourced).stderr), "", "and sourcing again says nothing");
    let ending = console::strip_ansi_codes(&home.interactive("source $sourcefile; function fish_prompt; printf 'mine\\n'; end; emit fish_prompt; fish_prompt; echo END")).into_owned();
    assert!(ending.contains("(UTC)]mine\nEND"), "a prompt's own last newline is kept: {ending}");
    let stock = prompts("source $sourcefile; emit fish_prompt; fish_prompt");
    assert!(stock.len() == 1 && stock[0].starts_with('[') && stock[0].trim_end().ends_with('>'), "fish's own prompt, behind the clock: {stock:?}");
}

/// The real thing, through a terminal: a new interactive fish, its prompt set up by config.fish —
/// which runs after bashrs's conf.d hook — draws its very first prompt with the clock in front.
#[test]
fn the_first_prompt_of_a_new_fish_has_the_clock_in_front() {
    let home = Home::new("pty");
    let config = home.dir.join(".config/fish");
    std::fs::create_dir_all(config.join("conf.d")).unwrap();
    std::fs::write(config.join("conf.d/bashrs.fish"), "source \"$HOME/.bashrs/sourcefile.fish\"\n").unwrap();
    std::fs::write(config.join("config.fish"), "function fish_prompt; printf 'mine> '; end\n").unwrap();
    let mut session = Command::new("timeout")
        .args(["20", "script", "-qfc", "fish -i", "/dev/null"])
        .env("HOME", &home.dir)
        .env("TERM", "xterm")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CACHE_HOME")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("`script` (util-linux) gives fish its terminal");
    use std::io::Write as _;
    session.stdin.take().expect("piped").write_all(b"exit\r").unwrap();
    let out = session.wait_with_output().unwrap();
    let screen = console::strip_ansi_codes(&String::from_utf8_lossy(&out.stdout)).into_owned();
    assert!(out.status.success(), "the session ended on its own: {screen}");
    assert!(screen.contains("(UTC)]mine> "), "the first prompt: {screen}");
}
