//! bashrs's internal command line — the commands the binary invokes on *itself*, never typed by a
//! user, kept separate from the user-facing [`crate::cli`] so inner-workings don't mix with real
//! commands. Each internal command is its own struct; [`handle`] dispatches to them by an `argv`
//! marker, so adding one is a new struct plus a new arm there.
//!
//! There's one today: [`GgElevatedRescan`]. When `gg` can't read some paths, it re-execs
//! `sudo bashrs …` — the current process is unprivileged, so the search has to run in a fresh root
//! process. That process is recognised and dispatched by [`handle`], called from the binary entry
//! point ([`crate::run`]) before the user CLI ever sees it.

use crate::support::{superuser, treegrep};
use clap::Parser;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// `gg`'s elevated re-scan — two sides of one round-trip: [`reexec`](Self::reexec) builds the `sudo`
/// invocation (parent side, called by `gg`), and this struct parses it back (child side, run by
/// [`handle`]). Its flags mirror the tuning `gg` resolved, plus the exact paths to re-search as root.
#[derive(Parser)]
pub(crate) struct GgElevatedRescan {
    #[arg(long = "expr")]
    expressions: Vec<String>,
    #[arg(long)]
    context: usize,
    #[arg(long = "text-limit")]
    text_limit: Option<u64>,
    #[arg(long = "no-number")]
    no_number: bool,
    #[arg(long)]
    delve: bool,
    /// Directories are walked recursively; files are searched directly.
    paths: Vec<PathBuf>,
}

impl GgElevatedRescan {
    /// The `argv[1]` marker tagging this command's re-exec, so [`handle`] can route to it. Written
    /// by [`Self::reexec`], read by [`handle`] — one source of truth, so the two sides can't drift.
    const MARKER: &str = "gg-elevated-rescan";

    /// Parent side: re-exec this binary under the superuser to re-scan `paths` (the ones the
    /// unprivileged pass couldn't read), forwarding `gg`'s tuning, then drop the elevation. `gg`
    /// calls this once the user approves; the spawned root process lands in [`handle`]. No dedup —
    /// the paths turned up nothing on the first pass.
    pub(crate) fn reexec(expressions: &[String], paths: &BTreeSet<PathBuf>, opts: &treegrep::Options) {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(err) => return eprintln!("gg: can't locate myself to re-run as root: {err}"),
        };
        // `<superuser> <exe> gg-elevated-rescan --context N [--text-limit N] [--no-number] --expr E
        // … <paths>` — exactly the shape this struct parses back.
        let mut cmd = superuser::command();
        cmd.arg(exe).arg(Self::MARKER).arg("--context").arg(opts.context.to_string());
        if let Some(limit) = opts.text_limit {
            cmd.arg("--text-limit").arg(limit.to_string());
        }
        if !opts.line_number {
            cmd.arg("--no-number");
        }
        if opts.delve {
            cmd.arg("--delve");
        }
        for expr in expressions {
            cmd.arg("--expr").arg(expr);
        }
        cmd.args(paths);
        let _ = cmd.status();
        // Drop the cached elevation so it can't carry over to a later command.
        superuser::revoke();
    }

    /// Child side: run the parsed re-scan as the now-elevated (root) process.
    fn run(self) {
        let opts = treegrep::Options {
            text_limit: self.text_limit,
            line_number: !self.no_number,
            context: self.context,
            delve: self.delve,
        };
        treegrep::search(&self.expressions, &self.paths, &opts);
    }
}

/// Dispatch a self-re-exec to its internal command by matching `argv[1]` against each command's
/// marker; return `false` for an ordinary invocation, so [`crate::run`] falls through to the user
/// CLI. Called from the binary entry point ahead of clap — these are never user-typed commands.
/// A new internal command adds an arm here.
pub(crate) fn handle() -> bool {
    match std::env::args_os().nth(1).and_then(|a| a.into_string().ok()).as_deref() {
        // `parse_from` treats its first item as the program name and ignores it, so skipping the exe
        // leaves the marker consumed there, with the real flags and paths parsed after it.
        Some(GgElevatedRescan::MARKER) => {
            GgElevatedRescan::parse_from(std::env::args_os().skip(1)).run();
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gg_elevated_rescan_parses_the_reexec_argv_shape() {
        // Mirrors what `GgElevatedRescan::reexec` builds: the marker (consumed as the program name
        // by `parse_from`), gg's tuning flags, then the paths as positionals.
        let cli = GgElevatedRescan::parse_from([
            GgElevatedRescan::MARKER, "--context", "3", "--text-limit", "500", "--no-number",
            "--expr", "foo", "--expr", "bar", "/a", "/b",
        ]);
        assert_eq!(cli.expressions, ["foo", "bar"]);
        assert_eq!(cli.context, 3);
        assert_eq!(cli.text_limit, Some(500));
        assert!(cli.no_number);
        assert_eq!(cli.paths, [PathBuf::from("/a"), PathBuf::from("/b")]);
    }
}
