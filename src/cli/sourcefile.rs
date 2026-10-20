//! The sourcefile generator and its completion data — the crate root's *assembly* half
//! (parsing/dispatch is [`super`], which serves this module's output through the hidden
//! `generate`, `install-shell` and `complete-flags` commands). Nothing here leaks downward:
//! each subsystem *contributes* its shell surface — categories their command wrappers and
//! `#[shell_body]` functions, [`crate::tools`] its PATH line, [`crate::drivers::stainless`]
//! its aliases, [`crate::conf`] the environment/keybinds/greeting — and this module gathers
//! them into the one sourced file, plus the per-command flag lists TAB-completion asks for.
//!
//! One file per [`Shell`]: `sourcefile.sh` (bash, and zsh) and `sourcefile.fish`. Both come out
//! of the one sequence in [`wrappers`] — the same sections, in the same order, from the same
//! data — and each section that must be written differently `match`es on the shell, so neither a
//! new section nor a new shell compiles until every one of them says how it is written. What each
//! shell's syntax needs lives in its own child, [`bash`] and [`fish`].

mod bash;
mod fish;

use std::collections::BTreeMap;

use clap::Subcommand;
use super::adjust_pinned;
use crate::categories::autogen_lookup::{GgCommand, GrepCommand};
use crate::categories::autogen_styles::StylizedEchoCommand;
use crate::categories::ai_fingerprint::AiFingerprintCommand;
use crate::categories::bashrs::BashrsCommand;
use crate::categories::builtins::BuiltinsCommand;
use crate::categories::comfy_repos::ComfyReposCommand;
use crate::categories::download::DownloadCommand;
use crate::categories::exposed_helpers::ExposedHelpersCommand;
use crate::categories::filesystem::FilesystemCommand;
use crate::categories::git::GitCommand;
use crate::categories::lookup::LookupCommand;
use crate::categories::media::audio_fx::MediaAudioFxCommand;
use crate::categories::media::images::MediaImagesCommand;
use crate::categories::media::metadata::MediaMetadataCommand;
use crate::categories::media::transcode::MediaTranscodeCommand;
use crate::categories::network::NetworkCommand;
use crate::categories::packages::PackagesCommand;
use crate::categories::processes::ProcessesCommand;
use crate::categories::project::ProjectCommand;
use crate::categories::python::PythonCommand;
use crate::categories::shell::ShellCommand;
use crate::categories::styles::StyleCommand;
use crate::conf::{config_file, environment, greeting};
use crate::drivers::stainless;
use crate::support::shell_dialect::{fish_original, Shell, ShellText};
use crate::tools;

/// A category's pure-shell command (`#[shell_body]`): `(name, bodies, comment)` — emitted as a
/// plain function, since its work (e.g. `cd`) must run in the calling shell, not the binary.
type ShellFn = (&'static str, ShellText, &'static str);

/// What one category enum contributes to the sourcefile — the four things `#[category]`
/// generates for every enum, held as fn pointers so that one list can hold enums of distinct
/// types.
struct Surface {
    /// Adds the enum's subcommands to a clap group (clap's `augment_subcommands`).
    commands: fn(clap::Command) -> clap::Command,
    /// The enum's pure-shell commands (`#[shell_body]`).
    shell_functions: fn() -> &'static [ShellFn],
    /// The function a command's wrapper runs on a reload, by command name (`#[after("…")]`).
    wrapper_suffix: fn(&str) -> Option<&'static str>,
    /// The probe piped into a command's wrapper, by command name (`#[piped(…)]`).
    wrapper_prefix: fn(&str) -> Option<ShellText>,
}

/// A category enum's [`Surface`]. A macro because each enum is a distinct *type*; the four fn
/// pointers are the erasure that lets [`CATEGORIES`] list them all.
macro_rules! surface {
    ($enum:ty) => {
        Surface {
            commands: <$enum>::augment_subcommands,
            shell_functions: <$enum>::shell_functions,
            wrapper_suffix: <$enum>::wrapper_suffix,
            wrapper_prefix: <$enum>::wrapper_prefix,
        }
    };
}

/// The command categories — and the ONE place a category is registered with the sourcefile: the
/// label heading its section, and the enum(s) whose commands it groups. The clap graph, the
/// shell functions and the wrapper hooks are all derived from this list, so a category cannot be
/// in one and missing from another. One row per category — never per command.
const CATEGORIES: &[(&str, &[Surface])] = &[
    ("bashrs", &[surface!(BashrsCommand)]),
    ("overrides", &[surface!(BuiltinsCommand)]),
    ("filesystem", &[surface!(FilesystemCommand)]),
    ("git", &[surface!(GitCommand)]),
    ("download", &[surface!(DownloadCommand)]),
    ("exposed_helpers", &[surface!(ExposedHelpersCommand)]),
    ("media", &[surface!(MediaTranscodeCommand), surface!(MediaMetadataCommand),
                surface!(MediaAudioFxCommand), surface!(MediaImagesCommand)]),
    ("network", &[surface!(NetworkCommand)]),
    ("packages", &[surface!(PackagesCommand)]),
    ("processes", &[surface!(ProcessesCommand)]),
    ("project", &[surface!(ProjectCommand)]),
    ("python", &[surface!(PythonCommand)]),
    // `shell_new`/`shell_bare` are `#[shell_body]` (they `exec` the calling shell);
    // `sudo_bashrs` and `print_args` are ordinary commands with generated wrappers.
    ("shell", &[surface!(ShellCommand)]),
    // One `lookup` group. The generated `g`/`gg` families come first so the hand-written
    // commands can follow what they are built on: `GG` is `gg --delve` and `GGG` is `GG`
    // plus more, which only reads that way listed after `gg`. Within that last enum `hg`
    // is declared after them, so the history search closes the section.
    ("lookup", &[surface!(GrepCommand), surface!(GgCommand), surface!(LookupCommand)]),
    // One `style` group spanning both style enums: hand-written + generated.
    ("style", &[surface!(StyleCommand), surface!(StylizedEchoCommand)]),
    // One `comfy_repos` group: the Rust companions this shell wraps, plus the AI-fingerprint
    // command over the `ai_detection` crate — an adaptation of a dependency like every other
    // entry here, so it belongs beside them rather than in a category of its own.
    ("comfy_repos", &[surface!(ComfyReposCommand), surface!(AiFingerprintCommand)]),
];

/// Every category enum's [`Surface`], in section order.
fn surfaces() -> impl Iterator<Item = &'static Surface> {
    CATEGORIES.iter().flat_map(|(_, surfaces)| surfaces.iter())
}

/// The categories as the generator reads them: each label with one `clap::Command` holding all
/// its enums' subcommands (the pinned flags hidden — [`adjust_pinned`]) and its pure-shell
/// commands.
fn category_commands() -> Vec<(&'static str, clap::Command, Vec<ShellFn>)> {
    CATEGORIES
        .iter()
        .map(|(label, surfaces)| {
            let group = surfaces.iter().fold(clap::Command::new(*label), |cmd, surface| (surface.commands)(cmd));
            let shell_fns = surfaces.iter().flat_map(|surface| (surface.shell_functions)()).copied().collect();
            (*label, adjust_pinned(group), shell_fns)
        })
        .collect()
}

/// One category as both shells write it: its label, its binary-backed commands, and its
/// pure-shell ones.
struct Section {
    label: &'static str,
    wrappers: Vec<Wrapper>,
    bodies: Vec<ShellFn>,
}

/// A binary-backed command as both shells' wrappers need it.
struct Wrapper {
    /// The clap name every wrapper calls — an alias is purely a shell-side convenience and never
    /// has to resolve as a clap alias.
    real: String,
    /// Every name the command answers to: the real one, then its visible aliases.
    names: Vec<String>,
    /// The first line of its clap `about`.
    about: Option<String>,
    /// The function to run when the command asks for a reload (`#[after]`).
    after: Option<&'static str>,
    /// What the shell pipes into it (`#[piped]`).
    piped: Option<ShellText>,
}

/// [`category_commands`] as [`Section`]s, every hidden command left out.
fn sections() -> Vec<Section> {
    category_commands()
        .into_iter()
        .map(|(label, category, bodies)| {
            let wrappers = category
                .get_subcommands()
                .filter(|sub| !sub.is_hide_set()) // skip any command marked internal
                .map(|sub| Wrapper {
                    real: sub.get_name().to_string(),
                    names: std::iter::once(sub.get_name()).chain(sub.get_visible_aliases()).map(str::to_string).collect(),
                    about: first_line(sub.get_about()),
                    after: wrapper_suffix(sub.get_name()),
                    piped: wrapper_prefix(sub.get_name()),
                })
                .collect();
            Section { label, wrappers, bodies }
        })
        .collect()
}

/// A clap help text's first line, as the comments and descriptions carry it — first line only, so
/// a comment can't run over.
fn first_line(text: Option<&clap::builder::StyledStr>) -> Option<String> {
    text.map(ToString::to_string)
        .and_then(|text| text.lines().next().map(str::to_string))
        .filter(|line| !line.is_empty())
}

/// Command name → one-line description, for the keybind comments, which look their bound
/// function up here.
fn abouts(sections: &[Section]) -> BTreeMap<String, String> {
    let mut abouts = BTreeMap::new();
    for section in sections {
        for wrapper in &section.wrappers {
            if let Some(about) = &wrapper.about {
                abouts.insert(wrapper.real.clone(), about.clone());
            }
        }
        for (name, _, comment) in &section.bodies {
            if !comment.is_empty() {
                abouts.insert((*name).to_string(), (*comment).to_string());
            }
        }
    }
    abouts
}

/// The function a command's wrapper runs when it asks for a reload (e.g. `shell_new`).
fn wrapper_suffix(name: &str) -> Option<&'static str> {
    // Command names are unique across categories (clap flattening requires it): first match wins.
    surfaces().find_map(|surface| (surface.wrapper_suffix)(name))
}

/// The probe piped into a command's wrapper (ahead of the binary) — e.g. `hg` searches the shell
/// history, which only the shell itself can produce, so its wrapper runs `history` and pipes it in.
fn wrapper_prefix(name: &str) -> Option<ShellText> {
    surfaces().find_map(|surface| (surface.wrapper_prefix)(name))
}

/// The placeholder for the project directory this binary was compiled from.
const PROJECT_ROOT: &str = "{PROJECT_ROOT}";
/// The placeholder (fish) for the shell's own definition of an override's name.
const ORIGINAL: &str = "{ORIGINAL}";

/// A hand-written fragment with its generation-time substitutions made. The attributes only take
/// string literals, but the generator runs inside the binary — which is exactly where compile-time
/// knowledge lives — so these are baked into the emitted text instead of fetched through a
/// subprocess at every call:
/// - [`PROJECT_ROOT`] — the project directory, quoted for the shell the fragment is written in;
/// - [`ORIGINAL`], fish only — the shell's own definition of `name`, which [`fish`] saves under
///   another name before the override replaces it.
fn substitute(text: &str, shell: Shell, name: &str) -> String {
    let text = text.replace(PROJECT_ROOT, &shell.quote(env!("CARGO_MANIFEST_DIR")));
    match shell {
        Shell::Bash => text,
        Shell::Fish => text.replace(ORIGINAL, &fish_original(name)),
    }
}

/// One flag a command offers for completion, read from clap's model, so that neither shell's
/// completion can drift from what the command accepts.
struct Flag {
    short: Option<char>,
    long: Option<String>,
    /// The first line of the flag's help.
    help: Option<String>,
}

/// The flags `command` offers: every argument that is neither positional nor hidden (the pinned
/// ones are hidden — [`HIDDEN_PINNED`]), plus the auto help flag, which only materializes when
/// clap *builds* a command, which these introspection copies aren't (no command disables it).
fn flags(command: &clap::Command) -> Vec<Flag> {
    let mut flags: Vec<Flag> = command
        .get_arguments()
        .filter(|arg| !arg.is_positional() && !arg.is_hide_set())
        .map(|arg| Flag { short: arg.get_short(), long: arg.get_long().map(str::to_string), help: first_line(arg.get_help()) })
        .collect();
    if !flags.iter().any(|flag| flag.long.as_deref() == Some("help")) {
        flags.push(Flag { short: Some('h'), long: Some("help".into()), help: Some("Print help".to_string()) });
    }
    flags
}

/// The flags of the command named `name` (its clap name or a visible alias, so `upup` completes
/// like `packages_upup`) as `shell`'s completer reads them — [`flags`], spelled `-short` and
/// `--long`. Bash's takes them space-separated; fish's one per line, each with its help after a
/// tab, which fish shows beside it. An unknown name yields nothing, so the completer simply
/// offers no flags. Backs the hidden `complete-flags` command the generated completers call at
/// TAB-time.
pub(super) fn complete_flags(name: &str, shell: Shell) -> String {
    for (_, category, _) in category_commands() {
        for sub in category.get_subcommands() {
            if sub.get_name() != name && !sub.get_visible_aliases().any(|alias| alias == name) {
                continue;
            }
            let spelled = flags(sub).into_iter().flat_map(|flag| {
                let names = [flag.short.map(|short| format!("-{short}")), flag.long.map(|long| format!("--{long}"))];
                names.into_iter().flatten().map(move |spelling| (spelling, flag.help.clone()))
            });
            return match shell {
                Shell::Bash => spelled.map(|(spelling, _)| spelling).collect::<Vec<_>>().join(" "),
                Shell::Fish => spelled
                    .map(|(spelling, help)| match help {
                        Some(help) => format!("{spelling}\t{help}"),
                        None => spelling,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
        }
    }
    String::new()
}

/// The sourcefile for `shell` — [`generated`]'s text, which is all the `generate` command prints.
pub(super) fn wrappers(shell: Shell) -> String {
    generated(shell).0
}

/// Build the sourcefile for `shell` — `~/.bashrs/sourcefile.sh` for bash (which zsh also sources),
/// `~/.bashrs/sourcefile.fish` for fish — and the names it gives completions to, which the install
/// needs for fish (see [`fish::completion`]).
///
/// Definitions are grouped by category, each under a comment header. Every
/// wrapper — including unprefixed aliases — dispatches to the command's *real* name,
/// so an alias is purely a shell-side convenience and never has to resolve as a
/// clap alias. The binary path is inlined into each function rather than held in
/// a shared variable, so sourcing leaves nothing behind in the user's shell.
pub(super) fn generated(shell: Shell) -> (String, Vec<String>) {
    let sections = sections();
    let mut body = String::new();
    for section in &sections {
        let definitions = match shell {
            Shell::Bash => bash::definitions(section),
            Shell::Fish => fish::definitions(section),
        };
        if !definitions.is_empty() {
            body += &format!("\n# {}\n{definitions}", section.label);
        }
    }

    // Non-Rust companion repos (cloned by the hidden `install-stainless` command), aliased to their launchers.
    let (comfy, stainless_flags) = stainless::aliases(shell);
    if !comfy.is_empty() {
        body += &format!("\n# stainless comfy repos\n{comfy}");
    }

    // Bundled tools (fetched and kept current by the same bin): their shim dir wins by PATH.
    body += &format!("\n# bundled tools\n{}", tools::shell_setup(shell));

    // Environment settings and keybinds: raw shell run when the sourcefile is sourced.
    // (The session commands are a category now — emitted above with the rest.)
    body += &format!("\n# environment\n{}", environment::settings(shell));

    let abouts = abouts(&sections);
    body += &match shell {
        Shell::Bash => bash::keybinds(&abouts),
        Shell::Fish => fish::keybinds(&abouts),
    };
    let (completion, completed) = match shell {
        Shell::Bash => bash::completion(&sections, &stainless_flags),
        Shell::Fish => fish::completion(&sections, &stainless_flags),
    };
    body += &completion;

    // A load greeting — after the `gecho`/`boecho` wrappers it calls are defined.
    body += &format!("\n# greeting\n{}", greeting::line(shell));

    // The very last lines, deliberately: while an archived config sits unmerged, the red nag
    // (via `errcho`, defined above) is the closest thing to the user's prompt.
    body += &format!("\n\n# stale-config check\n{}", config_file::stale_config_notice(shell));

    let file = match shell {
        Shell::Bash => bash::file(&body),
        Shell::Fish => fish::file(&body),
    };
    (file, completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` with every run of two-plus spaces collapsed to one — for asserting a line's
    /// CONTENT while staying agnostic about the aligner's gaps, which have their own
    /// column-equality tests. Newlines are untouched, so line-based checks still work.
    fn flat(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut spaces = 0_usize;
        for ch in text.chars() {
            if ch == ' ' {
                spaces += 1;
                continue;
            }
            out.push_str(&" ".repeat(spaces.min(1)));
            spaces = 0;
            out.push(ch);
        }
        out
    }
    use crate::cli::Cli;
    use crate::conf::{keybinds, RELOAD_EXIT_CODE};
    use clap::CommandFactory;

    #[test]
    fn complete_flags_reflect_each_variant() {
        let has = |flags: &str, want: &str| flags.split(' ').any(|flag| flag == want);
        let complete_flags = |name| complete_flags(name, Shell::Bash);
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

    /// Fish's completer takes the same flags one per line, each with its help after a tab — the
    /// description fish shows beside it — and both spellings of a flag carry the same one, which
    /// fish's pager then lists together.
    #[test]
    fn fish_gets_the_same_flags_each_with_its_description() {
        let fish = complete_flags("gg", Shell::Fish);
        let bash = complete_flags("gg", Shell::Bash);
        let spellings: Vec<&str> = fish.lines().map(|line| line.split('\t').next().unwrap_or(line)).collect();
        assert_eq!(spellings.join(" "), bash, "the same flags, in the same order");
        let help = |spelling: &str| fish.lines().find_map(|line| line.strip_prefix(&format!("{spelling}\t"))).map(str::to_string);
        assert_eq!(help("-C"), help("--context"), "both spellings, one description");
        assert!(help("--context").is_some_and(|help| !help.is_empty()), "{fish}");
        assert_eq!(help("--help").as_deref(), Some("Print help"));
        assert_eq!(complete_flags("no_such_command", Shell::Fish), "");
    }

    #[test]
    fn wrappers_register_flag_completion() {
        let script = wrappers(Shell::Bash);
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
        let block = bash::stainless_flags_block(&rows);
        // Arms hold their rows (spacing-agnostic — the gap is the aligner's), and `flags=`
        // sits in one column across them, so the case reads alias left, flag list right.
        assert!(block.contains("q)") && block.contains("flags=\"--explain --research -h --help\" ;;"), "{block}");
        assert!(block.contains("ai)") && block.contains("flags=\"--dry-run\" ;;"), "{block}");
        let flag_columns: std::collections::BTreeSet<usize> = block
            .lines()
            .filter(|line| line.contains(") "))
            .filter_map(|line| line.find("flags=\""))
            .collect();
        assert_eq!(flag_columns.len(), 1, "the case arms are not aligned: {block}");
        assert!(block.contains("[[ $cur == -* ]] || return 0"), "flags only after `-`: {block}");
        assert!(
            block.contains("complete -F _bashrs_stainless_flags -o default q ai"),
            "both aliases registered: {block}"
        );
        assert_eq!(bash::stainless_flags_block(&[]), "", "no rows, no block");
    }

    #[test]
    fn wrappers_cover_every_command_and_alias() {
        // Collapsed before comparing: the aligner's gaps are asserted by the column tests,
        // and every line here states CONTENT only.
        let script = flat(&wrappers(Shell::Bash));
        let has = |line: &str| assert!(script.contains(line), "missing wrapper line: {line}");
        has("fs_usage() { \"$HOME/.bashrs/bashrs\" fs_usage \"$@\"; }");
        has("dl_page_links() { \"$HOME/.bashrs/bashrs\" dl_page_links \"$@\"; }");
        has("dl() { \"$HOME/.bashrs/bashrs\" dl \"$@\"; }");
        has("media_convert() { \"$HOME/.bashrs/bashrs\" media_convert \"$@\"; }");
        has("media_convert_quality() { \"$HOME/.bashrs/bashrs\" media_convert_quality \"$@\"; }");
        has("media_convert_compact() { \"$HOME/.bashrs/bashrs\" media_convert_compact \"$@\"; }");
        has("media_trim() { \"$HOME/.bashrs/bashrs\" media_trim \"$@\"; }");
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
        has("diff() { \"$HOME/.bashrs/bashrs\" diff \"$@\"; }"); // overrides: a system command shadowed under its own name
        // brs_compile starts a fresh session only when compile signals a reload (exit code), and
        // returns the compile's own status otherwise
        has(&format!(
            "brs_compile() {{ \"$HOME/.bashrs/bashrs\" brs_compile \"$@\"; local code=$?; [ \"$code\" -eq {RELOAD_EXIT_CODE} ] && shell_new; return \"$code\"; }}"
        ));
        has("brs_sourcefile() { \"$HOME/.bashrs/bashrs\" brs_sourcefile \"$@\"; }");
        has("brs_configure() { \"$HOME/.bashrs/bashrs\" brs_configure \"$@\"; }");
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
        let script = wrappers(Shell::Bash);
        let content = flat(&script);
        // Bash's file is zsh's too: the session commands start whichever of the two runs them.
        assert!(content.contains(r#"shell_new() { exec "${ZSH_NAME:-bash}"; }"#), "shell_new missing");
        assert!(content.contains(r#"shell_bare() { _BASHRS_BARE=1 exec "${ZSH_NAME:-bash}"; }"#), "shell_bare missing");
        // The one-shot guard: consume the flag AND return, so a bare session starts with a clean
        // environment and the very next shell arms bashrs again.
        assert!(
            script.contains("[ -n \"$_BASHRS_BARE\" ] && { unset _BASHRS_BARE ; return 0; }"),
            "the flag-consuming bare guard must sit at the sourcefile's top"
        );
        assert!(script.contains(r#"bind '"\en": "shell_new\n"'"#), "ALT+N keybind missing");
        assert!(script.contains(r#"bind '"\e\C-n": "shell_bare\n"'"#), "CTRL+ALT+N keybind missing");
        // Every bind carries a comment naming the chord as a keyboard labels it, plus what it
        // runs — sourced from the bound command's own description, so it cannot drift.
        assert!(script.contains("# Alt+N: Start a fresh shell session"), "bind comment missing");
        assert!(script.contains("# Ctrl+Alt+N: Fresh shell WITHOUT bashrs"), "chord comment missing");
        // And the comments sit in one column, like every other block's — the bind texts vary
        // in length, so equality here means the table alignment actually ran.
        let comment_columns: std::collections::BTreeSet<usize> = script
            .lines()
            .filter(|line| line.trim_start().starts_with("bind "))
            .filter_map(|line| line.find("  # "))
            .collect();
        assert_eq!(comment_columns.len(), 1, "bind comments are not aligned: {comment_columns:?}");
        assert!(script.contains(r#"bind '"\eh": "brs_sourcefile\n"'"#), "ALT+H keybind missing");
        assert!(script.contains(r#"bind '"\ew": "brs_configure\n"'"#), "ALT+W keybind missing");
        assert!(script.contains(r#"bind '"\eq": "brs_compile\n"'"#), "ALT+Q keybind missing");
        // Spacing-agnostic: the gap before `then` belongs to the aligner (keybinds' own test
        // asserts the column), this one only cares that the ALT+L chain is present.
        assert!(script.contains(r#"if [[ $__bashrs_desktops == *" cinnamon "* ]];"#), "ALT+L desktop-restart missing");
        assert!(script.contains("if [ -n \"$BASH_VERSION\" ]; then"), "keybinds should be bash-guarded");
    }

    /// A keybind pointing at a command that does not exist fails *silently*: readline binds the
    /// text happily, the description lookup misses and degrades to echoing the command back, and
    /// nothing complains until someone presses the key and gets `command not found`. Renaming a
    /// bound function without updating `keybinds::bindings` is exactly how that happens, so the
    /// two are tied together here rather than left to be discovered by hand.
    #[test]
    fn every_keybind_targets_a_function_the_sourcefile_defines() {
        let script = wrappers(Shell::Bash);
        for (key, bound) in keybinds::bindings() {
            // A bind may carry arguments; only the command itself has to resolve.
            let command = bound.split_once(' ').map_or(*bound, |(name, _)| name);
            let definition = format!("{command}() {{");
            assert!(
                script.lines().any(|line| flat(line).starts_with(&definition)),
                "{key} binds `{command}`, which this sourcefile never defines — \
                 that key would fail only at the moment someone pressed it"
            );
            // The other half of the same failure: the description lookup is keyed by command
            // name, and on a miss it degrades to printing the command back. That reads as a
            // comment, so it survives review — assert the lookup actually resolved.
            let bind_line = script
                .lines()
                .find(|line| line.contains(&format!("\"{key}\":")))
                .unwrap_or_else(|| panic!("{key} is in bindings() but emitted no bind line"));
            let (_, description) = bind_line.rsplit_once(": ").expect("every bind line is commented");
            assert_ne!(
                description.trim(),
                command,
                "{key}'s comment is just `{command}` echoed back — the description lookup missed"
            );
        }
    }

    /// The `cd` commands and `shell_def` are pure shell — a child process can't move its
    /// parent, and only the calling shell knows its own aliases/functions/variables — so what
    /// the sourcefile defines IS the implementation, pinned here. `dotbashrs_cd`'s off-prefix
    /// name is deliberate: the dot names the directory (~/.bashrs), and the clap-side naming
    /// test can't see shell-body commands, so this is where the spelling is held.
    #[test]
    fn the_cd_and_define_commands_are_inline_shell() {
        let script = wrappers(Shell::Bash);
        let content = flat(&script);
        // The project root is baked at generation time, shell-quoted — no subprocess at `cd`
        // time. Asserted through the same quote + env the generator uses, so this test is
        // location-independent.
        let baked = format!(
            "brs_cd() {{ cd -- {}; }}",
            crate::support::shell_quote::quote(env!("CARGO_MANIFEST_DIR"))
        );
        assert!(content.contains(&baked), "brs_cd must cd to the baked project root: {baked}");
        assert!(
            !script.contains("{PROJECT_ROOT}"),
            "the placeholder must never survive into the emitted script"
        );
        assert!(
            content.contains(r#"dotbashrs_cd() { cd -- "$HOME/.bashrs"; }"#),
            "dotbashrs_cd must cd to the installation directory"
        );
        // The `cd` builtin override: the real builtin (every native behavior kept), and on
        // success the landing directory's contents — with GNU ls's own shell-escape quoting,
        // so a problematic name pastes back like lll's do. Its section header proves the
        // category registered end-to-end.
        assert!(script.contains("\n# overrides\n"), "the overrides section is missing");
        assert!(
            content.contains(r#"cd() { builtin cd "$@" && ls -AF --color=always --group-directories-first --quoting-style=shell-escape; }"#),
            "the cd override must run the builtin, then list escaped on success"
        );
        // shell_def's wrapper pipes the four shell-only probes into the binary (NUL-delimited),
        // where the ranking/labeling logic lives in tested Rust — the shell half is only what a
        // child process cannot see. Every probe silenced and `--`-terminated.
        let def = script
            .lines()
            .find(|line| line.trim_start().starts_with("shell_def()"))
            .expect("shell_def must be emitted");
        for needed in [
            r#""$(alias -- "$__n")""#,
            r#""$(declare -f -- "$__n")""#,
            r#""$(declare -p -- "$__n")""#,
            r#""$(type -at -- "$__n")""#,
            // One printf frames all four fields; substitutions never short-circuit.
            r"printf '%s\0%s\0%s\0%s\0'",
            // ONE stderr discard for the whole probe loop, sitting inside the pipeline
            // segment — the binary's own stderr (the not-found reports) stays audible.
            r#"done 2>/dev/null | "$HOME/.bashrs/bashrs" shell_def "$@""#,
        ] {
            assert!(def.contains(needed), "shell_def's wrapper is missing `{needed}`: {def}");
        }
    }

    /// Every category block is a table — name, `{`-opened body, `#` comment — so within each
    /// section the `{` openers share one column. Checked per section against the real labels
    /// (each category aligns independently; a global column would stretch every short section
    /// to the longest name in the file).
    #[test]
    fn every_category_aligns_its_openers_into_a_column() {
        let script = wrappers(Shell::Bash);
        let mut sections_checked = 0_usize;
        // The category sections, plus the stainless block — not a category, but the same
        // three-column table and the same alignment promise.
        let labels: Vec<String> = category_commands()
            .iter()
            .map(|(label, _, _)| (*label).to_string())
            .chain(std::iter::once("stainless comfy repos".to_string()))
            .collect();
        for label in labels {
            let header = format!("# {label}");
            let braces: std::collections::BTreeSet<usize> = script
                .lines()
                .skip_while(|line| *line != header)
                .skip(1)
                .take_while(|line| !line.starts_with("# ") && !line.is_empty())
                .filter(|line| line.contains("() "))
                .filter_map(|line| line.find('{'))
                .collect();
            if braces.is_empty() {
                continue; // a category can be momentarily wrapper-less (overrides once was)
            }
            assert_eq!(braces.len(), 1, "`{{` openers in `# {label}` are not one column");
            sections_checked += 1;
        }
        assert!(sections_checked >= 15, "sections actually checked: {sections_checked}");
    }

    /// The reason `align_columns` runs with `reasonable_spacing`: `shell_def`'s probe loop is
    /// four times as wide as any neighbour, and without it the whole shell section's comment
    /// column was dragged out to meet it. With it, the outlier keeps its head aligned (the
    /// brace column — asserted for every section elsewhere), goes minimal from its body on,
    /// and the ordinary rows' comments snap back to their own width.
    #[test]
    fn the_shell_def_monster_does_not_stretch_its_category() {
        let script = wrappers(Shell::Bash);
        let shell: Vec<&str> = script
            .lines()
            .skip_while(|line| *line != "# shell")
            .skip(1)
            .take_while(|line| !line.starts_with("# ") && !line.is_empty())
            .filter(|line| line.contains("() "))
            .collect();
        let (monster, ordinary): (Vec<&str>, Vec<&str>) =
            shell.iter().partition(|line| line.starts_with("shell_def()"));
        assert_eq!(monster.len(), 1, "shell_def is emitted once: {shell:?}");

        // The ordinary rows share a comment column of their own natural width…
        let comment_columns: std::collections::BTreeSet<usize> =
            ordinary.iter().filter_map(|line| line.find("  # ")).collect();
        assert_eq!(comment_columns.len(), 1, "ordinary rows align among themselves: {ordinary:?}");
        let column = *comment_columns.iter().next().expect("one column");
        assert!(column < 80, "…no longer stretched by shell_def (was ~230): {column}");

        // …while the monster's tail is minimal: exactly the two-space join before its comment.
        assert!(
            monster[0].contains(r#""$@"; }  # "#) && !monster[0].contains(r#""$@"; }   "#),
            "shell_def's own comment sits at minimal spacing: {}",
            monster[0]
        );
    }

    /// The `autokey_*` commands take real paths and complete like any other command — flags
    /// from the binary, filenames from the PWD. The store's contents are deliberately NOT
    /// completion candidates: they live outside the PWD (the full-path listing is
    /// `autokey_list_bashrs_profiles`'s job, and a bare `autokey_apply` already reaches the
    /// whole store), so offering them would only pollute the completion space.
    #[test]
    fn profile_commands_get_no_special_completion() {
        let script = wrappers(Shell::Bash);
        assert!(!script.contains("_bashrs_autokey_complete"), "the store completer was removed");
        assert!(
            !script.contains("user-data/sequencer/\"*.toml"),
            "nothing in the sourcefile should glob the profile store"
        );
    }

    #[test]
    fn wrappers_include_environment_settings() {
        let script = wrappers(Shell::Bash);
        assert!(script.contains("\n# environment\n"), "environment section missing");
        assert!(script.contains("export GREP_COLORS='mt=7;31'"), "GREP_COLORS export missing");
        assert!(script.contains("export PS1=") && script.contains("(UTC)"), "PS1 export missing");
    }

    #[test]
    fn the_interactivity_guard_tops_the_file_ahead_of_everything() {
        // Non-interactive shells must inherit nothing at all — most of all not the `python3`
        // function, which would reroute external scripts onto the bundled interpreter.
        let script = wrappers(Shell::Bash);
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
        let script = wrappers(Shell::Bash);
        let content = flat(&script);
        assert!(content.contains("..() { cd .. \"$@\"; }"), "`..` shell function missing:\n{script}");
        let filesystem = script.split("\n# ").find(|s| s.starts_with("filesystem")).expect("filesystem section");
        assert!(filesystem.contains("..()") && filesystem.contains("# Hop one directory up"),
            "the shell function belongs to its category, doc comment included: {filesystem}");
        assert!(!script.contains("..() { \"$HOME/.bashrs/bashrs\""),
            "a shell-body command must not call the binary");
    }

    #[test]
    fn wrappers_end_with_the_load_greeting() {
        assert!(wrappers(Shell::Bash).contains("gecho \"Loaded Bashrs!"), "load greeting missing");
    }

    #[test]
    fn wrappers_group_commands_under_a_category_comment() {
        let script = wrappers(Shell::Bash);
        // A blank line then a comment introduces each category's block.
        assert!(script.contains("\n# bashrs\n"));
        assert!(script.contains("\n# media\n"));
    }

    #[test]
    fn wrappers_exclude_the_internal_commands() {
        assert!(!wrappers(Shell::Bash).contains("generate() {"));
        assert!(!wrappers(Shell::Bash).contains("complete-flags() {"));
        assert!(!wrappers(Shell::Bash).contains("install-shell() {"));
        assert!(!wrappers(Shell::Bash).contains("install-stainless() {"));
    }

    #[test]
    fn wrappers_include_the_stainless_aliases() {
        // The alias is emitted whether or not the repo has been cloned yet; only the trailing
        // `--help` comment varies with the environment, so assert just the alias line's stable
        // head — collapsed, since the block's `{` gaps belong to the aligner.
        let content = flat(&wrappers(Shell::Bash));
        assert!(content.contains("ai() { python3 \"$HOME/.bashrs/stainless_comfy/"), "stainless `ai` alias missing");
        // …and the same clone's auxiliary entry point: `ai_audit_self`, a `-m` module run from the tool's dir.
        assert!(
            content.contains("ai_audit_self() { (cd \"$HOME/.bashrs/stainless_comfy/contAInerized/dockerized_claude_code\" >/dev/null && python3 -m launch.audit \"$@\"); }"),
            "stainless `ai_audit_self` aux alias missing"
        );
        // …and the same clone's `quick_question.py` script family: `q` bare, `q3` with a pinned flag.
        assert!(
            content.contains("q() { python3 \"$HOME/.bashrs/stainless_comfy/contAInerized/dockerized_claude_code/quick_question.py\" \"$@\"; }"),
            "stainless `q` script alias missing"
        );
        assert!(
            content.contains("q3() { python3 \"$HOME/.bashrs/stainless_comfy/contAInerized/dockerized_claude_code/quick_question.py\" --research \"$@\"; }"),
            "stainless `q3` script alias missing"
        );
    }

    #[test]
    fn wrappers_put_the_bundled_tools_on_path() {
        let script = wrappers(Shell::Bash);
        assert!(script.contains("\n# bundled tools\n"), "section missing");
        assert!(script.contains("PATH=\"$HOME/.bashrs/tools/bin:$PATH\""), "shim-dir prepend missing");
    }

    #[test]
    fn wrappers_leave_no_variable_behind_in_the_shell() {
        // The path is inlined per function, so sourcing defines functions only —
        // nothing lingers in the user's environment afterward.
        assert!(!wrappers(Shell::Bash).contains("__bashrs_bin"));
    }

    #[test]
    fn wrappers_bail_early_when_the_binary_is_missing() {
        // `return 0` explicitly: a bare `return` would forward the failed test's status 1 to
        // whatever sourced the file, and "bashrs isn't installed" is a no-op, not an error.
        assert!(wrappers(Shell::Bash).contains("[ -f \"$HOME/.bashrs/bashrs\" ] || return 0"));
    }

    #[test]
    fn file_opens_with_a_bash_shebang() {
        assert!(wrappers(Shell::Bash).starts_with("#!/usr/bin/env bash\n"));
    }

    #[test]
    fn wrappers_carry_each_command_description_as_an_inline_comment() {
        // The command's clap `about` trails its wrapper as a `# ...` comment. Comments are
        // aligned into a column, so the wrapper and its comment are no longer adjacent —
        // assert each is present rather than expecting them side by side.
        let script = wrappers(Shell::Bash);
        assert!(flat(&script).contains("media_metadata() { \"$HOME/.bashrs/bashrs\" media_metadata \"$@\"; }"));
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
