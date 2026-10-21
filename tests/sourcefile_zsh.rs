//! Behavioral tests of the generated `sourcefile.sh` under zsh, run through a real zsh. Zsh's
//! `.zshrc` sources bash's file — the install wires both rc files — so this is the zsh counterpart
//! of `sourcefile_shell.rs` and `sourcefile_fish.rs`: the same contracts (nothing for
//! non-interactive shells, everything for interactive ones, a one-shot bare session, the session
//! commands starting the shell again), plus the parts zsh does its own way — the greeting at the
//! first prompt, the keybinds, `hg`'s history, a prompt left as it was — and what zsh setups bring
//! with them, aliases of the commands' names among them.
//!
//! Each zsh starts with `-f`, its startup files skipped — the system's and the user's — so the
//! tests see stock zsh and bashrs alone, and each script sources the file itself. `HOME` points at
//! a fresh directory, and every shell starts through [`common::shell`] — detached from the terminal
//! running the tests, and without its bashrs session.

mod common;

use common::{leading_shell, lines, shell, Captured, Home};
use std::io::Write;
use std::process::{Command, Output, Stdio};

impl Home {
    /// `zsh -f` under this `HOME` — one started from another, as [`shell`] starts it — with the
    /// bash sourcefile's path in `$sourcefile`, so no script has to quote it.
    fn zsh(&self) -> Command {
        self.zsh_from(shell("zsh"))
    }

    /// [`Self::zsh`], but a terminal's first zsh, leading its session — the one that greets.
    fn first_zsh(&self) -> Command {
        self.zsh_from(leading_shell("zsh"))
    }

    /// `zsh` set up as both of those start it.
    fn zsh_from(&self, mut zsh: Command) -> Command {
        zsh.arg("-f")
            .env("HOME", &self.dir)
            .env("sourcefile", self.dir.join(".bashrs/sourcefile.sh"))
            .env("TZ", "UTC")
            .env_remove("ZDOTDIR");
        zsh
    }

    /// Run `script` with `zsh -c` — interactive (`-i`) or not.
    fn run(&self, interactive: bool, script: &str) -> Output {
        let mut zsh = self.zsh();
        if interactive {
            zsh.arg("-i");
        }
        zsh.args(["-c", script]).output().expect("zsh is needed for the zsh tests")
    }

    /// [`Self::run`], interactive, returning stdout.
    fn interactive(&self, script: &str) -> Captured {
        Captured::new(&self.run(true, script).stdout)
    }

    /// [`prompted`], in [`Self::zsh`], returning stdout.
    fn at_prompts(&self, prompts: &[&str]) -> Captured {
        Captured::new(&prompted(self.zsh(), prompts).stdout)
    }

    /// An interactive bash that has sourced the file, run `script` — for reading what bash
    /// defined, to hold zsh to the same.
    fn bash(&self, script: &str) -> String {
        let sourced = format!("source \"$HOME/.bashrs/sourcefile.sh\" 2>/dev/null; {script}");
        // No `inputrc`: the macros a system's readline setup adds are none of bashrs's.
        let out = shell("bash")
            .args(["--norc", "-i", "-c", &sourced])
            .env("HOME", &self.dir)
            .env("INPUTRC", "/dev/null")
            .output()
            .expect("bash");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

/// `zsh`, interactive, answering `prompts`, one command line each, read from its stdin — so zsh
/// runs its `precmd` hooks before each, as it does before every prompt it draws (for `-c` it runs
/// none). The prompts themselves go to stderr.
fn prompted(mut zsh: Command, prompts: &[&str]) -> Output {
    zsh.arg("-i");
    fed(zsh, &(prompts.join("\n") + "\n"))
}

/// `command`'s output, with `input` on its stdin.
fn fed(mut command: Command, input: &str) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("zsh is needed for the zsh tests");
    child.stdin.take().expect("stdin is piped").write_all(input.as_bytes()).expect("write stdin");
    child.wait_with_output().expect("zsh")
}

// ——— The file ————————————————————————————————————————————————————————————

/// Zsh itself judges the whole file (`-n`: nothing runs) — bash's words in bash's blocks, its own
/// in zsh's. The tests below source it and check what got defined, so a syntax error in a later
/// section would fail none of them.
#[test]
fn the_whole_file_parses_in_zsh() {
    let home = Home::new("zsh_parse");
    let parse = home.zsh().arg("-n").arg(home.dir.join(".bashrs/sourcefile.sh")).output().expect("zsh");
    assert!(parse.status.success(), "zsh rejects the file: {}", Captured::new(&parse.stderr));
}

// ——— Who gets bashrs ———————————————————————————————————————————————————————

/// A script can source the file too: it must see no output and no definitions — above all not
/// the `python3` function.
#[test]
fn non_interactive_zsh_inherits_nothing_at_all() {
    let home = Home::new("zsh_inert");
    let out = home.run(false, "source $sourcefile; echo status:$?; print -r -- lll:$+functions[lll] python3:$+functions[python3]");
    assert_eq!(&*Captured::new(&out.stdout), "status:0\nlll:0 python3:0\n", "{}", Captured::new(&out.stderr));
    let silence = home.run(false, "source $sourcefile");
    assert_eq!(silence.stdout, b"", "a non-interactive source must write nothing to stdout");
}

/// Arming bashrs runs nothing that can complain: an interactive source is silent on stderr.
#[test]
fn interactive_sourcing_writes_nothing_to_stderr() {
    let home = Home::new("zsh_quiet");
    let out = home.run(true, "source $sourcefile");
    assert_eq!(&*Captured::new(&out.stderr), "", "sourcing must not complain");
}

/// Whatever bash defines from the file, zsh does too — read from the two real shells, not from a
/// list. Only bash's completion helpers are bash's alone: the completion block is.
#[test]
fn every_function_bash_gets_zsh_gets_too() {
    let home = Home::new("zsh_parity");
    let bash = home.bash("declare -F");
    let zsh = home.interactive("source $sourcefile; print -rl -- ${(k)functions}");
    let zsh: std::collections::BTreeSet<&str> = zsh.lines().collect();
    let bash: Vec<&str> =
        bash.lines().filter_map(|line| line.strip_prefix("declare -f ")).filter(|name| !name.starts_with("_bashrs_")).collect();
    assert!(bash.len() > 100, "bash defined too little to compare: {bash:?}");
    let missing: Vec<&&str> = bash.iter().filter(|name| !zsh.contains(**name)).collect();
    assert!(missing.is_empty(), "defined in bash but not in zsh: {missing:?}");
}

/// Sourcing sets the variables it means to — `GREP_COLORS`, bash's `HISTTIMEFORMAT` (inert in
/// zsh), the record of aliases set aside, the `precmd_functions` the greeting waits in, in a
/// terminal's first zsh — and `PATH` it only extends; nothing else is left behind. The line editor
/// is loaded first, as in any zsh that draws a prompt: under `-c` the keybinds would load it, and
/// its own variables would count as the file's.
#[test]
fn sourcing_leaves_no_variable_of_its_own_behind() {
    let home = Home::new("zsh_novars");
    let script = "zmodload zsh/zle; before=(${(k)parameters} before); source $sourcefile; print -rl -- ${${(k)parameters}:|before}";
    let out = Captured::new(&home.first_zsh().args(["-i", "-c", script]).output().expect("zsh").stdout);
    let mut new = lines(&out);
    new.sort_unstable();
    assert_eq!(new, ["GREP_COLORS", "HISTTIMEFORMAT", "__bashrs_set_aside", "precmd_functions"], "{out}");
}

/// `shell_bare`'s flag: the source defines NOTHING and consumes the flag on the way out, and the
/// very next source arms everything again — no manual unset.
#[test]
fn a_bare_session_defines_nothing_and_consumes_its_flag() {
    let home = Home::new("zsh_bare");
    let out = home.interactive(
        "export _BASHRS_BARE=1; source $sourcefile; echo status:$?
         print -r -- lll-bare:$+functions[lll]
         print -r -- flag:${_BASHRS_BARE-gone}
         source $sourcefile; print -r -- lll-back:$+functions[lll]",
    );
    assert_eq!(lines(&out), ["status:0", "lll-bare:0", "flag:gone", "lll-back:1"], "{out}");
}

/// The session commands start zsh again — `"${ZSH_NAME:-bash}"` names zsh there: `shell_new` a
/// fresh shell, nothing of the old one's left but what it exported, and `shell_bare` one carrying
/// the flag that keeps bashrs out of it. The new shell reads its commands where the old one would
/// have, from stdin, so it reports on itself.
#[test]
fn the_session_commands_start_zsh_again() {
    let home = Home::new("zsh_session");
    let report = "print -r -- \"$ZSH_NAME ${kept-gone} ${_BASHRS_BARE-unset}\"\n";
    for (command, expected) in [("shell_new", "zsh gone unset\n"), ("shell_bare", "zsh gone 1\n")] {
        let mut zsh = home.zsh();
        zsh.args(["-i", "-c", &format!("source $sourcefile; kept=here; {command}")]);
        let out = fed(zsh, report);
        assert_eq!(&*Captured::new(&out.stdout), expected, "{command}: {}", Captured::new(&out.stderr));
    }
}

/// `..` is a function name zsh takes as bash does: it climbs one directory, through the `cd`
/// override.
#[test]
fn the_dotdot_function_climbs_one_directory() {
    let home = Home::new("zsh_dotdot");
    std::fs::create_dir_all(home.dir.join("outer/inner")).unwrap();
    let out = home.interactive("source $sourcefile; cd $HOME/outer/inner >/dev/null; .. >/dev/null; pwd");
    assert_eq!(out.trim_end(), home.dir.join("outer").to_str().unwrap());
}

// ——— What zsh writes its own way ——————————————————————————————————————————

/// An alias of a command's name, set before the file is sourced — `alias g=git`, as oh-my-zsh's
/// git plugin sets it — breaks nothing. Zsh refuses `g() {…}` with the alias in place, and its parse
/// error ended the sourcing there, leaving the rest of the file undefined; `function g` takes the
/// name as written.
#[test]
fn an_alias_of_a_command_s_name_breaks_nothing() {
    let home = Home::new("zsh_aliased");
    let out = home.run(
        true,
        "alias g=git; alias ..='cd ..'; source $sourcefile
         print -r -- g:$+functions[g] dotdot:$+functions[..] errcho:$+functions[errcho] git:$+functions[git] typed:$(whence -w g)",
    );
    assert_eq!(&*Captured::new(&out.stderr), "", "sourcing must not complain");
    assert_eq!(
        &*Captured::new(&out.stdout),
        "g:1 dotdot:1 errcho:1 git:0 typed:g: function\n",
        "the whole file, nothing taken over, and bashrs's command the one typed"
    );
}

/// Every alias of a name the file defines a function for, set before the file is sourced — as
/// oh-my-zsh's git plugin sets `g` — is set aside: typing the name runs bashrs's command. Zsh
/// keeps the alias in its own table, disabled, so `enable -a` brings it back, and `shell_def`
/// shows it with the line that does the same in either shell. An unrelated alias stays.
/// Exhaustive, and read from zsh itself: every name the file gives zsh is aliased first.
#[test]
fn aliases_of_the_commands_names_are_set_aside() {
    let home = Home::new("zsh_set_aside");
    let defined = home.interactive("source $sourcefile; print -rl -- ${(k)functions}");
    let names: Vec<&str> = defined.lines().filter(|name| !name.starts_with("__bashrs_")).collect();
    assert!(names.len() > 100, "zsh defined too little to check: {names:?}");
    let aliases: String = names.iter().map(|name| format!("alias -- {name}=false\n")).collect();
    let out = home.interactive(&format!(
        "{aliases}alias keep=true\nsource $sourcefile\n\
         for n in {}; do [[ $(whence -w -- $n) == \"$n: function\" ]] || echo \"still an alias: $n\"; done\n\
         shell_def g\nalias keep\nenable -a g; echo \"enabled again: $(whence -w g)\"",
        names.join(" ")
    ));
    let out = out.plain();
    assert!(!out.contains("still an alias"), "{out}");
    assert!(out.contains("set-aside ALIAS") && out.contains("\nalias g='false'\n"), "shell_def shows it, restorable: {out}");
    assert!(out.contains("keep=true"), "an unrelated alias stays: {out}");
    assert!(out.ends_with("enabled again: g: alias\n"), "zsh kept the alias, disabled: {out}");
}

/// With `sh_word_split` on — a bash-compatibility option some users set — an alias whose name
/// holds a space (`x g`) is still one alias to the set-aside, not an `x` and a `g`: unquoted, the
/// `g` would be "set aside" too, and zsh would complain that it is no alias.
#[test]
fn an_alias_name_with_a_space_stays_whole_under_sh_word_split() {
    let home = Home::new("zsh_wordsplit");
    let out = home.run(true, "setopt shwordsplit; alias 'x g=echo spaced'; source $sourcefile; print -r -- \"kept: ${aliases[x g]}\"");
    assert_eq!(&*Captured::new(&out.stderr), "", "sourcing must not complain");
    assert_eq!(&*Captured::new(&out.stdout), "kept: echo spaced\n");
}

/// Zsh keeps its own prompt — as often as not a theme's, with a time of its own: sourcing leaves
/// `PS1` as it was, prompt after prompt, and bash's clock out of it.
#[test]
fn zsh_keeps_its_own_prompt() {
    let home = Home::new("zsh_prompt");
    let out = home.at_prompts(&["PS1='$ '", "source $sourcefile", "print -r -- $PS1", "print -r -- $PS1"]);
    assert_eq!(lines(&out), ["$ ", "$ "], "{out}");
}

/// …and so does a zsh started from a bash that sourced the file: bash's clocked prompt is bash's
/// own, never handed down, so zsh draws its own rather than bash's syntax, raw.
#[test]
fn a_zsh_started_from_bash_keeps_its_own_prompt() {
    let home = Home::new("zsh_from_bash");
    let out = home.bash("zsh -f -i -c 'print -r -- \"$PS1\"' 2>/dev/null");
    assert!(!out.trim().is_empty() && !out.contains("(UTC)") && !out.contains("\\u@\\h"), "zsh's own prompt: {out}");
}

/// The greeting waits for a terminal's first zsh's first prompt: made during zsh's start-up, it is
/// output that a theme's instant prompt (Powerlevel10k's) reports as a fault at every shell start.
/// It comes once, however often the file was sourced first — printed by the file itself, styled —
/// and the hook that made it is gone after.
#[test]
fn the_greeting_waits_for_zsh_s_first_prompt() {
    let home = Home::new("zsh_greet");
    let start_up = home.first_zsh().args(["-i", "-c", "source $sourcefile; source $sourcefile; print -r -- hooks:${#precmd_functions}"]).output().expect("zsh");
    assert_eq!(&*Captured::new(&start_up.stdout), "hooks:1\n", "nothing during start-up, and one hook waiting");
    let out = prompted(home.first_zsh(), &["source $sourcefile; source $sourcefile", "print -r -- hooks:$precmd_functions greet:$+functions[__bashrs_greet]"]);
    let said = Captured::new(&out.stdout);
    assert!(said.contains('\x1b') && !said.contains("\\033"), "styled: its escapes printed as such, not spelled: {said}");
    let said = said.plain();
    let said = lines(&said);
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(said[0].starts_with("Loaded Bashrs! Press ALT+H"), "{said:?}");
    assert_eq!(said[1], "hooks: greet:0");
}

/// …and only in a terminal's first zsh — the one leading its session: a zsh started from it,
/// which would only repeat the greeting, waits for no prompt to give it at. The nested zsh speaks
/// before it sources the file, so the output shows which zsh greeted.
#[test]
fn a_zsh_started_from_another_does_not_greet() {
    let home = Home::new("zsh_nested_greet");
    let out = prompted(
        home.first_zsh(),
        &["source $sourcefile", "zsh -f -i -c 'print -r -- nested; source $sourcefile; print -r -- hooks:${#precmd_functions}'; print -r -- back"],
    );
    let said = Captured::new(&out.stdout).plain();
    let said = lines(&said);
    assert_eq!(said.len(), 4, "{said:?}");
    assert!(said[0].starts_with("Loaded Bashrs!"), "{said:?}");
    assert_eq!(said[1..], ["nested", "hooks:0", "back"]);
}

/// The warning comes at the first prompt too, on stderr, exactly while an archived config waits in
/// `~/.bashrs` — one named `configrs*.old`, as an archive is now or was before the name gained
/// `.toml.`; once, however many wait — and not for the config itself or for another `.old`: no
/// glob there matches nothing, which in zsh is an error.
#[test]
fn the_warning_comes_while_an_archived_config_waits() {
    let home = Home::new("zsh_warn");
    let bashrs = home.dir.join(".bashrs");
    let stderr = || Captured::new(&prompted(home.first_zsh(), &["source $sourcefile", ":"]).stderr).plain().to_string();
    let warnings = || stderr().matches("an archived config (~/.bashrs/configrs*.old) awaits merging").count();
    std::fs::write(bashrs.join("configrs.toml"), "").unwrap();
    std::fs::write(bashrs.join("other.old"), "").unwrap();
    let quiet = stderr();
    assert!(!quiet.contains("awaits merging") && !quiet.contains("no matches"), "the config itself, or another archive, is no cue: {quiet}");
    std::fs::write(bashrs.join("configrs.toml.2026102112.old"), "").unwrap();
    assert_eq!(warnings(), 1, "an archive waits");
    std::fs::write(bashrs.join("configrs.old"), "").unwrap();
    assert_eq!(warnings(), 1, "one from before the name gained .toml. too — one warning for both");
    std::fs::remove_file(bashrs.join("configrs.toml.2026102112.old")).unwrap();
    assert_eq!(warnings(), 1, "and that one alone");
    // With globbing switched off, the function switches it on for its glob, and back off after.
    let off = prompted(home.first_zsh(), &["setopt noglob; source $sourcefile", "[[ -o noglob ]] && print -r -- still-off"]);
    assert_eq!(Captured::new(&off.stderr).plain().matches("awaits merging").count(), 1, "globbing off, the archive is found");
    assert!(Captured::new(&off.stdout).plain().ends_with("still-off\n"), "and globbing is off again: {}", Captured::new(&off.stdout));
}

/// Zsh binds the keys bash binds, each to the same command — read from the two real shells: each
/// key bash's `bind -s` lists is looked up in zsh's line editor in that same spelling.
#[test]
fn zsh_binds_every_key_bash_binds() {
    let home = Home::new("zsh_keys");
    let bash = home.bash("bind -s");
    let macros: Vec<(&str, &str)> = bash
        .lines()
        .filter_map(|line| {
            let (key, command) = line.strip_prefix('"')?.split_once("\": \"")?;
            Some((key, command.strip_suffix("\\C-j\"")?))
        })
        .collect();
    assert!(macros.len() >= 6, "bash bound too little to compare: {bash}");
    let lookups: String = macros.iter().map(|(key, _)| format!("bindkey -- '{key}'\n")).collect();
    let zsh = home.interactive(&format!("source $sourcefile\n{lookups}"));
    let zsh = lines(&zsh);
    assert_eq!(zsh.len(), macros.len(), "one answer per key: {zsh:?}");
    for ((key, command), bound) in macros.iter().zip(zsh) {
        assert!(bound.ends_with(&format!(" \"{command}^J\"")), "{key} runs `{command}` in bash, but in zsh: {bound}");
    }
}

/// `hg` reads zsh's whole history — not only the last 16 commands its `history` lists by default —
/// each time in the shape bash's `HISTTIMEFORMAT` gives, so the two shells' `hg` read alike. The
/// stand-in binary hands back what the wrapper fed it.
#[test]
fn hg_reads_zsh_s_whole_history_with_bash_s_timestamps() {
    let home = Home::new("zsh_hg");
    let entries: String = (0..20).map(|i| format!(": {}:0;echo entry-{i}\n", 1_760_000_000 + i * 60)).collect();
    std::fs::write(home.dir.join("history"), entries).unwrap();
    let out = home.interactive("setopt extended_history; HISTFILE=$HOME/history; fc -R; source $sourcefile; hg entry");
    let out = lines(&out);
    assert_eq!(out.len(), 20, "every entry, the oldest included: {out:?}");
    assert_eq!(out[0], "    1  2025-10-09_08:53:20  echo entry-0");
    assert_eq!(out[19], "   20  2025-10-09_09:12:20  echo entry-19");
}

/// With no history yet — `hg` the first command of a new shell — zsh's `history` complains; the
/// wrapper keeps that off the screen, as bash's `history` says nothing there.
#[test]
fn hg_says_nothing_of_an_empty_history() {
    let home = Home::new("zsh_hg_empty");
    let out = home.run(true, "source $sourcefile; hg entry; echo status:$?");
    assert_eq!(&*Captured::new(&out.stderr), "", "nothing on stderr");
    assert_eq!(&*Captured::new(&out.stdout), "status:0\n");
}
