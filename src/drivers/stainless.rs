//! Non-Rust ("stainless" — Rust-resistant) companion repos, cloned under `~/.bashrs/stainless_comfy`
//! and exposed as plain shell aliases in `sourcefile.sh`. Data-driven, like [`crate::conf::keybinds`]: a
//! `const` table plus functions that turn it into shell lines — no macro, no clap command.
//!
//! Two consumers of the one table:
//! - [`sync`] — run by the separate `stainless_sync` binary from `COMPILE.sh` (never installed):
//!   clone or update each repo.
//! - [`aliases`] — run by the main binary's [`crate::cli`] generator: emit one alias per repo, asking
//!   the (freshly cloned) tool itself for its `--help` description to use as the inline comment.
//!
//! `sync` runs first in `COMPILE.sh`, so the clones exist when `aliases` probes them. The clone is
//! the only shared artifact — nothing copies the tool's own `--help` into a side file to go stale.

use std::path::PathBuf;
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
}];

/// Clone root, `$HOME`-relative — kept as the "comfy" area (the dir you picked) even though the
/// module is the general stainless one.
const CLONE_BASE: &str = ".bashrs/stainless_comfy";

/// `https://…/contAInerized(.git)(/)` → `contAInerized`.
fn repo_name(repo: &str) -> &str {
    repo.trim_end_matches('/').rsplit('/').next().unwrap_or(repo).trim_end_matches(".git")
}

fn clone_dir(comfy: &Comfy) -> PathBuf {
    std::env::home_dir().unwrap_or_default().join(CLONE_BASE).join(repo_name(comfy.repo))
}

/// The `$HOME`-relative executable path — used verbatim in the alias (portable) and in the `--help`
/// probe (the shell expands `$HOME`).
fn exe_shell(comfy: &Comfy) -> String {
    format!("$HOME/{CLONE_BASE}/{}/{}", repo_name(comfy.repo), comfy.exe)
}

/// SIDE EFFECTS — the `stainless_sync` binary runs this at compile time: clone each repo if missing,
/// else best-effort `git pull`. Never aborts; a git failure just warns and the alias is emitted
/// anyway (pointing at its expected path). The `--help` description is read later, by [`aliases`].
pub fn sync() {
    for comfy in STAINLESS {
        let dir = clone_dir(comfy);
        if dir.exists() {
            git(Command::new("git").arg("-C").arg(&dir).args(["pull", "--ff-only"]), comfy.repo);
        } else {
            if let Some(parent) = dir.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            git(Command::new("git").args(["clone", "--depth", "1", comfy.repo]).arg(&dir), comfy.repo);
        }
        // The repo's runtime python packages, into the bundled environment (best-effort, like the
        // clone — `install` explains itself when the environment or uv is missing).
        if !super::python::install(comfy.python_deps) {
            eprintln!("stainless: {}'s python packages may be missing", repo_name(comfy.repo));
        }
    }
}

/// Run a prepared `git` command, warning (never failing the compile) if it doesn't succeed.
fn git(cmd: &mut Command, repo: &str) {
    if !matches!(cmd.status(), Ok(status) if status.success()) {
        eprintln!("stainless: git failed for {repo}; its alias may be stale or point at a missing path");
    }
}

/// Probe `<run> <exe> --help` (through a shell, so `$HOME` in `exe` expands) for a description.
/// The launcher goes through [`crate::tools::resolve`], mirroring the sourcefile's PATH shim — the probe
/// works even when the launcher is only bundled. Best-effort: any failure yields `None` (no comment).
fn fetch_about(comfy: &Comfy) -> Option<String> {
    let run = crate::tools::resolve(comfy.run);
    let script = format!("\"{}\" \"{}\" --help 2>&1", run.to_string_lossy(), exe_shell(comfy));
    let out = Command::new("bash").arg("-c").arg(script).output().ok()?;
    if !out.status.success() {
        return None;
    }
    extract_about(&String::from_utf8_lossy(&out.stdout))
}

/// Pure: the one-line description from `--help` text — the first non-empty line after the (possibly
/// multi-line) `usage:` block, else the first non-empty non-`usage` line. Tunable; shape your tool's
/// `--help` to suit.
fn extract_about(help: &str) -> Option<String> {
    let lines: Vec<&str> = help.lines().map(str::trim).collect();
    if let Some(blank) = lines.iter().position(|line| line.is_empty()) {
        if let Some(desc) = lines[blank + 1..].iter().find(|line| !line.is_empty()) {
            return Some((*desc).to_string());
        }
    }
    lines
        .iter()
        .find(|line| !line.is_empty() && !line.to_ascii_lowercase().starts_with("usage"))
        .map(|line| (*line).to_string())
}

/// The main binary's generator runs this. One alias per repo; for a repo that's actually been
/// cloned, it asks the tool itself (`--help`, live) for the description to use as an inline comment.
/// The clone check keeps it from spawning anything before the first `sync` (or during tests).
pub(crate) fn aliases() -> String {
    STAINLESS
        .iter()
        .map(|comfy| {
            let about = clone_dir(comfy).exists().then(|| fetch_about(comfy)).flatten();
            alias_line(comfy, about.as_deref())
        })
        .collect()
}

/// Pure: one alias definition line (`about` becomes an inline `#` comment when present).
fn alias_line(comfy: &Comfy, about: Option<&str>) -> String {
    let comment = about.map(|about| format!("  # {about}")).unwrap_or_default();
    format!("{}() {{ {} \"{}\" \"$@\"; }}{comment}\n", comfy.alias, comfy.run, exe_shell(comfy))
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
}
