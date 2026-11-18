//! Commands for managing bashrs itself.

// `brs_` and not `rs_`, which was tried: `rs` also begins common system commands (`rsync` among
// them), so `rs<TAB>` offered those alongside this category and every completion needed the `_`
// typed out by hand.
#[bashrs_macros::category(command = BashrsCommand, prefix = "brs_")]
mod commands {
    use crate::conf::{config_file, sourcefile_sections};
    use crate::support::args::NoArgs;
    use crate::support::shell_dialect::Shell;
    use crate::support::{exec, streamgrep, theme_code};
    use clap::Args;
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// Everything after the command, forwarded verbatim to COMPILE.sh — the script owns its
    /// flag set (and rejects unknowns loudly), so nothing needs re-declaring here when it
    /// grows one. The cost: clap can't list or TAB-complete the flags; COMPILE.sh's header
    /// documents them (currently --use-stable-cargo, --use-stable-carstay).
    #[derive(Args)]
    pub struct CompileArgs {
        /// Flags for COMPILE.sh: --use-stable-cargo, --use-stable-carstay — see its header
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    }

    /// Recompile and reinstall bashrs by running the project's COMPILE.sh
    #[after("shell_new")] // start a fresh shell after a successful compile
    pub fn compile(_args: CompileArgs) {
        // The project path is captured at build time (`CARGO_MANIFEST_DIR`), so
        // if the project was moved — or COMPILE.sh renamed — since the last
        // compile, report that clearly instead of failing obscurely.
        //
        // Every failure path exits non-zero so the wrapper's `&& shell_new` is
        // skipped: the shell should only restart after a real, successful compile.
        let project = Path::new(env!("CARGO_MANIFEST_DIR"));
        let script = match _locate_script(project, "COMPILE.sh") {
            Ok(script) => script,
            Err(msg) => {
                eprintln!("brs_compile: {msg}");
                std::process::exit(1);
            }
        };
        // The install step (COMPILE.sh's last stage) tells the user what happens next; it reads
        // "a reload will follow" off the process tree — this very invocation in its ancestry —
        // so nothing needs signalling from here (see `conf::install::reload_follows`).
        match Command::new("bash").arg(&script).args(&_args.args).current_dir(project).status() {
            // Signal the wrapper to start a fresh session — only on a real success.
            Ok(status) if status.success() => std::process::exit(crate::conf::RELOAD_EXIT_CODE),
            Ok(status) => {
                eprintln!("brs_compile: COMPILE.sh exited with status: {status}");
                std::process::exit(1);
            }
            Err(err) => {
                eprintln!("brs_compile: could not launch COMPILE.sh: {err}");
                std::process::exit(1);
            }
        }
    }

    /// Run the project's full test suite by running its TEST.sh — every category, the live and
    /// cookie-gated ones included (those self-skip where the machine's setup lacks the pieces)
    pub fn test(_args: NoArgs) {
        let project = Path::new(env!("CARGO_MANIFEST_DIR"));
        let script = match _locate_script(project, "TEST.sh") {
            Ok(script) => script,
            Err(msg) => {
                eprintln!("brs_test: {msg}");
                std::process::exit(1);
            }
        };
        match Command::new("bash").arg(&script).current_dir(project).status() {
            Ok(status) if status.success() => {}
            // TEST.sh's own summary already named what failed — just propagate the verdict.
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(err) => {
                eprintln!("brs_test: could not launch TEST.sh: {err}");
                std::process::exit(1);
            }
        }
    }

    #[derive(Args)]
    pub struct ConfigureArgs {
        /// Open the file in `$EDITOR` instead of the form — for adding your own keys, or editing
        /// a setting the form can't show (it lists the true/false ones)
        #[arg(short = 'e', long)]
        editor: bool,
    }

    /// Tick the bashrs settings on or off in a form (~/.bashrs/configrs.toml), creating it on
    /// first use; `-e` opens the file in your editor instead
    pub fn configure(args: ConfigureArgs) {
        let path = match config_file::ensure_current() {
            Ok(path) => path,
            Err(err) => return eprintln!("brs_configure: cannot create the config file: {err}"),
        };
        if args.editor {
            return _open_in_editor(&path);
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => return eprintln!("brs_configure: cannot read {}: {err}", path.display()),
        };
        let settings = config_file::settings(&text);
        if settings.is_empty() {
            // Nothing tickable — a hand-emptied file, or one whose settings stopped being flags.
            // The editor is the honest fallback rather than an empty form.
            eprintln!("brs_configure: no true/false settings in {} — opening it instead", path.display());
            return _open_in_editor(&path);
        }
        let mut form = _settings_form(&settings, &path);
        match terminal_choice::run(&mut form) {
            Ok(terminal_choice::Outcome::Submitted) => _save(&path, &text, &settings, &form),
            Ok(terminal_choice::Outcome::Cancelled) => println!("brs_configure: unchanged"),
            Err(err) => {
                // Not a terminal (piped, or a dumb one): say so and fall back to the editor,
                // which is what the command did before the form existed.
                eprintln!("brs_configure: {err}");
                _open_in_editor(&path);
            }
        }
    }

    /// Pick what to change, by its number: the bashrs settings (`brs_configure`'s form), the
    /// reminders, marking them done or skipped (`remind_mark`), or the reminder files, picking one
    /// or making one (`remind_edit`) — ALT+W
    pub fn edit(_args: NoArgs) {
        let picks: [(&str, fn()); 3] = [
            ("the bashrs settings — brs_configure", || configure(ConfigureArgs { editor: false })),
            ("the reminders: mark them done or skipped — remind_mark", || {
                crate::categories::comfy_repos::remind_mark(terminalert::cli::MarkArgs::default())
            }),
            ("the reminder files: pick one or make one — remind_edit", || {
                crate::categories::comfy_repos::remind_edit(terminalert::cli::EditArgs::default())
            }),
        ];
        let mut menu = picks
            .iter()
            .fold(terminal_choice::Menu::new("What do you want to change?"), |menu, (label, _)| menu.pick(*label));
        match terminal_choice::run_menu(&mut menu) {
            Ok(terminal_choice::Chosen::Picked(at)) => (picks[at].1)(),
            Ok(terminal_choice::Chosen::Cancelled) => println!("brs_edit: unchanged"),
            Err(err) => eprintln!("brs_edit: {err}"),
        }
    }

    /// The settings as a form: each `[section]` announced once, each setting's own comment block
    /// above its box. The boxes are ANONYMOUS single-option groups rather than one group per
    /// section, because that's the only shape that lets a setting's explanation sit directly above
    /// it — options within a group are contiguous, and comments cannot be threaded between them.
    /// `aligned` then pads the comments to the option column so the two read as one block.
    fn _settings_form(settings: &[config_file::Setting], path: &Path) -> terminal_choice::Form {
        let mut form = terminal_choice::Form::new()
            .title(format!("bashrs configuration — {}", path.display()))
            .aligned();
        let mut shown = "";
        for setting in settings {
            let (section, key) = setting.split();
            if section != shown {
                form = form.comment(format!("[{section}]"));
                shown = section;
            }
            for line in &setting.docs {
                form = form.comment(line.clone());
            }
            // Each group holds one option, so a per-option sub-title would say what the
            // comments above it already do; and every setting here is the user's to change.
            let mut option = terminal_choice::Choice::named(key);
            option.checked = setting.enabled;
            form.items.push(terminal_choice::Item::Checkboxes {
                label: String::new(),
                options: vec![option],
            });
        }
        form
    }

    /// Write back what the form holds, leaving the file alone when nothing moved. The checkbox
    /// groups come back in the order they were built, so zipping them onto `settings` pairs each
    /// answer with the key it came from.
    fn _save(path: &Path, text: &str, settings: &[config_file::Setting], form: &terminal_choice::Form) {
        let ticked = form.items.iter().filter_map(|item| match item {
            terminal_choice::Item::Checkboxes { options, .. } => options.first().map(|o| o.checked),
            _ => None,
        });
        let values: Vec<(String, bool)> =
            settings.iter().map(|s| s.path.clone()).zip(ticked).collect();
        let updated = config_file::with_values(text, &values);
        if updated == text {
            return println!("brs_configure: unchanged");
        }
        match std::fs::write(path, &updated) {
            Ok(()) => {
                let changed = values.iter().filter(|(p, on)| {
                    settings.iter().any(|s| s.path == *p && s.enabled != *on)
                });
                for (path, on) in changed {
                    println!("{path} = {on}");
                }
                println!("brs_configure: saved to {}", path.display());
            }
            Err(err) => eprintln!("brs_configure: cannot write {}: {err}", path.display()),
        }
    }

    /// Hand the file to the user's editor — the pre-form behaviour, kept for `-e` and for the
    /// cases the form can't serve. `$EDITOR` when set (edits right here in the terminal); else the
    /// desktop opener — `xdg-open` is Linux's `open` (a bare `open` is usually openvt, a console
    /// switcher).
    ///
    /// `$EDITOR` goes through the shell, as git runs it, so it may carry the editor's flags
    /// (`code --wait`, `emacsclient -t`) — which only its writer knows; the file is handed over as
    /// `$1`, one argument whatever it holds.
    fn _open_in_editor(path: &Path) {
        match std::env::var("EDITOR") {
            Ok(editor) if !editor.trim().is_empty() => {
                let script = format!("{} \"$1\"", editor.trim());
                exec::run_reporting("sh", [OsStr::new("-c"), OsStr::new(&script), OsStr::new("sh"), path.as_os_str()]);
            }
            _ => {
                exec::run_reporting("xdg-open", [path]);
            }
        }
    }

    /// Draw how my own crates depend on each other, and which of them are shared
    pub fn map_comfy(_args: NoArgs) {
        for line in crate::drivers::comfy_graph::map() {
            println!("{line}");
        }
    }

    /// `brs_sourcefile`'s arguments: a flag per section of the file, in the file's order, and a
    /// TERM. Written out rather than derived, because the flags are named by the sections
    /// ([`sourcefile_sections::SECTIONS`]), not by fields of this struct.
    pub struct SourcefileArgs {
        /// The sections asked for, by label — none: the whole file.
        sections: Vec<&'static str>,
        /// Only the lines holding it, as `g` finds them.
        term: Option<String>,
    }

    /// The TERM's argument id.
    const TERM: &str = "term";

    impl clap::FromArgMatches for SourcefileArgs {
        fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
            let asked = |label: &&str| matches.get_flag(&sourcefile_sections::flag(label));
            Ok(Self {
                sections: sourcefile_sections::SECTIONS.iter().copied().filter(asked).collect(),
                term: matches.get_one::<String>(TERM).cloned(),
            })
        }

        fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
            *self = Self::from_arg_matches(matches)?;
            Ok(())
        }
    }

    impl Args for SourcefileArgs {
        fn augment_args(cmd: clap::Command) -> clap::Command {
            let cmd = sourcefile_sections::SECTIONS.iter().fold(cmd, |cmd, label| {
                let flag = sourcefile_sections::flag(label);
                cmd.arg(
                    clap::Arg::new(flag.clone())
                        .long(flag)
                        .action(clap::ArgAction::SetTrue)
                        .help(format!("Only the `{label}` section")),
                )
            });
            cmd.arg(clap::Arg::new(TERM).value_name("TERM").help(
                "Print only the lines holding TERM, as `brs_sourcefile | g TERM` would: matched and \
                 coloured as `g` does it (case-insensitive, literal)",
            ))
        }

        fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
            Self::augment_args(cmd)
        }
    }

    /// Print the autogenerated source file, noting where it lives
    ///
    /// The file of the shell it is run from: fish's in fish, bash's anywhere else. A section's
    /// flag prints that section alone — several, each in the file's order; a TERM, only the lines
    /// holding it, as `g` finds them — within the sections asked for, if any.
    pub fn sourcefile(args: SourcefileArgs) {
        let path = _sourcefile_path(Shell::caller());
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) => return eprintln!("brs_sourcefile: cannot read {}: {err}", path.display()),
        };
        let shown = if args.sections.is_empty() {
            contents
        } else {
            _sections_of(&contents, &args.sections, &path)
        };
        match &args.term {
            // `g`'s own search, as on piped input (no line numbers) — over the file's text rather
            // than its highlighted print, whose colour codes would split a term spanning two tokens.
            Some(_) => {
                let piped_g = streamgrep::Options { context: 0, line_number: false, regex: false, invert: false };
                streamgrep::filter(args.term.as_slice(), shown.as_bytes(), &piped_g);
            }
            None => {
                // Syntax-highlight the shell as we print it. Fish's file too: synoptic has no fish
                // grammar, and its shell one reads fish's quoting right — `\'` inside quotes.
                let shown = theme_code::highlight(&shown, "sh");
                print!("{shown}");
                if !shown.ends_with('\n') {
                    println!();
                }
                println!(); // blank line to set the location note off from the file body
                println!("# source file: {}", path.display());
            }
        }
    }

    /// The `wanted` sections of `contents`, a sourcefile, in its order — saying which of them it
    /// hasn't (a file from before one was added, or the stainless one, which only some files
    /// have).
    fn _sections_of(contents: &str, wanted: &[&str], path: &Path) -> String {
        let found = sourcefile_sections::find(contents);
        for label in wanted.iter().filter(|label| !found.iter().any(|(there, _)| there == *label)) {
            eprintln!("brs_sourcefile: {} has no `{label}` section", path.display());
        }
        found.into_iter().filter(|(label, _)| wanted.contains(label)).map(|(_, at)| &contents[at]).collect()
    }

    /// `cd` to the bashrs project's root directory (where it stood at the last compile)
    ///
    /// The path is compile-time truth, baked into the emitted function by the sourcefile
    /// generator (`{PROJECT_ROOT}` — the generator runs inside the binary, which is where
    /// compile-time knowledge lives, so no subprocess is needed at `cd` time). If the project
    /// has moved since, the `cd` fails naming the stale path, and a recompile re-teaches it.
    /// Pure shell because a child process can't move its parent.
    #[shell_body(both = "cd -- {PROJECT_ROOT}")]
    pub fn cd() {}

    /// `cd` to ~/.bashrs — the installation: binary, sourcefiles, config, tools, companion clones,
    /// your own files
    #[shell_body(both = "cd -- {BASHRS_HOME}")]
    pub fn conf_cd() {}

    /// Resolve `script` inside the project directory, or explain why it can't be found.
    fn _locate_script(project: &Path, script: &str) -> Result<PathBuf, String> {
        if !project.is_dir() {
            return Err(format!(
                "project directory not found: {} — moved or removed since the last compile",
                project.display()
            ));
        }
        let path = project.join(script);
        if !path.is_file() {
            return Err(format!("{script} not found: {} — renamed or removed", path.display()));
        }
        Ok(path)
    }

    /// Path to `shell`'s autogenerated source file.
    fn _sourcefile_path(shell: Shell) -> PathBuf {
        match shell {
            Shell::Bash => crate::conf::Place::Sourcefile.path(),
            Shell::Fish => crate::conf::Place::FishSourcefile.path(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// `brs_sourcefile --help` lists a flag per section in the file's order, each named as
        /// its section with `-` for `_` and spaces — and the flags asked for come back by label.
        #[test]
        fn the_section_flags_follow_the_file() {
            let mut cmd = SourcefileArgs::augment_args(clap::Command::new("brs_sourcefile"));
            let help = cmd.render_long_help().to_string();
            let listed: Vec<&str> = help
                .lines()
                .filter_map(|line| line.trim_start().strip_prefix("--"))
                .filter_map(|rest| rest.split_whitespace().next())
                .filter(|flag| *flag != "help")
                .collect();
            let expected: Vec<String> = sourcefile_sections::SECTIONS.iter().map(|label| sourcefile_sections::flag(label)).collect();
            assert_eq!(listed, expected);
            assert!(listed.contains(&"exposed-helpers") && listed.contains(&"stainless-comfy-repos"), "{listed:?}");

            let matches = cmd.try_get_matches_from(["brs_sourcefile", "--lookup", "--bashrs", "alias"]).unwrap();
            let args = <SourcefileArgs as clap::FromArgMatches>::from_arg_matches(&matches).unwrap();
            assert_eq!(args.sections, ["bashrs", "lookup"], "in the file's order, whatever the command line's");
            assert_eq!(args.term.as_deref(), Some("alias"));
        }

        #[test]
        fn reports_a_missing_project_directory() {
            let err =
                _locate_script(Path::new("/no/such/bashrs/project"), "COMPILE.sh").unwrap_err();
            assert!(err.contains("project directory not found"), "got: {err}");
        }

        #[test]
        fn reports_a_missing_script_by_its_name() {
            // `src/` exists but holds neither script — exercises the second check for both.
            let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
            let err = _locate_script(&dir, "COMPILE.sh").unwrap_err();
            assert!(err.contains("COMPILE.sh not found"), "got: {err}");
            let err = _locate_script(&dir, "TEST.sh").unwrap_err();
            assert!(err.contains("TEST.sh not found"), "got: {err}");
        }

        #[test]
        fn resolves_the_scripts_when_present() {
            let project = Path::new(env!("CARGO_MANIFEST_DIR"));
            assert_eq!(_locate_script(project, "COMPILE.sh").unwrap(), project.join("COMPILE.sh"));
            assert_eq!(_locate_script(project, "TEST.sh").unwrap(), project.join("TEST.sh"));
        }

        #[test]
        fn each_shell_has_its_own_sourcefile() {
            assert_eq!(_sourcefile_path(Shell::Bash), crate::conf::Place::Sourcefile.path());
            assert_eq!(_sourcefile_path(Shell::Fish), crate::conf::Place::FishSourcefile.path());
        }

        /// The form must present the file faithfully: every flag gets a box, pre-ticked to what
        /// the file currently says, under its section and behind its own explanation. A box whose
        /// initial state disagreed with the file would silently rewrite a setting the user never
        /// touched.
        #[test]
        fn the_form_mirrors_the_config_file_it_was_built_from() {
            use terminal_choice::Item;

            let text = "[tools]\n# why languages\nalways_bundle_languages = true\n\
                        # why utilities\n# second line\nalways_bundle_utilities = false\n";
            let settings = config_file::settings(text);
            let form = _settings_form(&settings, Path::new("/tmp/configrs.toml"));

            let boxes: Vec<(&str, bool)> = form
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Checkboxes { options, .. } => {
                        Some((options[0].name.as_str(), options[0].checked))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                boxes,
                [("always_bundle_languages", true), ("always_bundle_utilities", false)],
                "one box per flag, ticked as the file has it"
            );

            let comments: Vec<&str> = form
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Comment(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(comments[0], "[tools]", "the section is announced once, first");
            assert_eq!(comments.iter().filter(|line| **line == "[tools]").count(), 1);
            for expected in ["why languages", "why utilities", "second line"] {
                assert!(comments.contains(&expected), "{expected} is missing from {comments:?}");
            }
            assert!(form.aligned, "comments line up with the boxes they explain");
            assert!(
                form.title.as_deref().is_some_and(|t| t.contains("configrs.toml")),
                "the title names the file being edited"
            );
        }

        /// Ticking a box and submitting must move exactly that value, leaving the comments — the
        /// only documentation this file has — untouched.
        #[test]
        fn submitting_the_form_writes_back_only_what_moved() {
            use terminal_choice::Item;

            let text = "[tools]\n# keep me\nalways_bundle_languages = true\nalways_bundle_utilities = true\n";
            let settings = config_file::settings(text);
            let mut form = _settings_form(&settings, Path::new("/tmp/configrs.toml"));
            // Untick the first box, as a user would.
            for item in &mut form.items {
                if let Item::Checkboxes { options, .. } = item {
                    if options[0].name == "always_bundle_languages" {
                        options[0].checked = false;
                    }
                }
            }
            let ticked = form.items.iter().filter_map(|item| match item {
                Item::Checkboxes { options, .. } => options.first().map(|o| o.checked),
                _ => None,
            });
            let values: Vec<(String, bool)> =
                settings.iter().map(|s| s.path.clone()).zip(ticked).collect();
            let updated = config_file::with_values(text, &values);

            assert!(updated.contains("always_bundle_languages = false"), "{updated}");
            assert!(updated.contains("always_bundle_utilities = true"), "the other is left: {updated}");
            assert!(updated.contains("# keep me"), "comments survive the write: {updated}");
        }
    }
}
