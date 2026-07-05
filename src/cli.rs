//! The command-line interface and `sourcefile.sh` generator — the orchestration
//! that ties [`crate::categories`] together with [`crate::shell_conf`] and
//! [`crate::support`]. It parses and dispatches commands (`Cli` / `Command`) and
//! builds the sourced shell file (`wrappers`).

use clap::{Parser, Subcommand};
use crate::categories::bashrs::BashrsCommand;
use crate::categories::comfy_repos::ComfyReposCommand;
use crate::categories::filesystem::FilesystemCommand;
use crate::categories::media::MediaCommand;
use crate::categories::packages::PackagesCommand;
use crate::categories::style::StyleCommand;
use crate::shell_conf::{keybinds, session};

/// Exit code a command returns to ask its generated wrapper to run its
/// `#[after]` action (e.g. start a fresh shell). It's distinct from success (0)
/// and failure (non-zero), so clap's `--help`/`-h` — which exit 0 — never
/// trigger it. Kept trivial in shell (`[ "$?" -eq N ]`) to parse on any bash.
pub const RELOAD_EXIT_CODE: i32 = 97;

#[derive(Parser)]
#[command(name = "bashrs", about = "Rust-based bashrc")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level command set. Each category is flattened so its commands appear
/// directly (e.g. `bashrs lss`), matching the shell functions we generate.
#[derive(Subcommand)]
pub enum Command {
    #[command(flatten)]
    Bashrs(BashrsCommand),
    #[command(flatten)]
    Filesystem(FilesystemCommand),
    #[command(flatten)]
    Media(MediaCommand),
    #[command(flatten)]
    ComfyRepos(ComfyReposCommand),
    #[command(flatten)]
    Packages(PackagesCommand),
    #[command(flatten)]
    Style(StyleCommand),
    /// Emit shell function wrappers for every command (used by COMPILE.sh).
    #[command(hide = true)]
    Generate,
}

impl Command {
    pub fn run(self) {
        match self {
            Command::Bashrs(cmd) => cmd.run(),
            Command::Filesystem(cmd) => cmd.run(),
            Command::Media(cmd) => cmd.run(),
            Command::ComfyRepos(cmd) => cmd.run(),
            Command::Packages(cmd) => cmd.run(),
            Command::Style(cmd) => cmd.run(),
            Command::Generate => print!("{}", wrappers()),
        }
    }
}

/// The command categories, each paired with the label used to group them in the
/// generated `sourcefile.sh`. One row per category — never per command.
fn category_commands() -> [(&'static str, clap::Command); 6] {
    [
        ("bashrs", BashrsCommand::augment_subcommands(clap::Command::new("bashrs"))),
        ("filesystem", FilesystemCommand::augment_subcommands(clap::Command::new("filesystem"))),
        ("media", MediaCommand::augment_subcommands(clap::Command::new("media"))),
        ("packages", PackagesCommand::augment_subcommands(clap::Command::new("packages"))),
        ("style", StyleCommand::augment_subcommands(clap::Command::new("style"))),
        ("comfy_repos", ComfyReposCommand::augment_subcommands(clap::Command::new("comfy_repos"))),
    ]
}

/// The shell appended (after `&&`) to a command's wrapper — e.g. to restart the
/// shell after a command that changes the environment. Command names are unique
/// across categories (clap flattening requires it), so the first match wins.
fn wrapper_suffix(name: &str) -> Option<&'static str> {
    BashrsCommand::wrapper_suffix(name)
        .or_else(|| FilesystemCommand::wrapper_suffix(name))
        .or_else(|| MediaCommand::wrapper_suffix(name))
        .or_else(|| PackagesCommand::wrapper_suffix(name))
        .or_else(|| StyleCommand::wrapper_suffix(name))
        .or_else(|| ComfyReposCommand::wrapper_suffix(name))
}

/// Build the shell function definitions sourced from `~/.bashrs/sourcefile.sh`.
///
/// Definitions are grouped by category, each under a comment header. Every
/// wrapper — including unprefixed aliases — dispatches to the command's *real* name,
/// so an alias is purely a shell-side convenience and never has to resolve as a
/// clap alias. The binary path is inlined into each function rather than held in
/// a shared variable, so sourcing leaves nothing behind in the user's shell.
fn wrappers() -> String {
    // Quoted so `$HOME` expands at call time and paths with spaces stay intact.
    const BIN: &str = "\"$HOME/.bashrs/bashrs\"";

    let mut body = String::new();
    for (label, category) in category_commands() {
        let mut lines = String::new();
        for sub in category.get_subcommands() {
            if sub.is_hide_set() {
                continue; // skip any command marked internal
            }
            let real = sub.get_name();
            // The command's one-line description (its clap `about`), appended as
            // an inline comment. First line only, so a comment can't run over.
            let about = sub
                .get_about()
                .map(|a| a.to_string())
                .and_then(|a| a.lines().next().map(str::to_string))
                .filter(|a| !a.is_empty())
                .map(|a| format!("  # {a}"))
                .unwrap_or_default();
            // Run the suffix (e.g. `session_new`) only when the command signals a
            // reload by exiting RELOAD_EXIT_CODE. A real success does; clap's
            // `--help` (exit 0) and any failure (non-zero) do not.
            let suffix = wrapper_suffix(real)
                .map(|s| format!("; [ \"$?\" -eq {RELOAD_EXIT_CODE} ] && {s}"))
                .unwrap_or_default();
            for shell_name in std::iter::once(real).chain(sub.get_visible_aliases()) {
                lines += &format!("{shell_name}() {{ {BIN} {real} \"$@\"{suffix}; }}{about}\n");
            }
        }
        if !lines.is_empty() {
            body += &format!("\n# {label}\n{lines}");
        }
    }

    // Session functions + keybinds: raw shell run when sourcefile.sh is sourced.
    body += &format!("\n# session\n{}", session::functions());
    let binds: String = keybinds::bindings()
        .iter()
        .map(|(key, func)| format!("    bind '\"{key}\": \"{func}\\n\"'\n"))
        .collect();
    if !binds.is_empty() {
        // `bind` is a bash readline builtin; zsh (which also sources this) has none.
        body += &format!("\n# keybinds (bash only)\nif [ -n \"$BASH_VERSION\" ]; then\n{binds}fi\n");
    }

    let mut out = String::from(
        "#!/usr/bin/env bash\n\
         # Auto-generated by `bashrs generate` — do not edit.\n\
         # Regenerate by re-running COMPILE.sh.\n",
    );
    if !body.is_empty() {
        // Bail early (before defining anything) if the binary isn't present.
        out += &format!("\n[ -f {BIN} ] || return\n");
        out += &body;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Command as ClapCommand, CommandFactory};

    fn command_names<E: Subcommand>() -> Vec<String> {
        E::augment_subcommands(ClapCommand::new("test"))
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect()
    }

    /// Every command in a category must carry its category prefix, unless it is
    /// explicitly listed as an unprefixed exception. This keeps the naming
    /// standard from silently eroding as commands are added.
    fn assert_prefixed(names: &[String], prefix: &str, unprefixed: &[&str]) {
        for name in names {
            assert!(
                name.starts_with(prefix) || unprefixed.contains(&name.as_str()),
                "command `{name}` must start with `{prefix}` or be a declared unprefixed exception in {unprefixed:?}",
            );
        }
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn filesystem_commands_follow_naming_standard() {
        assert_prefixed(&command_names::<FilesystemCommand>(), "fs_", &[]);
    }

    #[test]
    fn media_commands_follow_naming_standard() {
        assert_prefixed(&command_names::<MediaCommand>(), "media_", &[]);
    }

    #[test]
    fn packages_commands_follow_naming_standard() {
        // `upup` is prefixed (with a bare `upup` alias); `UPUP` is a deliberate
        // custom name (the loud "update everything"). Everything else is prefixed.
        assert_prefixed(&command_names::<PackagesCommand>(), "packages_", &["UPUP"]);
    }

    #[test]
    fn style_commands_are_bare_verbs() {
        // Style commands are intentionally unprefixed (short, memorable echo verbs),
        // never `style_`-prefixed — the inverse of the usual standard, by design.
        for name in command_names::<StyleCommand>() {
            assert!(!name.starts_with("style_"), "style command `{name}` should be bare, not prefixed");
        }
    }

    #[test]
    fn wrappers_cover_every_command_and_alias() {
        let script = wrappers();
        let has = |line: &str| assert!(script.contains(line), "missing wrapper line: {line}");
        has("media_conv() { \"$HOME/.bashrs/bashrs\" media_conv \"$@\"; }");
        has("media_metadata() { \"$HOME/.bashrs/bashrs\" media_metadata \"$@\"; }");
        has("media_hmerge_imgs() { \"$HOME/.bashrs/bashrs\" media_hmerge_imgs \"$@\"; }");
        has("packages_upup() { \"$HOME/.bashrs/bashrs\" packages_upup \"$@\"; }"); // packages (both)
        has("upup() { \"$HOME/.bashrs/bashrs\" packages_upup \"$@\"; }"); // unprefixed alias -> packages_upup
        has("packages_print() { \"$HOME/.bashrs/bashrs\" packages_print \"$@\"; }"); // prefixed only
        has("packages_update_toolchains() { \"$HOME/.bashrs/bashrs\" packages_update_toolchains \"$@\"; }"); // prefixed only
        has("UPUP() { \"$HOME/.bashrs/bashrs\" UPUP \"$@\"; }"); // custom-named: update everything
        // bashrs_compile starts a fresh session only when compile signals a reload (exit code)
        has(&format!(
            "bashrs_compile() {{ \"$HOME/.bashrs/bashrs\" bashrs_compile \"$@\"; [ \"$?\" -eq {RELOAD_EXIT_CODE} ] && session_new; }}"
        ));
        has("bashrs_sourcefile() { \"$HOME/.bashrs/bashrs\" bashrs_sourcefile \"$@\"; }");
        has("recho() { \"$HOME/.bashrs/bashrs\" recho \"$@\"; }"); // style: bare, unprefixed
    }

    #[test]
    fn wrappers_include_session_function_and_bash_guarded_keybind() {
        let script = wrappers();
        assert!(script.contains("session_new() { exec bash; }"), "session_new missing");
        assert!(script.contains(r#"bind '"\en": "session_new\n"'"#), "ALT+N keybind missing");
        assert!(script.contains("if [ -n \"$BASH_VERSION\" ]; then"), "keybinds should be bash-guarded");
    }

    #[test]
    fn wrappers_group_commands_under_a_category_comment() {
        let script = wrappers();
        // A blank line then a comment introduces each category's block.
        assert!(script.contains("\n# bashrs\n"));
        assert!(script.contains("\n# media\n"));
    }

    #[test]
    fn wrappers_exclude_the_internal_generate_command() {
        assert!(!wrappers().contains("generate() {"));
    }

    #[test]
    fn wrappers_leave_no_variable_behind_in_the_shell() {
        // The path is inlined per function, so sourcing defines functions only —
        // nothing lingers in the user's environment afterward.
        assert!(!wrappers().contains("__bashrs_bin"));
    }

    #[test]
    fn wrappers_bail_early_when_the_binary_is_missing() {
        assert!(wrappers().contains("[ -f \"$HOME/.bashrs/bashrs\" ] || return"));
    }

    #[test]
    fn file_opens_with_a_bash_shebang() {
        assert!(wrappers().starts_with("#!/usr/bin/env bash\n"));
    }

    #[test]
    fn wrappers_carry_each_command_description_as_an_inline_comment() {
        // The command's clap `about` trails its wrapper as a `# ...` comment.
        assert!(wrappers().contains(
            "media_metadata() { \"$HOME/.bashrs/bashrs\" media_metadata \"$@\"; }  # Get metadata of an audio/video/image file"
        ));
    }

    /// Guards the small per-category list in `category_commands` against drift:
    /// every command the CLI can dispatch must be grouped into exactly one
    /// category (and vice versa), so none can silently miss a wrapper.
    #[test]
    fn category_grouping_covers_exactly_the_cli_commands() {
        use std::collections::BTreeSet;
        let dispatchable: BTreeSet<String> = Cli::command()
            .get_subcommands()
            .filter(|c| !c.is_hide_set() && c.get_name() != "help")
            .map(|c| c.get_name().to_string())
            .collect();
        let grouped: BTreeSet<String> = category_commands()
            .iter()
            .flat_map(|(_, cmd)| {
                cmd.get_subcommands().map(|c| c.get_name().to_string()).collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(dispatchable, grouped, "every CLI command must belong to exactly one category");
    }
}

/// Exercises the naming modes of the `#[category]` macro on a throwaway category,
/// so the `#[prefixed]` / `#[unprefixed]` / `#[name]` logic is covered directly.
#[cfg(test)]
#[allow(dead_code)] // the generated `run`/handlers aren't invoked, only introspected
mod macro_naming_modes {
    use clap::{Command as ClapCommand, Subcommand};

    #[bashrs_macros::category(command = ProbeCommand, prefix = "p_")]
    mod probe {
        use clap::Args;

        /// prefixed by default
        pub fn alpha(_args: Alpha) {}
        #[derive(Args)]
        pub struct Alpha {}

        /// unprefixed only
        #[unprefixed]
        pub fn beta(_args: Beta) {}
        #[derive(Args)]
        pub struct Beta {}

        /// both forms
        #[prefixed]
        #[unprefixed]
        pub fn gamma(_args: Gamma) {}
        #[derive(Args)]
        pub struct Gamma {}

        /// explicit custom name
        #[name("custom-delta")]
        pub fn delta(_args: Delta) {}
        #[derive(Args)]
        pub struct Delta {}
    }

    #[test]
    fn each_mode_yields_the_expected_name_and_aliases() {
        let built = ProbeCommand::augment_subcommands(ClapCommand::new("probe"));
        let aliases = |name: &str| {
            built
                .get_subcommands()
                .find(|c| c.get_name() == name)
                .map(|c| c.get_visible_aliases().map(str::to_string).collect::<Vec<_>>())
        };
        assert_eq!(aliases("p_alpha"), Some(vec![]), "default: prefixed only");
        assert_eq!(aliases("beta"), Some(vec![]), "#[unprefixed]: bare name, no prefix");
        assert_eq!(aliases("p_gamma"), Some(vec!["gamma".to_string()]), "both: prefixed + bare alias");
        assert_eq!(aliases("custom-delta"), Some(vec![]), "#[name]: exact name, no prefix or alias");
        assert_eq!(aliases("p_delta"), None, "#[name] overrides the prefix");
    }
}
