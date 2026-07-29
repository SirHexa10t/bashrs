//! Non-Rust ("stainless" — Rust-resistant) companion repos, cloned under `~/.bashrs/stainless_comfy`
//! and exposed as plain shell aliases in `sourcefile.sh`. Data-driven, like [`crate::conf::keybinds`]: a
//! `const` table plus functions that turn it into shell lines — no macro, no clap command.
//!
//! Two consumers of the one table:
//! - [`sync`] — run by the hidden `install-stainless` command from `COMPILE.sh`:
//!   clone or update each repo.
//! - [`aliases`] — run by the main binary's [`crate::cli`] generator: emit one alias per repo, asking
//!   the (freshly cloned) tool itself for its `--help` description to use as the inline comment.
//!
//! `sync` runs first in `COMPILE.sh`, so the clones exist when `aliases` probes them. The clone is
//! the only shared artifact — nothing copies the tool's own `--help` into a side file to go stale.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One of your own non-Rust repos (a future external kind would be a sibling struct in this table).
struct Comfy {
    /// git URL to clone.
    repo: &'static str,
    /// Shell function name to expose.
    alias: &'static str,
    /// Launcher command — a bare name resolves like any command at use time (`python3` hits the
    /// sourcefile's tools fallback, so it works even with no system python).
    run: &'static str,
    /// Executable, relative to the clone root.
    exe: &'static str,
    /// PyPI packages the tool needs at runtime — installed into the bundled python environment
    /// by [`sync`], via [`super::python`].
    python_deps: &'static [&'static str],
    /// Extra entry points into this SAME clone — secondary aliases run by the same interpreter,
    /// from the main executable's directory (so a `-m package.module` finds its package).
    aux: &'static [Aux],
    /// Sibling runnable scripts in the same clone, each exposed as flag-variant aliases — run by
    /// absolute path with no `cd`, exactly like the main alias.
    scripts: &'static [Script],
}

/// A secondary alias for a [`Comfy`]'s clone: the same tool, a different entry point. Run by the
/// repo's own `run` interpreter, from the directory holding its `exe` — so `-m` module invocations
/// resolve — inside a subshell, leaving the caller's working directory untouched.
struct Aux {
    /// Shell function name to expose.
    alias: &'static str,
    /// Arguments to the repo's `run` interpreter (e.g. `-m launch.audit`); `"$@"` is appended.
    args: &'static str,
}

/// A runnable script in a [`Comfy`]'s clone, exposed as one or more flag-variant aliases. Run by
/// absolute path like the main alias — no `cd`, so it behaves as if invoked directly: the
/// interpreter puts the script's own directory on the import path, and the caller's working
/// directory is preserved. Like the `g`/`g<N>` idea — one file, several fixed-flag verbs.
struct Script {
    /// Runnable file, relative to the clone root.
    exe: &'static str,
    /// `(alias, extra args)` pairs — each becomes `alias() { <run> "<exe>" <args> "$@"; }`.
    variants: &'static [(&'static str, &'static str)],
}

/// The companion repos. Named `STAINLESS` (the general "non-Rust" bucket) though every entry is a
/// [`Comfy`] (your own) for now.
const STAINLESS: &[Comfy] = &[Comfy {
    repo: "https://github.com/SirHexa10t/contAInerized",
    alias: "ai",
    run: "python3",
    exe: "dockerized_claude_code/run.py",
    // The launcher's runtime deps, per its own install_dependencies.sh.
    python_deps: &["prompt_toolkit", "python-dotenv", "rich"],
    // A second entry point into the same clone: `ai_audit_self` runs the `launch.audit` module.
    aux: &[Aux { alias: "ai_audit_self", args: "-m launch.audit" }],
    // `quick_question.py` (beside run.py): `q` asks; `q2`/`q3` pin its --explain / --research modes.
    scripts: &[Script {
        exe: "dockerized_claude_code/quick_question.py",
        variants: &[("q", ""), ("q2", "--explain"), ("q3", "--research")],
    }],
}];

/// Clone root as the *generated shell* spells it — the sanctioned `$HOME`-relative literal, kept
/// for the alias lines the shell expands at use time (the Rust-side spelling of this same
/// directory is [`crate::conf::clones_dir`]). Named the "comfy" area (the dir you picked) even
/// though the module is the general stainless one.
const CLONE_BASE: &str = ".bashrs/stainless_comfy";

/// `https://…/contAInerized(.git)(/)` → `contAInerized`.
fn repo_name(repo: &str) -> &str {
    repo.trim_end_matches('/').rsplit('/').next().unwrap_or(repo).trim_end_matches(".git")
}

fn clone_dir(comfy: &Comfy) -> PathBuf {
    crate::conf::clones_dir().join(repo_name(comfy.repo))
}

/// A clone-relative path, `$HOME`-relative and shell-ready (the shell expands `$HOME`) — the form
/// used verbatim in aliases and in the `--help` probe.
fn clone_path_shell(comfy: &Comfy, rel: &str) -> String {
    format!("$HOME/{CLONE_BASE}/{}/{}", repo_name(comfy.repo), rel)
}

/// The `$HOME`-relative executable path — the main alias's target (and the `--help` probe's).
fn exe_shell(comfy: &Comfy) -> String {
    clone_path_shell(comfy, comfy.exe)
}

/// The `$HOME`-relative directory holding the executable — where [`Aux`] commands `cd` before
/// running, so a `-m package.module` finds its package. Falls back to the clone root for an `exe`
/// that sits there directly.
fn exe_dir_shell(comfy: &Comfy) -> String {
    let root = format!("$HOME/{CLONE_BASE}/{}", repo_name(comfy.repo));
    match Path::new(comfy.exe).parent().and_then(Path::to_str).filter(|dir| !dir.is_empty()) {
        Some(dir) => format!("{root}/{dir}"),
        None => root,
    }
}

/// SIDE EFFECTS — `install-stainless` runs this at compile time: clone each repo if
/// missing, else best-effort `git pull`. Never aborts; a git failure just warns and the alias is
/// emitted anyway (pointing at its expected path). The `--help` description is read later, by
/// [`aliases`]. A `pins` entry (a Carstay.toml revision, in `--use-stable-carstay` mode) puts that clone AT
/// the recorded commit — fetched explicitly and hard-reset, which discards nothing of the user's:
/// these clones are read-only mirrors.
pub fn sync(pins: &[(String, String)]) {
    for comfy in STAINLESS {
        let dir = clone_dir(comfy);
        let name = repo_name(comfy.repo);
        let existed = dir.exists();
        if !existed {
            if let Some(parent) = dir.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            git(Command::new("git").args(["clone", "--depth", "1", comfy.repo]).arg(&dir), comfy.repo);
        }
        let pin = pins.iter().find(|(pinned, _)| pinned == name).map(|(_, rev)| rev.as_str());
        match pin {
            Some(rev) if dir.exists() => {
                eprintln!("stainless: {name} pinned to the recorded revision {rev}");
                // GitHub serves unadvertised commits by SHA, so a shallow fetch of the exact
                // revision works even after upstream history moved (or was rewritten).
                if git(Command::new("git").arg("-C").arg(&dir).args(["fetch", "--depth", "1", "origin", rev]), comfy.repo) {
                    git(Command::new("git").arg("-C").arg(&dir).args(["reset", "--hard", rev]), comfy.repo);
                }
            }
            None if existed => {
                // A failed fast-forward usually means upstream history was rewritten (a
                // force-pushed main). Deliberately NOT auto-reset: the remedy is named and the
                // decision stays in the user's hands — delete the clone and it re-fetches fresh.
                let pulled = Command::new("git").arg("-C").arg(&dir).args(["pull", "--ff-only"]).status();
                if !matches!(pulled, Ok(status) if status.success()) {
                    eprintln!(
                        "stainless: could not fast-forward {name} (upstream history rewritten?) — delete {} and re-run COMPILE.sh to re-clone it fresh",
                        dir.display()
                    );
                }
            }
            _ => {} // fresh clone with no pin — already at the tip
        }
        // The repo's runtime python packages, into the bundled environment (best-effort, like the
        // clone — `install` explains itself when the environment or uv is missing).
        if !super::python::install(comfy.python_deps) {
            eprintln!("stainless: {}'s python packages may be missing", repo_name(comfy.repo));
        }
    }
}

/// Run a prepared `git` command, warning (never failing the compile) if it doesn't succeed;
/// reports the verdict for steps that chain (a pinned reset only makes sense after its fetch).
fn git(cmd: &mut Command, repo: &str) -> bool {
    let ok = matches!(cmd.status(), Ok(status) if status.success());
    if !ok {
        eprintln!("stainless: git failed for {repo}; its alias may be stale or point at a missing path");
    }
    ok
}

/// Each companion clone's current revision (`git rev-parse HEAD`), in table order — `None` when
/// the clone is missing or unreadable. Feeds [`crate::drivers::carstay`]'s manifest.
pub fn clone_revisions() -> Vec<(&'static str, Option<String>)> {
    STAINLESS
        .iter()
        .map(|comfy| {
            let out = Command::new("git")
                .arg("-C")
                .arg(clone_dir(comfy))
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|out| out.status.success());
            let rev = out
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .filter(|rev| !rev.is_empty());
            (repo_name(comfy.repo), rev)
        })
        .collect()
}

/// Width handed to `--help` probes via `COLUMNS`, so argparse (textwrap) lays a one-paragraph
/// description on a single line instead of wrapping it. Wrapping can break at a hyphen, which
/// [`extract_about`] would then rejoin as a spurious space (`~/.claude-agents` → `~/.claude-
/// agents`); an un-wrapped line has no such ambiguity. Far wider than any real description, so the
/// result is the same on any terminal.
const PROBE_COLUMNS: u32 = 10_000;

/// Run a `--help` probe `script` through bash (so `$HOME` expands and a `cd` is possible) and
/// return its full text. One probe serves both consumers — [`extract_about`] for the alias's
/// inline comment and [`extract_flags`] for its TAB completion — so each tool starts (a python
/// interpreter, at compile time) once, not twice. Best-effort: a spawn failure or a non-zero exit
/// yields `None` (no comment, no completion).
fn probe_help(script: &str) -> Option<String> {
    let out = Command::new("bash").arg("-c").arg(script).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Probe the main launcher or a sibling [`Script`]: `<run> "<clone-path>" --help`, run by absolute
/// path (no `cd`), `exe` clone-relative. `run` goes through [`crate::tools::resolve`], mirroring the
/// sourcefile's PATH shim, so the probe works even when the interpreter is only bundled.
fn fetch_help(comfy: &Comfy, exe: &str) -> Option<String> {
    let run = crate::tools::resolve(comfy.run);
    probe_help(&format!("COLUMNS={PROBE_COLUMNS} \"{}\" \"{}\" --help 2>&1", run.to_string_lossy(), clone_path_shell(comfy, exe)))
}

/// Probe an [`Aux`] exactly as it runs — `cd` into the exe's directory so a `-m module` resolves,
/// then `<run> <args> --help`. `extract_about` keeps just the first description line, so a
/// multi-line `--help` (like `launch.audit`'s) still yields a one-line comment.
fn fetch_help_aux(comfy: &Comfy, aux: &Aux) -> Option<String> {
    let run = crate::tools::resolve(comfy.run);
    probe_help(&format!("cd \"{}\" && COLUMNS={PROBE_COLUMNS} \"{}\" {} --help 2>&1", exe_dir_shell(comfy), run.to_string_lossy(), aux.args))
}

/// Pure: the one-line description from `--help` text — the first *paragraph* after the (possibly
/// multi-line) `usage:` block: consecutive non-empty lines joined with spaces. argparse's default
/// formatter word-wraps a one-paragraph description to the probe's width, so joining the run
/// recovers the whole sentence (otherwise it'd cut at the first wrapped line); a
/// `RawDescriptionHelpFormatter` help whose author put a blank after a summary line still yields
/// just that summary. Falls back to the first paragraph of the whole text when there's no `usage:`.
fn extract_about(help: &str) -> Option<String> {
    let lines: Vec<&str> = help.lines().map(str::trim).collect();
    // The description starts after the usage block (everything up to the first blank line); with no
    // blank, from the top.
    let start = lines.iter().position(|line| line.is_empty()).map_or(0, |blank| blank + 1);
    let paragraph: Vec<&str> = lines[start..]
        .iter()
        .skip_while(|line| line.is_empty() || line.to_ascii_lowercase().starts_with("usage"))
        .take_while(|line| !line.is_empty())
        .copied()
        .collect();
    (!paragraph.is_empty()).then(|| paragraph.join(" "))
}

/// Pure: the flags a `--help` text advertises, in appearance order. An option line (argparse
/// style) is an indented line starting with `-`; its flag tokens sit before the two-space gap
/// that separates them from the description (`-n N, --count N   how many`). Metavars are skipped
/// (no leading `-`), a `--foo=BAR` spelling keeps only `--foo`, and duplicates collapse. Usage
/// lines never start with `-` after trimming, so their `[-h]` noise never enters. The probes run
/// under a huge `COLUMNS`, so descriptions don't wrap into lines that could mimic option lines.
fn extract_flags(help: &str) -> Vec<String> {
    let mut flags: Vec<String> = Vec::new();
    for line in help.lines() {
        let trimmed = line.trim_start();
        if !line.starts_with(char::is_whitespace) || !trimmed.starts_with('-') {
            continue;
        }
        // Only the flag column: everything before the first 2-space run (the description gap).
        let column = trimmed.split("  ").next().unwrap_or(trimmed);
        for token in column.split([',', ' ']) {
            let flag = token.split('=').next().unwrap_or(token);
            let valid = flag.len() > 1
                && flag.starts_with('-')
                && flag.chars().skip(1).all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if valid && !flags.iter().any(|f| f == flag) {
                flags.push(flag.to_string());
            }
        }
    }
    flags
}

/// The main binary's generator runs this. One alias per repo entry point; for a repo that's
/// actually been cloned, each entry point's own `--help` is probed ONCE, live, and read twice:
/// the description becomes the alias's inline comment, the flag list its TAB completion (the
/// second element — `(alias, space-joined flags)` rows the sourcefile's completion block renders;
/// a flagless or unprobed alias contributes no row). A [`Script`] variant's pinned flags are
/// dropped from its own row, like the CLI's `HIDDEN_PINNED`: `q2` never offers the `--explain`
/// it already passes. The clone check keeps it from spawning anything before the first `sync`
/// (or during tests).
pub(crate) fn aliases() -> (String, Vec<(String, String)>) {
    let mut lines = String::new();
    let mut completions: Vec<(String, String)> = Vec::new();
    let mut complete = |alias: &str, flags: &[String], pinned: &str| {
        let offered: Vec<&str> = flags
            .iter()
            .map(String::as_str)
            .filter(|flag| !pinned.split_whitespace().any(|p| p == *flag))
            .collect();
        if !offered.is_empty() {
            completions.push((alias.to_string(), offered.join(" ")));
        }
    };
    for comfy in STAINLESS {
        // One clone check gates every live `--help` probe (nothing spawns before the first sync).
        let exists = clone_dir(comfy).exists();
        let help = exists.then(|| fetch_help(comfy, comfy.exe)).flatten();
        let about = help.as_deref().and_then(extract_about);
        lines.push_str(&alias_line(comfy, about.as_deref()));
        complete(comfy.alias, &help.as_deref().map(extract_flags).unwrap_or_default(), "");
        for aux in comfy.aux {
            let help = exists.then(|| fetch_help_aux(comfy, aux)).flatten();
            let about = help.as_deref().and_then(extract_about);
            lines.push_str(&aux_line(comfy, aux, about.as_deref()));
            complete(aux.alias, &help.as_deref().map(extract_flags).unwrap_or_default(), "");
        }
        for script in comfy.scripts {
            let help = exists.then(|| fetch_help(comfy, script.exe)).flatten();
            let about = help.as_deref().and_then(extract_about);
            lines.push_str(&script_lines(comfy, script, about.as_deref()));
            let flags = help.as_deref().map(extract_flags).unwrap_or_default();
            for (alias, pinned) in script.variants {
                complete(alias, &flags, pinned);
            }
        }
    }
    (lines, completions)
}

/// Pure: one alias definition line (`about` becomes an inline `#` comment when present).
fn alias_line(comfy: &Comfy, about: Option<&str>) -> String {
    let comment = about.map(|about| format!("  # {about}")).unwrap_or_default();
    format!("{}() {{ {} \"{}\" \"$@\"; }}{comment}\n", comfy.alias, comfy.run, exe_shell(comfy))
}

/// Pure: one auxiliary alias line (`about` becomes an inline `#` comment when present) — `cd` to
/// the executable's directory (in a subshell, so the caller's cwd is unchanged) and run the repo's
/// interpreter with the aux's own arguments. The `cd`'s own stdout is discarded — a customised `cd`
/// or a `CDPATH` hit echoes the directory, which would otherwise pollute the tool's output — while
/// its errors still reach stderr and `&&` still gates the run on a successful `cd`.
fn aux_line(comfy: &Comfy, aux: &Aux, about: Option<&str>) -> String {
    let comment = about.map(|about| format!("  # {about}")).unwrap_or_default();
    format!("{}() {{ (cd \"{}\" >/dev/null && {} {} \"$@\"); }}{comment}\n", aux.alias, exe_dir_shell(comfy), comfy.run, aux.args)
}

/// Pure: the alias lines for one [`Script`] — one per flag variant, each running the script by
/// absolute path (no `cd`) with its fixed flags then `"$@"`, like the main alias. An empty-flag
/// variant emits no stray space. The script's one probed `--help` line (`about`) becomes an inline
/// comment on every variant, suffixed with the pinned flag so `q2`/`q3` read distinctly from `q`.
fn script_lines(comfy: &Comfy, script: &Script, about: Option<&str>) -> String {
    let path = clone_path_shell(comfy, script.exe);
    script
        .variants
        .iter()
        .map(|(alias, args)| {
            let flags = if args.is_empty() { String::new() } else { format!(" {args}") };
            let comment = match about {
                None => String::new(),
                Some(about) if args.is_empty() => format!("  # {about}"),
                Some(about) => format!("  # {about} ({args})"),
            };
            format!("{alias}() {{ {} \"{path}\"{flags} \"$@\"; }}{comment}\n", comfy.run)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &Comfy = &Comfy {
        repo: "https://github.com/x/contAInerized.git",
        alias: "ai",
        run: "python3",
        exe: "dockerized_claude_code/run.py",
        python_deps: &[],
        aux: &[Aux { alias: "ai_audit_self", args: "-m launch.audit" }],
        scripts: &[Script {
            exe: "dockerized_claude_code/quick_question.py",
            variants: &[("q", ""), ("q2", "--explain")],
        }],
    };

    #[test]
    fn repo_name_strips_host_git_suffix_and_trailing_slash() {
        assert_eq!(repo_name("https://github.com/x/contAInerized"), "contAInerized");
        assert_eq!(repo_name("https://github.com/x/contAInerized.git"), "contAInerized");
        assert_eq!(repo_name("https://github.com/x/repo/"), "repo");
    }

    #[test]
    fn alias_line_wires_run_and_exe_with_an_optional_comment() {
        assert_eq!(
            alias_line(SAMPLE, None),
            "ai() { python3 \
             \"$HOME/.bashrs/stainless_comfy/contAInerized/dockerized_claude_code/run.py\" \"$@\"; }\n"
        );
        assert!(alias_line(SAMPLE, Some("Launch an agent")).contains("; }  # Launch an agent\n"));
    }

    #[test]
    fn aux_line_cds_into_the_exe_dir_and_runs_the_module_in_a_subshell() {
        // The subshell `(cd … && …)` keeps the caller's cwd; the cd target is the exe's directory,
        // so `python3 -m launch.audit` can import the `launch` package that lives beside run.py.
        // The probed `--help` first line rides along as an inline comment.
        assert_eq!(
            aux_line(SAMPLE, &SAMPLE.aux[0], Some("Audit the launcher's state")),
            "ai_audit_self() { (cd \
             \"$HOME/.bashrs/stainless_comfy/contAInerized/dockerized_claude_code\" >/dev/null \
             && python3 -m launch.audit \"$@\"); }  # Audit the launcher's state\n"
        );
        // No probe (clone absent) → no comment.
        assert!(
            aux_line(SAMPLE, &SAMPLE.aux[0], None).ends_with("\"$@\"); }\n"),
            "no about → no trailing comment"
        );
    }

    #[test]
    fn script_lines_run_by_absolute_path_with_fixed_flags_and_no_cd() {
        // Each variant is a bare alias like the main one (no `cd`): the script by absolute path,
        // its fixed flags, then "$@". The probed `--help` line is an inline comment, the pinned
        // flag named so variants differ; an empty-flag variant leaves no stray space before "$@".
        let out = script_lines(SAMPLE, &SAMPLE.scripts[0], Some("Ask a quick question"));
        assert!(
            out.contains("q() { python3 \"$HOME/.bashrs/stainless_comfy/contAInerized/dockerized_claude_code/quick_question.py\" \"$@\"; }  # Ask a quick question\n"),
            "bare variant + comment: {out}"
        );
        assert!(
            out.contains("q2() { python3 \"$HOME/.bashrs/stainless_comfy/contAInerized/dockerized_claude_code/quick_question.py\" --explain \"$@\"; }  # Ask a quick question (--explain)\n"),
            "flag variant names the pinned flag: {out}"
        );
        // No probe (clone absent) → no comment, and still no stray space on the bare variant.
        assert!(
            script_lines(SAMPLE, &SAMPLE.scripts[0], None).contains("q() { python3 \"$HOME/.bashrs/stainless_comfy/contAInerized/dockerized_claude_code/quick_question.py\" \"$@\"; }\n"),
            "no about → no comment"
        );
    }

    #[test]
    fn exe_dir_shell_is_the_clone_root_when_the_exe_sits_there() {
        const FLAT: &Comfy = &Comfy {
            repo: "https://github.com/x/tool.git",
            alias: "t",
            run: "python3",
            exe: "main.py", // no subdirectory
            python_deps: &[],
            aux: &[],
            scripts: &[],
        };
        assert_eq!(exe_dir_shell(FLAT), "$HOME/.bashrs/stainless_comfy/tool");
    }

    #[test]
    fn extract_about_takes_the_description_after_the_usage_block() {
        // A multi-line `usage:` block precedes the description; the wrapped usage line is skipped.
        let help = "usage: run.py [-h] [--dry-run]\n              [target]\n\nLaunch a containerized agent.\n\npositional arguments:\n  target\n";
        assert_eq!(extract_about(help).as_deref(), Some("Launch a containerized agent."));
    }

    #[test]
    fn extract_about_falls_back_or_gives_none() {
        assert_eq!(extract_about("A tiny tool.\n").as_deref(), Some("A tiny tool."));
        assert_eq!(extract_about(""), None);
    }

    #[test]
    fn extract_about_joins_a_width_wrapped_one_paragraph_description() {
        // argparse's default formatter word-wraps a single-paragraph description to the probe's
        // width; the whole paragraph (to the blank before `options:`) is rejoined, not cut at the
        // first wrapped line — the `q` regression.
        let help = "usage: q [-h]\n\nAsk one direct question, answered in one shot. Put your\nprompt in quotes. If you need files, use the communal dir\n\noptions:\n  -h, --help\n";
        assert_eq!(
            extract_about(help).as_deref(),
            Some("Ask one direct question, answered in one shot. Put your prompt in quotes. If you need files, use the communal dir"),
        );
    }

    #[test]
    fn extract_flags_reads_argparse_option_lines_only() {
        // Short+long pairs split on the comma; metavars (no dash) drop; `--foo=BAR` keeps the
        // flag; the usage line and prose (unindented, or not dash-led) contribute nothing; and
        // a flag repeated across lines lands once.
        let help = "usage: q [-h] [--explain] [-n N]\n\n\
                    Ask one direct question. Try --explain for more — this prose line is not indented.\n\n\
                    options:\n\
                    \x20 -h, --help            show this help message and exit\n\
                    \x20 --explain             explain the answer too\n\
                    \x20 -n N, --count N       how many answers\n\
                    \x20 --format=STYLE        output style\n\
                    \x20 --explain             (a duplicate row, kept once)\n";
        assert_eq!(
            extract_flags(help),
            ["-h", "--help", "--explain", "-n", "--count", "--format"]
        );
        assert!(extract_flags("no options here\n").is_empty());
    }

    #[test]
    fn extract_about_stops_at_a_blank_within_a_raw_description() {
        // A RawDescription help (author newlines kept) — a summary line, a blank, then a section:
        // only the summary paragraph is taken (the `launch.audit` shape).
        let help = "usage: python -m launch.audit [-h]\n\nAudit the launcher's state.\n\nReports:\n  - things\n";
        assert_eq!(extract_about(help).as_deref(), Some("Audit the launcher's state."));
    }
}
