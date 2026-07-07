//! Text-lookup commands: the shell-history search (`hg`) and the loud all-caps `GG` (recursive
//! search with `--delve` forced on). This module also owns the runners the generated shortcut
//! families forward to — `_grep` for the `g` family and `_gg` for the `gg` family (both in
//! [`crate::categories::autogen_lookup`]). `_gg` lives here, not in `support`, because it drives the
//! root re-scan round-trip in [`crate::internal_cli`], which sits above the `support` search engines
//! ([`streamgrep`](crate::support::streamgrep), [`treegrep`](crate::support::treegrep)); the shared
//! argument structs are in [`crate::support::args`].

#[bashrs_macros::category(command = LookupCommand, prefix = "look_")]
mod commands {
    use crate::support::args::{GgArgs, GrepArgs};
    use crate::support::{input, streamgrep, treegrep};
    use clap::Args;

    /// Search your shell history (case-insensitive, literal) — like `history | grep -iF`
    #[unprefixed]
    #[piped("history")]
    pub fn hg(args: HgArgs) {
        // The wrapper pipes `history` — a bash builtin only the shell itself can produce —
        // into our stdin; we read that stream and filter it in-process.
        let text = match input::read_input(None) {
            Ok(text) => text,
            Err(err) => return eprintln!("hg: {err}"),
        };
        streamgrep::filter(&args.pattern, &text, 0, false);
    }

    /// The term to find in your shell history.
    #[derive(Args)]
    pub struct HgArgs {
        /// Term to match (case-insensitive, literal — no regex).
        pattern: String,
    }

    /// Recursive search with `--delve` always on — also looks inside binaries we can decode (video
    /// subtitle tracks, `.torrent` text). The loud, all-caps sibling of `gg`, à la `UPUP`.
    #[name("GG")]
    #[trailing_newline]
    pub fn gg_delve(args: GgArgs) {
        // Force `--delve`; it's a plain bool, so setting it when the caller already passed it is a
        // harmless no-op.
        _gg(&GgArgs { delve: true, ..args }, 0);
    }

    /// Read the input, then filter it with `grep -iF` semantics (literal, case-insensitive) and
    /// `context` lines around each match (0 = none), using the `grep` crate in-process. With
    /// `line_number`, prefix each line with its number (`grep -n`). Matches are coloured only when
    /// writing to a terminal, like `--color=auto`. Backs the generated `g` family.
    pub(crate) fn _grep(args: &GrepArgs, context: usize) {
        let text = match input::read_input(args.source.as_deref()) {
            Ok(text) => text,
            Err(err) => return eprintln!("g: {err}"),
        };
        streamgrep::filter(&args.pattern, &text, context, args.line_number);
    }

    /// Build options from the shared `gg` args plus the chosen `context` (lines around each match),
    /// run the recursive search, then offer a root re-scan of anything that was unreadable. Backs
    /// the generated `gg` family and the hand-written `GG`.
    pub(crate) fn _gg(args: &GgArgs, context: usize) {
        let opts = treegrep::Options { line_number: !args.no_line_number, context, delve: args.delve };
        let roots = [std::path::PathBuf::from(&args.directory)];
        let denied = treegrep::search(&args.expressions, &roots, &opts);
        _offer_root_rescan(&args.expressions, &denied, &opts);
    }

    /// If some paths were unreadable and we're interactive, offer to re-scan just those as root by
    /// re-exec'ing ourselves under the superuser command, scoped to the denied paths (no dedup — they turned
    /// up nothing on the first pass). Non-interactive runs just note what was skipped.
    fn _offer_root_rescan(
        expressions: &[String],
        denied: &std::collections::BTreeSet<std::path::PathBuf>,
        opts: &treegrep::Options,
    ) {
        use std::io::{IsTerminal, Write};
        if denied.is_empty() {
            return;
        }
        if !std::io::stdin().is_terminal() {
            eprintln!("\n{} path(s) skipped (permission denied); run interactively to re-scan as root.", denied.len());
            return;
        }
        eprintln!("\n{} path(s) were unreadable (permission denied):", denied.len());
        // Cap the list so a huge count can't scroll the count/prompt off-screen: show up to 10, or
        // the first 9 plus a summary line when there are more.
        let shown = if denied.len() > 10 { 9 } else { denied.len() };
        for path in denied.iter().take(shown) {
            eprintln!("  {}", path.display());
        }
        if denied.len() > shown {
            eprintln!("  [{} more paths omitted]", denied.len() - shown);
        }
        eprint!("Re-scan them as root? [y/N] ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).is_err() || !answer.trim().eq_ignore_ascii_case("y") {
            return;
        }
        crate::internal_cli::GgElevatedRescan::reexec(expressions, denied, opts);
    }
}
