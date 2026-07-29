//! The command-line interface: parse argv against the assembled command tree (`Cli` /
//! `Command`) and dispatch — including the binary's hidden self-commands (generate,
//! install-shell, install-stainless, complete-flags). The other half of the crate root's
//! job, assembling `sourcefile.sh` and the completion data those hidden commands serve,
//! lives in the [`sourcefile`] child.

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use crate::categories::autogen_lookup::{GgCommand, GrepCommand};
use crate::categories::autogen_styles::StylizedEchoCommand;
use crate::categories::bashrs::BashrsCommand;
use crate::categories::comfy_repos::ComfyReposCommand;
use crate::categories::download::DownloadCommand;
use crate::categories::filesystem::FilesystemCommand;
use crate::categories::git::GitCommand;
use crate::categories::lookup::LookupCommand;
use crate::categories::media::audio_fx::MediaAudioFxCommand;
use crate::categories::media::images::MediaImagesCommand;
use crate::categories::media::metadata::MediaMetadataCommand;
use crate::categories::media::transcode::MediaTranscodeCommand;
use crate::categories::packages::PackagesCommand;
use crate::categories::project::ProjectCommand;
use crate::categories::python::PythonCommand;
use crate::categories::styles::StyleCommand;

// The generator half of the crate root: everything `sourcefile.sh` and TAB-completion need,
// assembled from the categories' contributed shell surfaces.
mod sourcefile;

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
    Git(GitCommand),
    #[command(flatten)]
    Download(DownloadCommand),
    // One `media` group, one enum per product (see `categories::media`).
    #[command(flatten)]
    MediaTranscode(MediaTranscodeCommand),
    #[command(flatten)]
    MediaMetadata(MediaMetadataCommand),
    #[command(flatten)]
    MediaAudioFx(MediaAudioFxCommand),
    #[command(flatten)]
    MediaImages(MediaImagesCommand),
    #[command(flatten)]
    ComfyRepos(ComfyReposCommand),
    #[command(flatten)]
    Packages(PackagesCommand),
    #[command(flatten)]
    Project(ProjectCommand),
    #[command(flatten)]
    Python(PythonCommand),
    #[command(flatten)]
    Lookup(LookupCommand),
    #[command(flatten)]
    Grep(GrepCommand),
    #[command(flatten)]
    Treegrep(GgCommand),
    // Two enums, one logical `style` category: hand-written commands (`errcho`) and the
    // generated `recho` matrix. Both flatten to bare top-level commands.
    #[command(flatten)]
    Style(StyleCommand),
    #[command(flatten)]
    StylizedEcho(StylizedEchoCommand),
    /// Emit shell function wrappers for every command (used by COMPILE.sh).
    #[command(hide = true)]
    Generate,
    /// Install the running binary under ~/.bashrs, write the sourcefile, and wire the shell rc
    /// files (COMPILE.sh's last step).
    #[command(hide = true)]
    InstallShell,
    /// Provision the non-Rust companions — bundled tools, repos, the Carstay.toml record
    /// (COMPILE.sh's step before install-shell).
    #[command(hide = true)]
    InstallStainless {
        /// Provision the versions recorded in Carstay.toml instead of the latest releases
        #[arg(long)]
        use_stable_carstay: bool,
    },
    /// Print one command's completable flags (asked by the generated completer at TAB-time).
    #[command(hide = true)]
    CompleteFlags {
        /// The shell function name being completed (a command's clap name or visible alias).
        command: String,
    },
}

impl Command {
    pub fn run(self) {
        match self {
            Command::Bashrs(cmd) => cmd.run(),
            Command::Filesystem(cmd) => cmd.run(),
            Command::Git(cmd) => cmd.run(),
            Command::Download(cmd) => cmd.run(),
            Command::MediaTranscode(cmd) => cmd.run(),
            Command::MediaMetadata(cmd) => cmd.run(),
            Command::MediaAudioFx(cmd) => cmd.run(),
            Command::MediaImages(cmd) => cmd.run(),
            Command::ComfyRepos(cmd) => cmd.run(),
            Command::Packages(cmd) => cmd.run(),
            Command::Project(cmd) => cmd.run(),
            Command::Python(cmd) => cmd.run(),
            Command::Lookup(cmd) => cmd.run(),
            Command::Grep(cmd) => cmd.run(),
            Command::Treegrep(cmd) => cmd.run(),
            Command::Style(cmd) => cmd.run(),
            Command::StylizedEcho(cmd) => cmd.run(),
            Command::Generate => print!("{}", sourcefile::wrappers()),
            Command::InstallShell => crate::conf::install::install_shell(&sourcefile::wrappers()),
            Command::InstallStainless { use_stable_carstay } => {
                crate::drivers::install_stainless(use_stable_carstay)
            }
            Command::CompleteFlags { command } => println!("{}", sourcefile::complete_flags(&command)),
        }
    }
}

/// Flags a command forces on, hidden from its help and completion — still *accepted*, since
/// passing one is a harmless no-op (it's already on). Keyed by clap name and arg ID (the field
/// name). The `g<N>`/`gg<N>` variants aren't listed: they REMOVE their pinned `-C` outright by
/// taking the `*Base` argument structs (see [`crate::support::args`]).
const HIDDEN_PINNED: &[(&str, &[&str])] = &[
    ("GG", &["delve"]),
    ("GGG", &["delve", "save", "regex"]),
    ("backup_find_bitrot", &["eager_checksum"]),
];

/// Hide each [`HIDDEN_PINNED`] flag on its command. Applied everywhere the command tree is built —
/// parsing ([`parse`]) and generation (`sourcefile::category_commands`) — so help, wrappers, and completion
/// all tell one truth. Commands absent from `cmd` are skipped (each category holds only its own).
fn hide_pinned(mut cmd: clap::Command) -> clap::Command {
    for &(name, args) in HIDDEN_PINNED {
        if cmd.find_subcommand(name).is_none() {
            continue; // don't let `mut_subcommand` conjure the command into the wrong category
        }
        cmd = cmd.mut_subcommand(name, |sub| {
            args.iter().fold(sub, |sub, arg| sub.mut_arg(*arg, |a| a.hide(true)))
        });
    }
    cmd
}

/// Parse argv against the adjusted command tree ([`hide_pinned`]) — the binary's normal entry,
/// called by [`crate::run`] once the re-exec handlers have passed on the invocation.
pub fn parse() -> Cli {
    let mut matches = hide_pinned(Cli::command()).get_matches();
    Cli::from_arg_matches_mut(&mut matches).unwrap_or_else(|err| err.exit())
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
        hide_pinned(Cli::command()).debug_assert(); // the adjusted tree the binary actually parses
    }

    #[test]
    fn hidden_pinned_flags_are_real_and_hidden() {
        // `mut_arg` silently CREATES a missing arg, so a drifted field name would ghost a fresh
        // hidden arg instead of hiding the real flag — the long/short assertion catches that.
        let cmd = hide_pinned(Cli::command());
        for &(name, args) in HIDDEN_PINNED {
            let sub = cmd.find_subcommand(name).unwrap_or_else(|| panic!("`{name}` not found"));
            for id in args {
                let arg = sub
                    .get_arguments()
                    .find(|a| a.get_id().as_str() == *id)
                    .unwrap_or_else(|| panic!("`{name}` has no arg `{id}`"));
                assert!(arg.is_hide_set(), "`{name}`'s `{id}` should be hidden");
                assert!(
                    arg.get_long().is_some() || arg.get_short().is_some(),
                    "`{name}`'s `{id}` looks like a ghost created by `mut_arg` — the arg ID drifted"
                );
            }
        }
    }

    #[test]
    fn filesystem_commands_follow_naming_standard() {
        // `lll` is a bare `ls`-like verb (à la `ll`), intentionally unprefixed — the same
        // kind of exception the style echoes and `UPUP` take from the `<category>_` norm.
        assert_prefixed(&command_names::<FilesystemCommand>(), "fs_", &["lll"]);
    }

    #[test]
    fn git_commands_follow_naming_standard() {
        assert_prefixed(&command_names::<GitCommand>(), "git_", &[]);
    }

    #[test]
    fn media_commands_follow_naming_standard() {
        for names in [
            command_names::<MediaTranscodeCommand>(),
            command_names::<MediaMetadataCommand>(),
            command_names::<MediaAudioFxCommand>(),
            command_names::<MediaImagesCommand>(),
        ] {
            assert_prefixed(&names, "media_", &[]);
        }
    }

    #[test]
    fn packages_commands_follow_naming_standard() {
        // `upup` is prefixed (with a bare `upup` alias); `UPUP` is a deliberate
        // custom name (the loud "update everything"). Everything else is prefixed.
        assert_prefixed(&command_names::<PackagesCommand>(), "packages_", &["UPUP"]);
    }

    #[test]
    fn project_commands_follow_naming_standard() {
        assert_prefixed(&command_names::<ProjectCommand>(), "pro_", &[]);
    }

    #[test]
    fn comfy_commands_follow_naming_standard() {
        // External tools keep their own upstream name (`table`, plus its pinned-preset sibling
        // `table_fancy`) or a task-named family — all unprefixed by design (`backup_*` flattens
        // filesync's subcommands into directly-completable commands).
        assert_prefixed(
            &command_names::<ComfyReposCommand>(),
            "comfy_",
            &["table", "table_fancy", "backup_diff", "backup_sync", "backup_find_bitrot"],
        );
    }

    #[test]
    fn python_commands_follow_naming_standard() {
        // `py` is the bare inline evaluator, à la the classic bashrc alias.
        assert_prefixed(&command_names::<PythonCommand>(), "py_", &[]);
    }

    #[test]
    fn lookup_commands_follow_naming_standard() {
        // `hg` mirrors the classic `history | grep` alias; `GG` is the loud all-caps sibling of `gg`
        // (recursive search with `--delve` forced), and `GGG` is `GG` with `--save`/`-E` too — all
        // bare, memorable exceptions à la `UPUP`.
        assert_prefixed(&command_names::<LookupCommand>(), "lookup_", &["hg", "GG", "GGG"]);
    }

    #[test]
    fn grep_family_are_bare_verbs() {
        // The generated `g`/`g3`/… shortcuts are intentionally bare, like the style echoes.
        for name in command_names::<GrepCommand>() {
            assert!(!name.starts_with("lookup_"), "grep command `{name}` should be bare, not prefixed");
        }
    }

    #[test]
    fn gg_family_are_bare_verbs() {
        // The generated `gg`/`gg2`/… recursive-search shortcuts are intentionally bare too.
        for name in command_names::<GgCommand>() {
            assert!(!name.starts_with("lookup_"), "gg command `{name}` should be bare, not prefixed");
        }
    }

    #[test]
    fn style_commands_are_bare_verbs() {
        // Style commands — both the hand-written `StyleCommand` and the generated
        // `StylizedEchoCommand` — are intentionally unprefixed: short, memorable echo
        // verbs, never `style_`-prefixed. The inverse of the usual standard, by design.
        let names =
            command_names::<StyleCommand>().into_iter().chain(command_names::<StylizedEchoCommand>());
        for name in names {
            assert!(!name.starts_with("style_"), "style command `{name}` should be bare, not prefixed");
        }
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
