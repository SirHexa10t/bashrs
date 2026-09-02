//! Shell mechanics: understand how the shell treats your command line, and control the shell
//! session itself.
//!
//! - [`shell_args_print`](commands::args_print) shows what actually reached the program — the argv
//!   shell produced after it expanded, split, and unquoted your line.
//! - [`shell_def`](commands::def) shows what a name *is* to the current shell — alias, function,
//!   builtin, binaries, variable — in every meaning it holds at once.
//! - `shell_new` / `shell_bare` / `sudo_bashrs` start or move between shells.
//!
//! Two of the three session commands must run in the *calling* shell — `exec bash` replaces the
//! shell that invoked it, and a child process cannot replace its parent — so they are
//! `#[shell_body]` commands, declared here and emitted into `sourcefile.sh` as inline shell.
//! `sudo_bashrs` is different in kind: it *spawns* a nested shell and returns when that shell
//! exits, which a binary does perfectly well, so its logic is Rust and its wrapper is the ordinary
//! generated one-liner. Its name breaks the `shell_` prefix deliberately — it reads as "sudo,
//! but a BASHRS shell": what you get is not a bare root prompt but a root session with the
//! sourcefile re-armed, and the name should say so before you are root to find out.
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

    /// 'define' the given name(s): what runs first, then everything it outranks, each labeled
    ///
    /// A name can mean several things at once — an alias over a function over a binary — and
    /// the shell resolves in a fixed order: alias, then function, then builtin, then PATH.
    /// `shell_def` prints the winner FIRST, marked as what runs, and every outranked meaning
    /// after it, each labeled with how to reach it anyway (`\name` skips the alias,
    /// `builtin name` skips both, `command name` runs the binary). A variable of the same name
    /// is orthogonal to all of that and reported as such, value and attributes included.
    /// Commentary prints blue so it can't be mistaken for the definitions themselves, and the
    /// rank talk only appears when a name actually holds rival meanings — a sole meaning is
    /// stated plainly.
    ///
    /// Split along what each side alone can know: only the calling shell sees its aliases,
    /// functions, unexported variables, and builtin set, so the wrapper's `#[piped]` prefix
    /// probes those four (`--`-terminated, so a name starting with `-` can't read as flags)
    /// and pipes them in NUL-delimited — a shell string can't contain NUL, so no probe output
    /// can fake a boundary. One `printf` per name frames all four: command substitutions run
    /// independently (a failing probe yields an empty field, never blocks the next), and the
    /// trailing newline `$()` strips is one [`_def_report`] trims anyway. The probes' stderr is
    /// discarded once, on the loop's `done` (a missing meaning is data here, not an error) —
    /// and only there: the redirect sits inside the pipeline segment, so the BINARY's stderr,
    /// which carries the not-found reports, still reaches the terminal. Everything decidable out-of-shell IS out of the shell: ranking,
    /// labels, the PATH walk (this process inherits the caller's PATH), and exit codes are
    /// Rust — [`_def_report`] — where they are unit-tested instead of being an if/case forest
    /// in a string of bash. Under zsh the `type -at` probe fails silently (empty field) and
    /// the report derives the ranking from which probes answered, claiming nothing about
    /// builtins it cannot see.
    #[name("shell_def")]
    #[piped(r#"local __n; for __n in "$@"; do printf '%s\0%s\0%s\0%s\0' "$(alias -- "$__n")" "$(declare -f -- "$__n")" "$(declare -p -- "$__n")" "$(type -at -- "$__n")"; done 2>/dev/null"#)]
    pub fn def(args: DefArgs) {
        let mut piped = Vec::new();
        use std::io::Read as _;
        if std::io::stdin().read_to_end(&mut piped).is_err() {
            eprintln!("shell_def: could not read the probe stream");
            std::process::exit(2);
        }
        let dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).filter(|dir| !dir.as_os_str().is_empty()).collect())
            .unwrap_or_default();
        match _def_report(&args.names, &piped, &dirs) {
            Err(protocol) => {
                eprintln!("shell_def: {protocol}");
                std::process::exit(2);
            }
            Ok((report, missing)) => {
                print!("{report}");
                for name in &missing {
                    eprintln!("shell_def: '{name}' is nothing here - no alias, function, builtin, binary, or variable");
                }
                if !missing.is_empty() {
                    std::process::exit(1);
                }
            }
        }
    }

    /// The names to define. Hyphen-led values allowed: a variable or alias may be named `-x`,
    /// and a tool for asking what a name is must not eat it as a flag.
    #[derive(Args)]
    pub struct DefArgs {
        #[arg(value_name = "NAME", num_args = 1.., required = true, allow_hyphen_values = true)]
        names: Vec<String>,
    }

    /// What the wrapper's probes said about one name, in the order they were piped.
    struct Probes<'a> {
        /// `alias -- NAME` — the exact re-creatable definition, or empty.
        alias: &'a str,
        /// `declare -f -- NAME` — the function body, or empty.
        function: &'a str,
        /// `declare -p -- NAME` — the variable's declaration, attributes included, or empty.
        variable: &'a str,
        /// `type -at -- NAME` — every meaning in resolution order, one per line (bash only;
        /// empty under zsh, where the ranking is derived from the other probes instead).
        kinds: &'a str,
    }

    /// The whole report: the piped probe stream split against `names`, each name described.
    /// `Ok((report, missing))` — the printable text, plus the names that meant nothing (the
    /// caller reports those on stderr and exits 1). `Err` is a protocol violation: the stream
    /// didn't hold four fields per name, i.e. the binary was run without its shell wrapper.
    fn _def_report(
        names: &[String],
        piped: &[u8],
        path_dirs: &[std::path::PathBuf],
    ) -> Result<(String, Vec<String>), String> {
        let mut fields: Vec<&[u8]> = piped.split(|byte| *byte == 0).collect();
        // Every field is NUL-terminated, so a well-formed stream splits into 4N fields plus
        // one empty trailer.
        if fields.last().is_some_and(|last| last.is_empty()) {
            fields.pop();
        }
        if fields.len() != names.len() * 4 {
            return Err(format!(
                "probe stream holds {} fields for {} names (4 expected per name) — \
                 shell_def is driven by its shell wrapper, not run directly",
                fields.len(),
                names.len()
            ));
        }
        let mut report = String::new();
        let mut missing = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let text: Vec<std::borrow::Cow<'_, str>> =
                fields[index * 4..index * 4 + 4].iter().map(|f| String::from_utf8_lossy(f)).collect();
            let probes = Probes {
                alias: text[0].trim_end(),
                function: text[1].trim_end(),
                variable: text[2].trim_end(),
                kinds: text[3].trim_end(),
            };
            if index > 0 {
                report.push('\n');
            }
            match _describe(name, &probes, &_path_binaries(name, path_dirs)) {
                Some(described) => report.push_str(&described),
                None => missing.push(name.clone()),
            }
        }
        Ok((report, missing))
    }

    /// A commentary line of the report, blue — so the eye can split OUR narration from the
    /// definitions themselves (alias text, function bodies, paths, declarations), which stay
    /// exactly as the shell spelled them.
    fn _note(text: &str) -> String {
        crate::support::doc_style::_scoped(
            &crate::support::doc_style::_wrap(&[&crate::support::theme::Basic::Blue]),
            text,
        )
    }

    /// One name's story: the effective meaning first, every outranked meaning after it labeled
    /// with its escape hatch, a same-named variable last, labeled orthogonal. Rank talk ("the
    /// top rank, this is what runs") appears only when there IS a rivalry — a name with a
    /// single meaning is stated plainly. `None` when the name means nothing at all.
    fn _describe(name: &str, probes: &Probes<'_>, binaries: &[String]) -> Option<String> {
        // The effective kind: bash's `type -at` first line is the shell's own verdict. With no
        // verdict (zsh), derive it from which probes answered — the precedence is fixed, so
        // presence is enough; only builtin/keyword can't be derived, so they're never claimed.
        let mut kinds = probes.kinds.lines();
        let effective = kinds.next().map(str::to_string).or_else(|| {
            if !probes.alias.is_empty() {
                Some("alias".into())
            } else if !probes.function.is_empty() {
                Some("function".into())
            } else if !binaries.is_empty() {
                Some("file".into())
            } else {
                None
            }
        });
        let outranked: Vec<&str> = kinds.collect();
        let function_outranked = outranked.contains(&"function")
            || (probes.kinds.is_empty()
                && effective.as_deref() == Some("alias")
                && !probes.function.is_empty());
        // Whether anything is outranked at all — only then is "this is what runs" saying
        // something, and only then does the winner get the qualifier.
        let contested = function_outranked
            || outranked.contains(&"builtin")
            || match effective.as_deref() {
                Some("file") => binaries.len() > 1,
                _ => !binaries.is_empty(),
            };
        let qualified = |plain: &str, ranked: &str| if contested { ranked.to_string() } else { plain.to_string() };

        let mut out = String::new();
        match effective.as_deref() {
            Some("alias") => {
                out += &format!(
                    "{}\n",
                    _note(&qualified(
                        &format!("{name} is an ALIAS:"),
                        &format!("{name} is an ALIAS - the top rank, this is what runs:"),
                    ))
                );
                out += &format!("{}\n", probes.alias);
            }
            Some("function") => {
                out += &format!(
                    "{}\n",
                    _note(&qualified(
                        &format!("{name} is a FUNCTION:"),
                        &format!("{name} is a FUNCTION - this is what runs:"),
                    ))
                );
                out += &format!("{}\n", probes.function);
            }
            Some("builtin") => {
                out += &format!(
                    "{}\n",
                    _note(&qualified(
                        &format!("{name} is a shell BUILTIN"),
                        &format!("{name} is a shell BUILTIN - this is what runs"),
                    ))
                );
            }
            Some("keyword") => out += &format!("{}\n", _note(&format!("{name} is a shell KEYWORD"))),
            Some("file") => {
                let first = binaries.first().map_or("(not on this PATH)", String::as_str);
                let label = qualified(
                    &format!("{name} is a BINARY:"),
                    &format!("{name} is a BINARY - this is what runs:"),
                );
                out += &format!("{} {first}\n", _note(&label));
            }
            _ => {}
        }
        if function_outranked {
            out += &format!("{}\n", _note(&format!("outranked FUNCTION - still defined, runs via \\{name}:")));
            out += &format!("{}\n", probes.function);
        }
        if outranked.contains(&"builtin") {
            out += &format!("{}\n", _note(&format!("outranked BUILTIN - runs via: builtin {name}")));
        }
        if effective.as_deref() == Some("file") {
            if binaries.len() > 1 {
                out += &format!("{}\n", _note("later in PATH (not run):"));
                for path in &binaries[1..] {
                    out += &format!("{path}\n");
                }
            }
        } else if !binaries.is_empty() {
            out += &format!(
                "{}\n",
                _note(&format!("outranked BINARIES (not run) - the first runs via: command {name}"))
            );
            for path in binaries {
                out += &format!("{path}\n");
            }
        }
        if !probes.variable.is_empty() {
            out += &format!(
                "{} {}\n",
                _note(&format!("also a VARIABLE, unrelated to running '{name}':")),
                probes.variable
            );
        }
        (!out.is_empty()).then_some(out)
    }

    /// Every executable named `name` on the caller's PATH, in PATH order — this process
    /// inherits that PATH, so no shell probe is needed for it. Mirrors `type -aP`.
    fn _path_binaries(name: &str, dirs: &[std::path::PathBuf]) -> Vec<String> {
        use std::os::unix::fs::PermissionsExt as _;
        dirs.iter()
            .map(|dir| dir.join(name))
            .filter(|path| {
                path.is_file()
                    && path.metadata().is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
            })
            .map(|path| path.display().to_string())
            .collect()
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
    #[name("sudo_bashrs")]
    pub fn sudo(_args: NoArgs) {
        if superuser::is_root() {
            eprintln!("sudo_bashrs: this is already a root shell");
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
                eprintln!("sudo_bashrs: could not start the root shell: {err}");
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
    mod def_tests {
        use super::*;

        const NONE: &Probes<'_> =
            &Probes { alias: "", function: "", variable: "", kinds: "" };

        /// The originating complaint, exactly: an alias laid over a generated wrapper function
        /// must read in rank order — the alias first, marked as what runs, the function below
        /// it labeled outranked (it IS still defined) with the `\name` escape hatch. The
        /// commentary is blue ([`_note`]); the definitions stay exactly as the shell spelled
        /// them, uncoloured.
        #[test]
        fn an_alias_over_a_function_reads_in_rank_order() {
            let probes = Probes {
                alias: "alias lll='echo ll'",
                function: "lll () \n{ \n    \"$HOME/.bashrs/bashrs\" lll \"$@\"\n}",
                variable: "",
                kinds: "alias\nfunction",
            };
            let text = _describe("lll", &probes, &[]).expect("both meanings exist");
            assert_eq!(
                text,
                format!(
                    "{}\nalias lll='echo ll'\n{}\nlll () \n{{ \n    \"$HOME/.bashrs/bashrs\" lll \"$@\"\n}}\n",
                    _note("lll is an ALIAS - the top rank, this is what runs:"),
                    _note("outranked FUNCTION - still defined, runs via \\lll:"),
                )
            );
        }

        /// Rank talk exists to settle rivalries; a name with a single meaning gets none of it —
        /// just what the thing is. (An accompanying variable doesn't count as a rival: it never
        /// runs.)
        #[test]
        fn a_sole_meaning_is_stated_without_rank_talk() {
            let alias_only =
                _describe("zz", &Probes { alias: "alias zz='ls -l'", ..*NONE }, &[]).unwrap();
            assert_eq!(alias_only, format!("{}\nalias zz='ls -l'\n", _note("zz is an ALIAS:")));

            let function_only = _describe(
                "fun",
                &Probes { function: "fun () { true }", kinds: "function", ..*NONE },
                &[],
            )
            .unwrap();
            assert_eq!(function_only, format!("{}\nfun () {{ true }}\n", _note("fun is a FUNCTION:")));

            let one_binary = vec!["/usr/bin/xz".to_string()];
            let binary_only = _describe("xz", &Probes { kinds: "file", ..*NONE }, &one_binary).unwrap();
            assert_eq!(binary_only, format!("{} /usr/bin/xz\n", _note("xz is a BINARY:")));

            for text in [&alias_only, &function_only, &binary_only] {
                assert!(!text.contains("what runs") && !text.contains("top rank"), "{text}");
            }
        }

        /// Without bash's `type -at` verdict (zsh sends an empty kinds field), the rank is
        /// derived from which probes answered — precedence is fixed, so presence is enough —
        /// and nothing is claimed about builtins, which only bash can report.
        #[test]
        fn a_missing_kinds_verdict_is_derived_not_guessed() {
            let probes = Probes {
                alias: "alias lll='echo ll'",
                function: "lll () { true }",
                variable: "",
                kinds: "",
            };
            let text = _describe("lll", &probes, &[]).unwrap();
            assert!(
                text.starts_with(&_note("lll is an ALIAS - the top rank, this is what runs:")),
                "{text}"
            );
            assert!(text.contains("outranked FUNCTION"), "{text}");
            assert!(!text.contains("BUILTIN"), "builtins are unknowable without the verdict: {text}");
        }

        /// A binary that runs shows its path; later PATH hits are listed but marked not-run.
        /// When something outranks the binaries entirely, they all demote to `command name`.
        #[test]
        fn path_binaries_split_into_the_runner_and_the_shadowed() {
            let binaries = vec!["/usr/bin/grep".to_string(), "/bin/grep".to_string()];
            let ran = _describe("grep", &Probes { kinds: "file\nfile", ..*NONE }, &binaries).unwrap();
            assert_eq!(
                ran,
                format!(
                    "{} /usr/bin/grep\n{}\n/bin/grep\n",
                    _note("grep is a BINARY - this is what runs:"),
                    _note("later in PATH (not run):"),
                )
            );

            let shadowed = _describe(
                "grep",
                &Probes { alias: "alias grep='grep --color=auto'", kinds: "alias\nfile\nfile", ..*NONE },
                &binaries,
            )
            .unwrap();
            assert!(
                shadowed.contains(&format!(
                    "{}\n/usr/bin/grep\n/bin/grep\n",
                    _note("outranked BINARIES (not run) - the first runs via: command grep")
                )),
                "{shadowed}"
            );
        }

        /// Builtins and keywords are their own stories; a variable is orthogonal to all of it
        /// and reported even when it is the only meaning.
        #[test]
        fn builtins_keywords_and_variables_are_labeled_for_what_they_are() {
            let cd = _describe("cd", &Probes { kinds: "builtin", ..*NONE }, &[]).unwrap();
            assert_eq!(cd, format!("{}\n", _note("cd is a shell BUILTIN")));
            let kw = _describe("if", &Probes { kinds: "keyword", ..*NONE }, &[]).unwrap();
            assert_eq!(kw, format!("{}\n", _note("if is a shell KEYWORD")));
            // The variable's declaration itself stays uncoloured — it is quoted shell, not
            // commentary.
            let var = _describe(
                "HOME",
                &Probes { variable: "declare -x HOME=\"/home/u\"", ..*NONE },
                &[],
            )
            .unwrap();
            assert_eq!(
                var,
                format!(
                    "{} declare -x HOME=\"/home/u\"\n",
                    _note("also a VARIABLE, unrelated to running 'HOME':")
                )
            );
            assert_eq!(_describe("nothing", NONE, &[]), None, "no meaning anywhere is None");
        }

        /// The report layer: fields are cut on NUL (4 per name, one empty trailer), names are
        /// separated by a blank line, meaningless names collect into `missing`, and a stream
        /// that doesn't match the names is refused as a protocol error — the symptom of
        /// running the binary without its wrapper.
        #[test]
        fn the_report_splits_the_stream_and_collects_the_missing() {
            let names = vec!["cd".to_string(), "ghost".to_string()];
            let stream = b"\0\0\0builtin\0\0\0\0\0";
            let (report, missing) = _def_report(&names, stream, &[]).unwrap();
            assert_eq!(report, format!("{}\n\n", _note("cd is a shell BUILTIN")));
            assert_eq!(missing, vec!["ghost"]);

            let err = _def_report(&names, b"only\0two\0", &[]).unwrap_err();
            assert!(err.contains("driven by its shell wrapper"), "{err}");
        }

        /// The PATH walk honors order and the executable bit — a plain file of the right name
        /// is not a binary.
        #[test]
        fn the_path_walk_takes_executables_in_path_order() {
            use std::os::unix::fs::PermissionsExt as _;
            let base = std::env::temp_dir().join(format!("bashrs-def-{}", std::process::id()));
            let (first, second) = (base.join("a"), base.join("b"));
            std::fs::create_dir_all(&first).unwrap();
            std::fs::create_dir_all(&second).unwrap();
            for dir in [&first, &second] {
                let bin = dir.join("tool");
                std::fs::write(&bin, "#!/bin/sh\n").unwrap();
                std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            std::fs::write(first.join("plainfile"), "").unwrap();

            let dirs = vec![first.clone(), second.clone()];
            let hits = _path_binaries("tool", &dirs);
            assert_eq!(hits, vec![
                first.join("tool").display().to_string(),
                second.join("tool").display().to_string(),
            ]);
            assert!(_path_binaries("plainfile", &dirs).is_empty(), "not executable, not a binary");
            assert!(_path_binaries("absent", &dirs).is_empty());
            let _ = std::fs::remove_dir_all(&base);
        }
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
