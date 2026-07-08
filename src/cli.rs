//! The command-line interface and `sourcefile.sh` generator — the orchestration
//! that ties [`crate::categories`] together with [`crate::shell_conf`] and
//! [`crate::support`]. It parses and dispatches commands (`Cli` / `Command`) and
//! builds the sourced shell file (`wrappers`).

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use crate::categories::autogen_lookup::{GgCommand, GrepCommand};
use crate::categories::autogen_styles::StylizedEchoCommand;
use crate::categories::bashrs::BashrsCommand;
use crate::categories::comfy_repos::ComfyReposCommand;
use crate::categories::filesystem::FilesystemCommand;
use crate::categories::git::GitCommand;
use crate::categories::lookup::LookupCommand;
use crate::categories::media::MediaCommand;
use crate::categories::packages::PackagesCommand;
use crate::categories::project::ProjectCommand;
use crate::categories::styles::StyleCommand;
use crate::shell_conf::{environment, greeting, keybinds, session, stainless};

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
    Git(GitCommand),
    #[command(flatten)]
    Media(MediaCommand),
    #[command(flatten)]
    ComfyRepos(ComfyReposCommand),
    #[command(flatten)]
    Packages(PackagesCommand),
    #[command(flatten)]
    Project(ProjectCommand),
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
            Command::Media(cmd) => cmd.run(),
            Command::ComfyRepos(cmd) => cmd.run(),
            Command::Packages(cmd) => cmd.run(),
            Command::Project(cmd) => cmd.run(),
            Command::Lookup(cmd) => cmd.run(),
            Command::Grep(cmd) => cmd.run(),
            Command::Treegrep(cmd) => cmd.run(),
            Command::Style(cmd) => cmd.run(),
            Command::StylizedEcho(cmd) => cmd.run(),
            Command::Generate => print!("{}", wrappers()),
            Command::CompleteFlags { command } => println!("{}", complete_flags(&command)),
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
];

/// Hide each [`HIDDEN_PINNED`] flag on its command. Applied everywhere the command tree is built —
/// parsing ([`parse`]) and generation ([`category_commands`]) — so help, wrappers, and completion
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

/// The command categories, each paired with the label used to group them in the
/// generated `sourcefile.sh`. One row per category — never per command.
fn category_commands() -> [(&'static str, clap::Command); 9] {
    let categories = [
        ("bashrs", BashrsCommand::augment_subcommands(clap::Command::new("bashrs"))),
        ("filesystem", FilesystemCommand::augment_subcommands(clap::Command::new("filesystem"))),
        ("git", GitCommand::augment_subcommands(clap::Command::new("git"))),
        ("media", MediaCommand::augment_subcommands(clap::Command::new("media"))),
        ("packages", PackagesCommand::augment_subcommands(clap::Command::new("packages"))),
        ("project", ProjectCommand::augment_subcommands(clap::Command::new("project"))),
        // One `lookup` group: `hg` (history search) plus the generated g-family.
        ("lookup", GgCommand::augment_subcommands(GrepCommand::augment_subcommands(
            LookupCommand::augment_subcommands(clap::Command::new("lookup")),
        ))),
        // One `style` group spanning both style enums: hand-written + generated.
        ("style", StylizedEchoCommand::augment_subcommands(
            StyleCommand::augment_subcommands(clap::Command::new("style")),
        )),
        ("comfy_repos", ComfyReposCommand::augment_subcommands(clap::Command::new("comfy_repos"))),
    ];
    categories.map(|(label, cmd)| (label, hide_pinned(cmd)))
}

/// Space-separated `-short --long` names of `command`'s arguments (matched by clap name or visible
/// alias, so `upup` completes like `packages_upup`). Positionals and hidden flags (the pinned ones
/// — [`HIDDEN_PINNED`]) are omitted; an unknown name yields nothing, so the completer simply offers
/// no flags. Backs the hidden `complete-flags` command the generated completer calls at TAB-time.
fn complete_flags(name: &str) -> String {
    for (_, category) in category_commands() {
        for sub in category.get_subcommands() {
            if sub.get_name() != name && !sub.get_visible_aliases().any(|alias| alias == name) {
                continue;
            }
            let mut flags: Vec<String> = Vec::new();
            for arg in sub.get_arguments() {
                if arg.is_positional() || arg.is_hide_set() {
                    continue;
                }
                if let Some(short) = arg.get_short() {
                    flags.push(format!("-{short}"));
                }
                if let Some(long) = arg.get_long() {
                    flags.push(format!("--{long}"));
                }
            }
            // The auto help flag only materializes when clap *builds* a command, which these
            // introspection copies aren't — add it by hand (no command disables it).
            if !flags.iter().any(|flag| flag == "--help") {
                flags.push("-h".into());
                flags.push("--help".into());
            }
            return flags.join(" ");
        }
    }
    String::new()
}

/// Every category's `(wrapper_suffix, wrapper_prefix)` lookups — one row per category, so adding
/// a category is one line here instead of an edit to each lookup. Command names are unique across
/// categories (clap flattening requires it), so the first match wins.
type WrapperLookup = fn(&str) -> Option<&'static str>;
const WRAPPER_HOOKS: &[(WrapperLookup, WrapperLookup)] = &[
    (BashrsCommand::wrapper_suffix, BashrsCommand::wrapper_prefix),
    (FilesystemCommand::wrapper_suffix, FilesystemCommand::wrapper_prefix),
    (GitCommand::wrapper_suffix, GitCommand::wrapper_prefix),
    (MediaCommand::wrapper_suffix, MediaCommand::wrapper_prefix),
    (PackagesCommand::wrapper_suffix, PackagesCommand::wrapper_prefix),
    (ProjectCommand::wrapper_suffix, ProjectCommand::wrapper_prefix),
    (LookupCommand::wrapper_suffix, LookupCommand::wrapper_prefix),
    (GrepCommand::wrapper_suffix, GrepCommand::wrapper_prefix),
    (GgCommand::wrapper_suffix, GgCommand::wrapper_prefix),
    (StyleCommand::wrapper_suffix, StyleCommand::wrapper_prefix),
    (StylizedEchoCommand::wrapper_suffix, StylizedEchoCommand::wrapper_prefix),
    (ComfyReposCommand::wrapper_suffix, ComfyReposCommand::wrapper_prefix),
];

/// The shell appended (after `&&`) to a command's wrapper — e.g. to restart the
/// shell after a command that changes the environment.
fn wrapper_suffix(name: &str) -> Option<&'static str> {
    WRAPPER_HOOKS.iter().find_map(|(suffix, _)| suffix(name))
}

/// The shell piped into a command's wrapper (ahead of the binary) — e.g. `hg` searches the
/// shell history, which only the shell itself can produce, so its wrapper runs `history` and
/// pipes it in.
fn wrapper_prefix(name: &str) -> Option<&'static str> {
    WRAPPER_HOOKS.iter().find_map(|(_, prefix)| prefix(name))
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
    let mut completion_names: Vec<String> = Vec::new(); // every wrapper + alias, for `complete -F`
    for (label, category) in category_commands() {
        let mut lines: Vec<String> = Vec::new();
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
            // A command that consumes a builtin's output (e.g. `hg` ← `history`) has it piped
            // in ahead of the binary; the shell must produce it, since we can't.
            let prefix = wrapper_prefix(real).map(|cmd| format!("{cmd} | ")).unwrap_or_default();
            for shell_name in std::iter::once(real).chain(sub.get_visible_aliases()) {
                lines.push(format!("{shell_name}() {{ {prefix}{BIN} {real} \"$@\"{suffix}; }}{about}"));
                completion_names.push(shell_name.to_string());
            }
        }
        if !lines.is_empty() {
            // Align each category's inline `# …` comments into a column with `table` (the
            // library function, not the shell wrapper); `trim_end` drops the padding it
            // adds to comment-less rows.
            body += &format!("\n# {label}\n");
            for line in table_formatter::format_table(&lines, 2, 2, None) {
                body.push_str(line.trim_end());
                body.push('\n');
            }
        }
    }

    // Non-Rust companion repos (cloned by the `stainless_sync` bin), aliased to their launchers.
    let comfy = stainless::aliases();
    if !comfy.is_empty() {
        body += &format!("\n# comfy / external tools\n{comfy}");
    }

    // Environment settings, session functions, keybinds: raw shell run when
    // sourcefile.sh is sourced.
    body += &format!("\n# environment\n{}", environment::settings());
    body += &format!("\n# session\n{}", session::functions());
    let binds: String = keybinds::bindings()
        .iter()
        .map(|(key, func)| format!("    bind '\"{key}\": \"{func}\\n\"'\n"))
        .collect();
    let desktop = keybinds::desktop_restart();
    if !binds.is_empty() || !desktop.is_empty() {
        // `bind` is a bash readline builtin; zsh (which also sources this) has none.
        body += &format!("\n# keybinds (bash only)\nif [ -n \"$BASH_VERSION\" ]; then\n{binds}{desktop}fi\n");
    }

    // Flag completion: on TAB after a `-`, ask the binary which flags the command being completed
    // accepts (`complete-flags`, always in sync with the real CLI); any other word falls back to
    // filename completion via `-o default`. `complete` is a bash builtin, so zsh skips the block.
    body += &format!(
        "\n# completion (bash only)\n\
         #   Optional flags displayed through tab-completion. Type `-` and follow up with <TAB><TAB>\n\
         #   e.g. `gg -<TAB><TAB>` lists gg's flags\n\
         #   e.g. `gg --de<TAB>` fills in `--delve`\n\
         if [ -n \"$BASH_VERSION\" ]; then\n\
         \x20   _bashrs_complete() {{\n\
         \x20       local cur=${{COMP_WORDS[COMP_CWORD]}}\n\
         \x20       [[ $cur == -* ]] && COMPREPLY=($(compgen -W \"$({BIN} complete-flags \"${{COMP_WORDS[0]}}\")\" -- \"$cur\"))\n\
         \x20   }}\n\
         \x20   complete -F _bashrs_complete -o default {}\n\
         fi\n",
        completion_names.join(" ")
    );

    // A load greeting, last — after the `gecho`/`boecho` wrappers it calls are defined.
    body += &format!("\n# greeting\n{}\n", greeting::line());

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
    fn complete_flags_reflect_each_variant() {
        let has = |flags: &str, want: &str| flags.split(' ').any(|flag| flag == want);
        let gg = complete_flags("gg");
        for flag in ["-C", "--context", "--delve", "-E", "-s", "--help"] {
            assert!(has(&gg, flag), "gg should complete {flag}: {gg}");
        }
        // A pinned variant doesn't offer the -C it would refuse…
        let gg2 = complete_flags("gg2");
        assert!(!has(&gg2, "-C") && !has(&gg2, "--context"), "gg2 pins -C: {gg2}");
        assert!(has(&gg2, "--delve"), "the rest of the set stays: {gg2}");
        // …and GGG doesn't offer the flags it forces (hidden, still accepted).
        let ggg = complete_flags("GGG");
        for pinned in ["--delve", "-s", "--save", "-E", "--extended-regexp"] {
            assert!(!has(&ggg, pinned), "GGG forces {pinned}; it must not be offered: {ggg}");
        }
        assert!(has(&ggg, "-C"), "GGG still tunes context: {ggg}");
        // Aliases resolve to their command; unknown names yield nothing.
        assert!(has(&complete_flags("upup"), "--help"), "aliases should resolve");
        assert_eq!(complete_flags("no_such_command"), "");
    }

    #[test]
    fn wrappers_register_flag_completion() {
        let script = wrappers();
        assert!(script.contains("_bashrs_complete()"), "completer function missing");
        assert!(
            script.contains("complete-flags \"${COMP_WORDS[0]}\""),
            "the completer should ask the binary at TAB-time"
        );
        let registration = script
            .lines()
            .find(|line| line.trim_start().starts_with("complete -F _bashrs_complete"))
            .expect("registration line missing");
        for name in ["gg", "GGG", "hg", "upup", "media_conv", "lll"] {
            assert!(
                registration.split_whitespace().any(|word| word == name),
                "`{name}` not registered for completion: {registration}"
            );
        }
        assert!(script.contains("\n# completion (bash only)\n"), "should be bash-guarded");
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
        assert_prefixed(&command_names::<MediaCommand>(), "media_", &[]);
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
        has("hg() { history | \"$HOME/.bashrs/bashrs\" hg \"$@\"; }"); // #[piped]: history fed in
        has("g() { \"$HOME/.bashrs/bashrs\" g \"$@\"; }"); // generated g-family (bare)
        has("g3() { \"$HOME/.bashrs/bashrs\" g3 \"$@\"; }");
        has("gg() { \"$HOME/.bashrs/bashrs\" gg \"$@\"; }"); // recursive tree grep (bare)
        has("GG() { \"$HOME/.bashrs/bashrs\" GG \"$@\"; }"); // gg with --delve forced, à la UPUP
    }

    #[test]
    fn wrappers_include_session_function_and_bash_guarded_keybind() {
        let script = wrappers();
        assert!(script.contains("session_new() { exec bash; }"), "session_new missing");
        assert!(script.contains(r#"bind '"\en": "session_new\n"'"#), "ALT+N keybind missing");
        assert!(script.contains(r#"bind '"\eh": "bashrs_sourcefile\n"'"#), "ALT+H keybind missing");
        assert!(script.contains(r#"bind '"\eq": "bashrs_compile\n"'"#), "ALT+Q keybind missing");
        assert!(script.contains("if pgrep -x cinnamon >/dev/null; then bind"), "ALT+L desktop-restart missing");
        assert!(script.contains("if [ -n \"$BASH_VERSION\" ]; then"), "keybinds should be bash-guarded");
    }

    #[test]
    fn wrappers_include_environment_settings() {
        let script = wrappers();
        assert!(script.contains("\n# environment\n"), "environment section missing");
        assert!(script.contains("export GREP_COLORS='mt=7;31'"), "GREP_COLORS export missing");
        assert!(script.contains("export PS1=") && script.contains("(UTC)"), "PS1 export missing");
    }

    #[test]
    fn wrappers_end_with_the_load_greeting() {
        assert!(wrappers().contains("gecho \"Loaded Bashrs!"), "load greeting missing");
    }

    #[test]
    fn wrappers_group_commands_under_a_category_comment() {
        let script = wrappers();
        // A blank line then a comment introduces each category's block.
        assert!(script.contains("\n# bashrs\n"));
        assert!(script.contains("\n# media\n"));
    }

    #[test]
    fn wrappers_exclude_the_internal_commands() {
        assert!(!wrappers().contains("generate() {"));
        assert!(!wrappers().contains("complete-flags() {"));
    }

    #[test]
    fn wrappers_include_the_stainless_aliases() {
        // The alias is emitted whether or not the repo has been cloned yet; only the trailing
        // `--help` comment varies with the environment, so assert just the alias line's stable head.
        assert!(wrappers().contains("ai() { \"$HOME/pydev/bin/python3\""), "stainless `ai` alias missing");
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
        // The command's clap `about` trails its wrapper as a `# ...` comment. Comments are
        // aligned into a column, so the wrapper and its comment are no longer adjacent —
        // assert each is present rather than expecting them side by side.
        let script = wrappers();
        assert!(script.contains("media_metadata() { \"$HOME/.bashrs/bashrs\" media_metadata \"$@\"; }"));
        assert!(script.contains("# Get metadata of an audio/video/image file"));
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
