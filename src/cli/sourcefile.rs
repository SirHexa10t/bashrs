//! The `sourcefile.sh` generator and its completion data — the crate root's *assembly* half
//! (parsing/dispatch is [`super`], which serves this module's output through the hidden
//! `generate`, `install-shell` and `complete-flags` commands). Nothing here leaks downward:
//! each subsystem *contributes* its shell surface — categories their command wrappers and
//! `#[shell_body]` functions, [`crate::tools`] its PATH line, [`crate::drivers::stainless`]
//! its aliases, [`crate::conf`] the environment/keybinds/greeting — and this module gathers
//! them into the one sourced file, plus the per-command flag lists TAB-completion asks for.

use clap::Subcommand;
use super::hide_pinned;
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
use crate::categories::network::NetworkCommand;
use crate::categories::packages::PackagesCommand;
use crate::categories::project::ProjectCommand;
use crate::categories::python::PythonCommand;
use crate::categories::session;
use crate::categories::styles::StyleCommand;
use crate::conf::{config_file, environment, greeting, keybinds};
use crate::conf::RELOAD_EXIT_CODE;
use crate::drivers::stainless;
use crate::tools;

/// A category's pure-shell command (`#[shell_body]`): `(name, body, comment)` — emitted as a
/// plain function, since its work (e.g. `cd`) must run in the calling shell, not the binary.
type ShellFn = (&'static str, &'static str, &'static str);

/// One label's clap group, from a flat list of the category enums that make it up: fold each
/// enum's subcommands into one `clap::Command`, and concatenate their shell functions. Written as
/// a macro because each enum is a distinct *type* — a runtime list would need them erased to fn
/// pointers twice over, once per half. Replaces the right-drifting nest of `augment_subcommands`
/// calls a multi-enum group would otherwise be.
macro_rules! category_group {
    ($label:literal, $($enum:ty),+ $(,)?) => {
        ($label,
         [$(<$enum>::augment_subcommands as fn(clap::Command) -> clap::Command),+]
             .iter()
             .fold(clap::Command::new($label), |cmd, augment| augment(cmd)),
         [$(<$enum>::shell_functions()),+].concat())
    };
}

/// The command categories: the label grouping them in the generated `sourcefile.sh`, the clap
/// graph, and the category's pure-shell commands. One row per category — never per command.
fn category_commands() -> [(&'static str, clap::Command, Vec<ShellFn>); 12] {
    let categories = [
        ("bashrs", BashrsCommand::augment_subcommands(clap::Command::new("bashrs")),
            BashrsCommand::shell_functions().to_vec()),
        ("filesystem", FilesystemCommand::augment_subcommands(clap::Command::new("filesystem")),
            FilesystemCommand::shell_functions().to_vec()),
        ("git", GitCommand::augment_subcommands(clap::Command::new("git")),
            GitCommand::shell_functions().to_vec()),
        ("download", DownloadCommand::augment_subcommands(clap::Command::new("download")),
            DownloadCommand::shell_functions().to_vec()),
        category_group!("media", MediaTranscodeCommand, MediaMetadataCommand,
                                 MediaAudioFxCommand, MediaImagesCommand),
        ("network", NetworkCommand::augment_subcommands(clap::Command::new("network")),
            NetworkCommand::shell_functions().to_vec()),
        ("packages", PackagesCommand::augment_subcommands(clap::Command::new("packages")),
            PackagesCommand::shell_functions().to_vec()),
        ("project", ProjectCommand::augment_subcommands(clap::Command::new("project")),
            ProjectCommand::shell_functions().to_vec()),
        ("python", PythonCommand::augment_subcommands(clap::Command::new("python")),
            PythonCommand::shell_functions().to_vec()),
        // One `lookup` group: `hg` (history search) plus the generated g-family.
        category_group!("lookup", LookupCommand, GrepCommand, GgCommand),
        // One `style` group spanning both style enums: hand-written + generated.
        category_group!("style", StyleCommand, StylizedEchoCommand),
        ("comfy_repos", ComfyReposCommand::augment_subcommands(clap::Command::new("comfy_repos")),
            ComfyReposCommand::shell_functions().to_vec()),
    ];
    categories.map(|(label, cmd, shell_fns)| (label, hide_pinned(cmd), shell_fns))
}

/// Space-separated `-short --long` names of `command`'s arguments (matched by clap name or visible
/// alias, so `upup` completes like `packages_upup`). Positionals and hidden flags (the pinned ones
/// — [`HIDDEN_PINNED`]) are omitted; an unknown name yields nothing, so the completer simply offers
/// no flags. Backs the hidden `complete-flags` command the generated completer calls at TAB-time.
pub(super) fn complete_flags(name: &str) -> String {
    for (_, category, _) in category_commands() {
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
    (MediaTranscodeCommand::wrapper_suffix, MediaTranscodeCommand::wrapper_prefix),
    (MediaMetadataCommand::wrapper_suffix, MediaMetadataCommand::wrapper_prefix),
    (MediaAudioFxCommand::wrapper_suffix, MediaAudioFxCommand::wrapper_prefix),
    (MediaImagesCommand::wrapper_suffix, MediaImagesCommand::wrapper_prefix),
    (NetworkCommand::wrapper_suffix, NetworkCommand::wrapper_prefix),
    (PackagesCommand::wrapper_suffix, PackagesCommand::wrapper_prefix),
    (ProjectCommand::wrapper_suffix, ProjectCommand::wrapper_prefix),
    (PythonCommand::wrapper_suffix, PythonCommand::wrapper_prefix),
    (LookupCommand::wrapper_suffix, LookupCommand::wrapper_prefix),
    (GrepCommand::wrapper_suffix, GrepCommand::wrapper_prefix),
    (GgCommand::wrapper_suffix, GgCommand::wrapper_prefix),
    (StyleCommand::wrapper_suffix, StyleCommand::wrapper_prefix),
    (StylizedEchoCommand::wrapper_suffix, StylizedEchoCommand::wrapper_prefix),
    (ComfyReposCommand::wrapper_suffix, ComfyReposCommand::wrapper_prefix),
];

/// The completion lines for the comfy/external aliases — `(alias, space-joined flags)` rows
/// contributed by [`stainless::aliases`], whose flag lists were probed from each tool's own
/// `--help` at generate time (the binary can't answer for them: they aren't clap commands, so
/// `complete-flags` knows nothing about them). Rendered as a sibling of `_bashrs_complete`
/// inside the same bash-only block, with the same contract: flags only once `-` is typed,
/// filenames otherwise (`-o default`). Empty input renders nothing — a sourcefile generated
/// before the first repo sync simply has no comfy completion.
fn comfy_complete_block(entries: &[(String, String)]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let arms: String = entries
        .iter()
        .map(|(alias, flags)| format!("            {alias}) flags=\"{flags}\" ;;\n"))
        .collect();
    let names: Vec<&str> = entries.iter().map(|(alias, _)| alias.as_str()).collect();
    format!(
        "\x20   _bashrs_comfy_complete() {{\n\
         \x20       local cur=${{COMP_WORDS[COMP_CWORD]}}\n\
         \x20       [[ $cur == -* ]] || return 0\n\
         \x20       local flags=\"\"\n\
         \x20       case \"${{COMP_WORDS[0]}}\" in\n\
         {arms}\
         \x20       esac\n\
         \x20       COMPREPLY=($(compgen -W \"$flags\" -- \"$cur\"))\n\
         \x20   }}\n\
         \x20   complete -F _bashrs_comfy_complete -o default {}\n",
        names.join(" ")
    )
}

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
pub(super) fn wrappers() -> String {
    // Quoted so `$HOME` expands at call time and paths with spaces stay intact.
    const BIN: &str = "\"$HOME/.bashrs/bashrs\"";

    let mut body = String::new();
    let mut completion_names: Vec<String> = Vec::new(); // every wrapper + alias, for `complete -F`
    for (label, category, shell_fns) in category_commands() {
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
        // The category's pure-shell commands (`#[shell_body]`): inline bodies, no binary call —
        // and no completion registration, which would replace bash's default (filename)
        // completion with a flagless void. The `unalias` must sit on its own line: a live alias
        // with the same name (e.g. the user's own rc still carrying `alias ..='cd ..'`) would be
        // alias-expanded INTO the definition as bash parses it — a syntax error that aborts the
        // whole source. `|| true` pins the usual no-such-alias failure to status 0, so a caller
        // running under `set -e` (sourced files inherit it) isn't aborted either. Same gotcha,
        // same cure as the bundled-tools `python3` function.
        for (name, fn_body, comment) in shell_fns {
            let about = if comment.is_empty() { String::new() } else { format!("  # {comment}") };
            lines.push(format!("unalias {name} 2>/dev/null || true"));
            lines.push(format!("{name}() {{ {fn_body}; }}{about}"));
        }
        if !lines.is_empty() {
            // Align each category's inline `# …` comments into a column with `table` (the
            // library function, not the shell wrapper); `trim_trailing` drops the padding it
            // adds to comment-less rows. The fallback is unreachable: only `sort` can error,
            // and it's off.
            body += &format!("\n# {label}\n");
            // Default delimiters (2-space split/join); `trim_trailing` drops the padding added to
            // comment-less rows. Fallback unreachable: sort is off and the defaults are valid.
            let opts = table_formatter::FormatOptions { trim_trailing: true, ..Default::default() };
            for line in table_formatter::format_table(&lines, &opts).unwrap_or(lines) {
                body.push_str(&line);
                body.push('\n');
            }
        }
    }

    // Non-Rust companion repos (cloned by the hidden `install-stainless` command), aliased to their launchers.
    let (comfy, comfy_completions) = stainless::aliases();
    if !comfy.is_empty() {
        body += &format!("\n# comfy / external tools\n{comfy}");
    }

    // Bundled tools (fetched and kept current by the same bin): their shim dir wins by PATH.
    body += &format!("\n# bundled tools\n{}", tools::shell_setup());

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
         {}fi\n",
        completion_names.join(" "),
        comfy_complete_block(&comfy_completions)
    );

    // A load greeting — after the `gecho`/`boecho` wrappers it calls are defined.
    body += &format!("\n# greeting\n{}", greeting::line());

    // The very last lines, deliberately: while an archived config sits unmerged, the red nag
    // (via `errcho`, defined above) is the closest thing to the user's prompt.
    body += &format!("\n\n# stale-config check\n{}", config_file::stale_config_notice());

    let mut out = String::from(
        "#!/usr/bin/env bash\n\
         # Auto-generated by `bashrs generate` — do not edit.\n\
         # Regenerate by re-running COMPILE.sh.\n",
    );
    if !body.is_empty() {
        // Interactive shells only, from the very top: a non-interactive shell that sources rc
        // files (scp, `ssh host cmd`, wrapper scripts) must inherit NOTHING — not the greeting
        // (stdout corrupts scp), and above all not the `python3` function, which would silently
        // reroute an unsuspecting external script (a distro upgrade hook, say) onto the bundled
        // interpreter. A PS1 probe would work equally well in bash, which sets PS1 in every
        // interactive shell and even scrubs an inherited one from non-interactive shells
        // (verified) — but `$-` *is* the interactivity flag, so it stays correct even in shells
        // that don't maintain PS1's invariant (non-interactive zsh keeps a default PS1).
        // `return 0` explicitly, so the early-out doesn't parrot the caller's stale status.
        out += "\ncase $- in *i*) ;; *) return 0 ;; esac  # interactive shells only — non-interactive contexts must inherit nothing\n";
        // The bashrs-free escape hatch: `session_bare` starts a shell with this flag exported,
        // and this guard CONSUMES it — unset, then return — so the shell comes up without the
        // surface and without the flag lingering in its environment. One-shot by design: any
        // later shell (or re-sourcing this file by hand) arms bashrs again.
        out += "[ -n \"$_BASHRS_BARE\" ] && { unset _BASHRS_BARE ; return 0; }  # session_bare: skip bashrs for THIS shell, flag consumed (any new shell arms again)\n";
        // Bail early (before defining anything) if the binary isn't present. `return 0`, not a
        // bare `return`: bare would silently forward the failed test's status 1 to whatever
        // sourced this file, and a not-yet-installed bashrs is a quiet no-op, not an error.
        out += &format!("[ -f {BIN} ] || return 0\n");
        out += &body;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::CommandFactory;

    #[test]
    fn complete_flags_reflect_each_variant() {
        let has = |flags: &str, want: &str| flags.split(' ').any(|flag| flag == want);
        let gg = complete_flags("gg");
        for flag in ["-C", "--context", "--delve", "-E", "-e", "--regexp", "-s", "--help"] {
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
        // A pinned upstream flag is hidden the same way: bitrot-finding forces --eager-checksum.
        let bitrot = complete_flags("backup_find_bitrot");
        assert!(!has(&bitrot, "--eager-checksum"), "forced flag must not be offered: {bitrot}");
        assert!(has(&bitrot, "--from"), "the rest of the upstream set stays: {bitrot}");
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
        for name in ["gg", "GGG", "hg", "upup", "media_convert", "lll"] {
            assert!(
                registration.split_whitespace().any(|word| word == name),
                "`{name}` not registered for completion: {registration}"
            );
        }
        assert!(
            registration.split_whitespace().all(|word| word != ".."),
            "shell-body commands take no flags — registering would kill default completion: {registration}"
        );
        assert!(script.contains("\n# completion (bash only)\n"), "should be bash-guarded");
    }

    #[test]
    fn comfy_completion_renders_gated_case_arms_or_nothing() {
        // One case arm per alias, registered on its own completer with the same `-` gate and
        // filename fallback as `_bashrs_complete`; pinned-flag filtering happens upstream
        // (stainless), so this renders rows verbatim. No rows → no block at all.
        let rows = [("q".to_string(), "--explain --research -h --help".to_string()),
                    ("ai".to_string(), "--dry-run".to_string())];
        let block = comfy_complete_block(&rows);
        assert!(block.contains("q) flags=\"--explain --research -h --help\" ;;"), "{block}");
        assert!(block.contains("ai) flags=\"--dry-run\" ;;"), "{block}");
        assert!(block.contains("[[ $cur == -* ]] || return 0"), "flags only after `-`: {block}");
        assert!(
            block.contains("complete -F _bashrs_comfy_complete -o default q ai"),
            "both aliases registered: {block}"
        );
        assert_eq!(comfy_complete_block(&[]), "", "no rows, no block");
    }

    #[test]
    fn wrappers_cover_every_command_and_alias() {
        let script = wrappers();
        let has = |line: &str| assert!(script.contains(line), "missing wrapper line: {line}");
        has("fs_usage() { \"$HOME/.bashrs/bashrs\" fs_usage \"$@\"; }");
        has("dl_page_links() { \"$HOME/.bashrs/bashrs\" dl_page_links \"$@\"; }");
        has("dl() { \"$HOME/.bashrs/bashrs\" dl \"$@\"; }");
        has("media_convert() { \"$HOME/.bashrs/bashrs\" media_convert \"$@\"; }");
        has("media_convert_quality() { \"$HOME/.bashrs/bashrs\" media_convert_quality \"$@\"; }");
        has("media_convert_compact() { \"$HOME/.bashrs/bashrs\" media_convert_compact \"$@\"; }");
        has("media_trim_start() { \"$HOME/.bashrs/bashrs\" media_trim_start \"$@\"; }");
        has("media_metadata() { \"$HOME/.bashrs/bashrs\" media_metadata \"$@\"; }");
        has("media_hmerge_imgs() { \"$HOME/.bashrs/bashrs\" media_hmerge_imgs \"$@\"; }");
        has("media_vmerge_imgs() { \"$HOME/.bashrs/bashrs\" media_vmerge_imgs \"$@\"; }");
        has("packages_upup() { \"$HOME/.bashrs/bashrs\" packages_upup \"$@\"; }"); // packages (both)
        has("upup() { \"$HOME/.bashrs/bashrs\" packages_upup \"$@\"; }"); // unprefixed alias -> packages_upup
        has("packages_print() { \"$HOME/.bashrs/bashrs\" packages_print \"$@\"; }"); // prefixed only
        has("packages_update_toolchains() { \"$HOME/.bashrs/bashrs\" packages_update_toolchains \"$@\"; }"); // prefixed only
        has("UPUP() { \"$HOME/.bashrs/bashrs\" UPUP \"$@\"; }"); // custom-named: update everything
        has("backup_diff() { \"$HOME/.bashrs/bashrs\" backup_diff \"$@\"; }"); // comfy: flattened filesync subcommand
        has("backup_find_bitrot() { \"$HOME/.bashrs/bashrs\" backup_find_bitrot \"$@\"; }"); // comfy: pinned variant
        // bashrs_compile starts a fresh session only when compile signals a reload (exit code)
        has(&format!(
            "bashrs_compile() {{ \"$HOME/.bashrs/bashrs\" bashrs_compile \"$@\"; [ \"$?\" -eq {RELOAD_EXIT_CODE} ] && session_new; }}"
        ));
        has("bashrs_sourcefile() { \"$HOME/.bashrs/bashrs\" bashrs_sourcefile \"$@\"; }");
        has("bashrs_configure() { \"$HOME/.bashrs/bashrs\" bashrs_configure \"$@\"; }");
        has("recho() { \"$HOME/.bashrs/bashrs\" recho \"$@\"; }"); // style: bare, unprefixed
        has("py_install() { \"$HOME/.bashrs/bashrs\" py_install \"$@\"; }"); // python: bundled-env packages
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
        assert!(script.contains("session_bare() { _BASHRS_BARE=1 exec bash; }"), "session_bare missing");
        // The one-shot guard: consume the flag AND return, so a bare session starts with a clean
        // environment and the very next shell arms bashrs again.
        assert!(
            script.contains("[ -n \"$_BASHRS_BARE\" ] && { unset _BASHRS_BARE ; return 0; }"),
            "the flag-consuming bare guard must sit at the sourcefile's top"
        );
        assert!(script.contains(r#"bind '"\en": "session_new\n"'"#), "ALT+N keybind missing");
        assert!(script.contains(r#"bind '"\e\C-n": "session_bare\n"'"#), "CTRL+ALT+N keybind missing");
        assert!(script.contains(r#"bind '"\eh": "bashrs_sourcefile\n"'"#), "ALT+H keybind missing");
        assert!(script.contains(r#"bind '"\ew": "bashrs_configure\n"'"#), "ALT+W keybind missing");
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
    fn the_interactivity_guard_tops_the_file_ahead_of_everything() {
        // Non-interactive shells must inherit nothing at all — most of all not the `python3`
        // function, which would reroute external scripts onto the bundled interpreter.
        let script = wrappers();
        let guard =
            script.find("case $- in *i*) ;; *) return 0 ;; esac").expect("interactivity guard missing");
        assert_eq!(script.matches("case $- in").count(), 1, "exactly one guard");
        let binary = script.find("[ -f \"$HOME/.bashrs/bashrs\" ] || return 0").expect("binary guard");
        let first_definition = script.find("() {").expect("some wrapper");
        assert!(guard < binary && binary < first_definition, "guards first, cheapest first");
    }

    #[test]
    fn shell_body_commands_emit_inline_functions_in_their_category() {
        // `..` must run in the calling shell (a child can't `cd` its parent), so its wrapper
        // carries the body itself — no binary call — grouped under its category like any command.
        let script = wrappers();
        assert!(script.contains("..() { cd .. \"$@\"; }"), "`..` shell function missing:\n{script}");
        let filesystem = script.split("\n# ").find(|s| s.starts_with("filesystem")).expect("filesystem section");
        assert!(filesystem.contains("..() {") && filesystem.contains("# Hop one directory up"),
            "the shell function belongs to its category, doc comment included: {filesystem}");
        assert!(!script.contains("..() { \"$HOME/.bashrs/bashrs\""),
            "a shell-body command must not call the binary");
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
        assert!(!wrappers().contains("install-shell() {"));
        assert!(!wrappers().contains("install-stainless() {"));
    }

    #[test]
    fn wrappers_include_the_stainless_aliases() {
        // The alias is emitted whether or not the repo has been cloned yet; only the trailing
        // `--help` comment varies with the environment, so assert just the alias line's stable head.
        assert!(wrappers().contains("ai() { python3 \"$HOME/.bashrs/stainless_comfy/"), "stainless `ai` alias missing");
        // …and the same clone's auxiliary entry point: `ai_audit_self`, a `-m` module run from the tool's dir.
        assert!(
            wrappers().contains("ai_audit_self() { (cd \"$HOME/.bashrs/stainless_comfy/contAInerized/dockerized_claude_code\" >/dev/null && python3 -m launch.audit \"$@\"); }"),
            "stainless `ai_audit_self` aux alias missing"
        );
        // …and the same clone's `quick_question.py` script family: `q` bare, `q3` with a pinned flag.
        assert!(
            wrappers().contains("q() { python3 \"$HOME/.bashrs/stainless_comfy/contAInerized/dockerized_claude_code/quick_question.py\" \"$@\"; }"),
            "stainless `q` script alias missing"
        );
        assert!(
            wrappers().contains("q3() { python3 \"$HOME/.bashrs/stainless_comfy/contAInerized/dockerized_claude_code/quick_question.py\" --research \"$@\"; }"),
            "stainless `q3` script alias missing"
        );
    }

    #[test]
    fn wrappers_put_the_bundled_tools_on_path() {
        let script = wrappers();
        assert!(script.contains("\n# bundled tools\n"), "section missing");
        assert!(script.contains("PATH=\"$HOME/.bashrs/tools/bin:$PATH\""), "shim-dir prepend missing");
    }

    #[test]
    fn wrappers_leave_no_variable_behind_in_the_shell() {
        // The path is inlined per function, so sourcing defines functions only —
        // nothing lingers in the user's environment afterward.
        assert!(!wrappers().contains("__bashrs_bin"));
    }

    #[test]
    fn wrappers_bail_early_when_the_binary_is_missing() {
        // `return 0` explicitly: a bare `return` would forward the failed test's status 1 to
        // whatever sourced the file, and "bashrs isn't installed" is a no-op, not an error.
        assert!(wrappers().contains("[ -f \"$HOME/.bashrs/bashrs\" ] || return 0"));
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
            .flat_map(|(_, cmd, _)| {
                cmd.get_subcommands().map(|c| c.get_name().to_string()).collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(dispatchable, grouped, "every CLI command must belong to exactly one category");
    }
}
