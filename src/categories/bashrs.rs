//! Commands for managing bashrs itself.

#[bashrs_macros::category(command = BashrsCommand, prefix = "bashrs_")]
mod commands {
    use crate::conf::config_file;
    use crate::support::args::NoArgs;
    use crate::support::{exec, theme_code};
    use clap::Args;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// Everything after the command, forwarded verbatim to COMPILE.sh — the script owns its
    /// flag set (and rejects unknowns loudly), so nothing needs re-declaring here when it
    /// grows one. The cost: clap can't list or TAB-complete the flags; COMPILE.sh's header
    /// documents them (currently --use-stable-cargo, --use-stable-carstay).
    #[derive(Args)]
    pub struct CompileArgs {
        /// Flags for COMPILE.sh (e.g. --use-stable-cargo, --use-stable-carstay — see its header)
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
                eprintln!("bashrs_compile: {msg}");
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
                eprintln!("bashrs_compile: COMPILE.sh exited with status: {status}");
                std::process::exit(1);
            }
            Err(err) => {
                eprintln!("bashrs_compile: could not launch COMPILE.sh: {err}");
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
                eprintln!("bashrs_test: {msg}");
                std::process::exit(1);
            }
        };
        match Command::new("bash").arg(&script).current_dir(project).status() {
            Ok(status) if status.success() => {}
            // TEST.sh's own summary already named what failed — just propagate the verdict.
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(err) => {
                eprintln!("bashrs_test: could not launch TEST.sh: {err}");
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
            Err(err) => return eprintln!("bashrs_configure: cannot create the config file: {err}"),
        };
        if args.editor {
            return _open_in_editor(&path);
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => return eprintln!("bashrs_configure: cannot read {}: {err}", path.display()),
        };
        let settings = config_file::settings(&text);
        if settings.is_empty() {
            // Nothing tickable — a hand-emptied file, or one whose settings stopped being flags.
            // The editor is the honest fallback rather than an empty form.
            eprintln!("bashrs_configure: no true/false settings in {} — opening it instead", path.display());
            return _open_in_editor(&path);
        }
        let mut form = _settings_form(&settings, &path);
        match terminal_choice::run(&mut form) {
            Ok(terminal_choice::Outcome::Submitted) => _save(&path, &text, &settings, &form),
            Ok(terminal_choice::Outcome::Cancelled) => println!("bashrs_configure: unchanged"),
            Err(err) => {
                // Not a terminal (piped, or a dumb one): say so and fall back to the editor,
                // which is what the command did before the form existed.
                eprintln!("bashrs_configure: {err}");
                _open_in_editor(&path);
            }
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
            form.items.push(terminal_choice::Item::Checkboxes {
                label: String::new(),
                options: vec![key.to_string()],
                checked: vec![setting.enabled],
            });
        }
        form
    }

    /// Write back what the form holds, leaving the file alone when nothing moved. The checkbox
    /// groups come back in the order they were built, so zipping them onto `settings` pairs each
    /// answer with the key it came from.
    fn _save(path: &Path, text: &str, settings: &[config_file::Setting], form: &terminal_choice::Form) {
        let ticked = form.items.iter().filter_map(|item| match item {
            terminal_choice::Item::Checkboxes { checked, .. } => checked.first().copied(),
            _ => None,
        });
        let values: Vec<(String, bool)> =
            settings.iter().map(|s| s.path.clone()).zip(ticked).collect();
        let updated = config_file::with_values(text, &values);
        if updated == text {
            return println!("bashrs_configure: unchanged");
        }
        match std::fs::write(path, &updated) {
            Ok(()) => {
                let changed = values.iter().filter(|(p, on)| {
                    settings.iter().any(|s| s.path == *p && s.enabled != *on)
                });
                for (path, on) in changed {
                    println!("{path} = {on}");
                }
                println!("bashrs_configure: saved to {}", path.display());
            }
            Err(err) => eprintln!("bashrs_configure: cannot write {}: {err}", path.display()),
        }
    }

    /// Hand the file to the user's editor — the pre-form behaviour, kept for `-e` and for the
    /// cases the form can't serve. `$EDITOR` when set (edits right here in the terminal); else the
    /// desktop opener — `xdg-open` is Linux's `open` (a bare `open` is usually openvt, a console
    /// switcher).
    fn _open_in_editor(path: &Path) {
        match std::env::var("EDITOR") {
            Ok(editor) if !editor.trim().is_empty() => {
                exec::run_reporting(editor.trim(), [path]);
            }
            _ => {
                exec::run_reporting("xdg-open", [path]);
            }
        }
    }

    /// Print the autogenerated source file, noting where it lives
    pub fn sourcefile(_args: NoArgs) {
        let path = _sourcefile_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => {
                // Syntax-highlight the shell as we print it.
                let shown = theme_code::highlight(&contents, "sh");
                print!("{shown}");
                if !shown.ends_with('\n') {
                    println!();
                }
                println!(); // blank line to set the location note off from the file body
                println!("# source file: {}", path.display());
            }
            Err(err) => eprintln!("bashrs_sourcefile: cannot read {}: {err}", path.display()),
        }
    }

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

    /// Path to the autogenerated source file (`~/.bashrs/sourcefile.sh`).
    fn _sourcefile_path() -> PathBuf {
        crate::conf::sourcefile()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

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
        fn sourcefile_lives_under_bashrs_home() {
            assert!(_sourcefile_path().ends_with(".bashrs/sourcefile.sh"));
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
                    Item::Checkboxes { options, checked, .. } => {
                        Some((options[0].as_str(), checked[0]))
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
                if let Item::Checkboxes { options, checked, .. } = item {
                    if options[0] == "always_bundle_languages" {
                        checked[0] = false;
                    }
                }
            }
            let ticked = form.items.iter().filter_map(|item| match item {
                Item::Checkboxes { checked, .. } => checked.first().copied(),
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
