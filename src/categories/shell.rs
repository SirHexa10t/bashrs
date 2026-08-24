//! Shell mechanics: understand how the shell treats your command line, and control the shell
//! session itself.
//!
//! - [`shell_args_print`](commands::args_print) shows what actually reached the program — the argv
//!   shell produced after it expanded, split, and unquoted your line.
//! - `shell_new` / `shell_bare` / `shell_sudo` start or move between shells.
//!
//! Two of the three session commands must run in the *calling* shell — `exec bash` replaces the
//! shell that invoked it, and a child process cannot replace its parent — so they are
//! `#[shell_body]` commands, declared here and emitted into `sourcefile.sh` as inline shell.
//! `shell_sudo` is different in kind: it *spawns* a nested shell and returns when that shell
//! exits, which a binary does perfectly well, so its logic is Rust and its wrapper is the ordinary
//! generated one-liner.
//!
//! `shell_new` is referenced by the ALT+N keybind (see [`crate::conf::keybinds`]) and by
//! `bashrs_compile` (via `#[after]`); `shell_bare` by CTRL+ALT+N. Both resolve at call time, so
//! nothing depends on where in the sourcefile these land.
//!
//! The pair around `_BASHRS_BARE` (underscore: machinery-internal, not a user knob): `shell_bare`
//! starts a fresh shell with the flag exported, and the sourcefile's guard (emitted by
//! [`crate::cli`]'s generator, beside the interactivity check) consumes it — `unset` + return — so
//! the shell comes up bashrs-free AND with a clean environment. One-shot by design: the flag never
//! lingers, so any new shell (or re-sourcing the file by hand) arms bashrs again; there is no
//! special way back to remember.

#[bashrs_macros::category(command = ShellCommand, prefix = "shell_")]
mod commands {
    use crate::support::args::NoArgs;
    use crate::support::shell_quote;
    use crate::support::superuser;
    use clap::Args;

    /// The words a shell hands a program are not the words you typed: by the time argv exists the
    /// shell has already expanded variables and globs, split on whitespace, and stripped the
    /// quotes that told it where the boundaries were. `shell_args_print` is a program that does
    /// nothing but report that argv back — so `shell_args_print "$x"` and `shell_args_print $x` are a
    /// before/after of word-splitting you can see. A classic teaching tool; demonstrated well at
    /// https://www.youtube.com/watch?v=w-PgWIZm5Qs .
    ///
    /// Every value is quoted, so an empty argument still shows (`''`) and a run of spaces inside
    /// one is visible where the shell put it — the whole point being to see the boundaries the
    /// shell chose, not the ones you meant.
    pub fn args_print(args: ArgsPrintArgs) {
        let count = args.args.len();
        println!("{count} arg{}:", if count == 1 { "" } else { "s" });
        for (index, arg) in args.args.iter().enumerate() {
            // 1-based, to read alongside the shell's own `$1 $2 …`. The quoting — always-on, so an
            // empty arg shows as `''` and every boundary is explicit — is the shared one.
            println!("  arg{}: {}", index + 1, shell_quote::quote(arg));
        }
    }

    /// Every token after the command, verbatim — hyphen-led ones (`-x`, `--foo`) included, since a
    /// tool for seeing what the shell passed must not itself eat the flags. What arrives here is
    /// already post-expansion: the shell has finished with it before the binary starts.
    #[derive(Args)]
    pub struct ArgsPrintArgs {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    }

    /// Start a fresh shell session
    #[name("shell_new")]
    #[shell_body("exec bash")]
    pub fn new() {}

    /// Fresh shell WITHOUT bashrs (one-shot: any later shell, or re-sourcing, arms it again)
    #[name("shell_bare")]
    #[shell_body("_BASHRS_BARE=1 exec bash")]
    pub fn bare() {}

    /// Move into a root shell with bashrs sourced — for running several elevated commands
    /// without a password each time. `exit` returns here. Files written inside are root-owned
    #[name("shell_sudo")]
    pub fn sudo(_args: NoArgs) {
        if superuser::is_root() {
            eprintln!("shell_sudo: this is already a root shell");
            return;
        }
        // Probed before sudo can mint one, so only an elevation this command earned is
        // dropped afterwards — a ticket the user already held belongs to their own workflow.
        let had_ticket = superuser::ticket_exists();
        let status = superuser::command().args(_root_shell_argv()).status();
        superuser::revoke_ours(had_ticket);
        match status {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(err) => {
                eprintln!("shell_sudo: could not start the root shell: {err}");
                std::process::exit(1);
            }
        }
    }

    /// What `sudo` runs: `env HOME=<home> bash --rcfile <sourcefile> -i`.
    ///
    /// Two details are load-bearing. **HOME is carried in by `env`**, not by `sudo HOME=…`
    /// (default sudoers refuses to set variables without `SETENV`) and not left to root's own
    /// HOME — the sourcefile locates the binary through `$HOME/.bashrs`, so under root's HOME
    /// the guard would find no binary and return early, handing back a bare root shell with
    /// no error to explain it. **`env` also spares us a nested `bash -c "…"` string**: every
    /// argument goes through as its own argv entry, so a home directory with spaces or quotes
    /// needs no escaping and cannot be re-parsed as shell.
    fn _root_shell_argv() -> Vec<std::ffi::OsString> {
        let mut home = std::ffi::OsString::from("HOME=");
        home.push(crate::conf::home());
        vec![
            "env".into(),
            home,
            "bash".into(),
            "--rcfile".into(),
            crate::conf::sourcefile().into_os_string(),
            "-i".into(),
        ]
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_root_shell_carries_this_users_home_and_sourcefile() {
            let argv = _root_shell_argv();
            let text: Vec<String> =
                argv.iter().map(|a| a.to_string_lossy().into_owned()).collect();
            assert_eq!(text[0], "env", "`env` sets HOME without sudoers' permission to");
            assert_eq!(
                text[1],
                format!("HOME={}", crate::conf::home().display()),
                "root's own HOME would hide the binary from the sourcefile's guard"
            );
            assert_eq!(&text[2..4], ["bash", "--rcfile"]);
            assert!(text[4].ends_with("sourcefile.sh"), "{text:?}");
            assert_eq!(text[5], "-i", "an rcfile is only read by an interactive shell");
        }

        #[test]
        fn every_argument_is_its_own_argv_entry() {
            // The whole reason for `env` over `bash -c "…"`: nothing here is a shell string,
            // so no path needs quoting and none can be re-parsed.
            for arg in _root_shell_argv() {
                let text = arg.to_string_lossy();
                assert!(
                    !text.contains(';') && !text.contains('"') && !text.contains('\''),
                    "argv entry looks like shell source: {text}"
                );
            }
        }
    }
}
