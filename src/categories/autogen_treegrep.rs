//! The generated `gg`-family of recursive-search shortcuts (`GgCommand`) — bare verbs `gg` plus
//! the context variants `gg2`/`gg3`/`gg5`/`gg10` (the number is how many lines to show around each
//! file-content match). Each runs the parallel tree search in [`crate::support::treegrep`].
//!
//! Only the region between the `GENERATED-TREEGREP` markers is generated — `build.rs` rewrites it
//! from `generative_constants.rs` during the build (when either changes), so it's never edited by hand.
//! The engine around it (`_gg`, `GgArgs`) is hand-written. The history-search command lives in
//! [`crate::categories::lookup`] (`hg`); the stream `g`-family in
//! [`crate::categories::autogen_lookup`].

#[bashrs_macros::category(command = GgCommand, prefix = "look_")]
mod commands {
    use crate::support::treegrep;
    use clap::Args;

    // GENERATED-TREEGREP-START

    /// Recursively search a directory for expression(s) — filenames, then file contents
    #[unprefixed]
    #[trailing_newline]
    pub fn gg(args: GgArgs) { _gg(args, 0); }

    /// Recursive search, showing 2 lines of context around each file-content match
    #[unprefixed]
    #[trailing_newline]
    pub fn gg2(args: GgArgs) { _gg(args, 2); }

    /// Recursive search, showing 3 lines of context around each file-content match
    #[unprefixed]
    #[trailing_newline]
    pub fn gg3(args: GgArgs) { _gg(args, 3); }

    /// Recursive search, showing 5 lines of context around each file-content match
    #[unprefixed]
    #[trailing_newline]
    pub fn gg5(args: GgArgs) { _gg(args, 5); }

    /// Recursive search, showing 10 lines of context around each file-content match
    #[unprefixed]
    #[trailing_newline]
    pub fn gg10(args: GgArgs) { _gg(args, 10); }
    // GENERATED-TREEGREP-END

    /// Recursive, case-insensitive search across filenames and file contents (skips binaries).
    /// Shared by the whole `gg` family; each variant differs only in its context size.
    #[derive(Args)]
    pub struct GgArgs {
        /// Expression(s) to search for (literal, case-insensitive; multiple are OR'd together).
        #[arg(required = true)]
        expressions: Vec<String>,
        /// Directory to search.
        #[arg(short, long, default_value = ".")]
        directory: String,
        /// Omit matches on lines longer than N characters (skips minified/dumped text).
        #[arg(short = 't', long = "text-limit", default_value_t = 5000)]
        text_limit: u64,
        /// Shortcut for `--text-limit 500`.
        #[arg(short, long)]
        short: bool,
        /// No line-length limit — show even very long matching lines.
        #[arg(short, long)]
        unlimited: bool,
        /// Don't prefix matches with line numbers (they're on by default for `gg`).
        #[arg(long)]
        no_line_number: bool,
    }

    /// Build options from the shared `gg` args plus the chosen `context` (lines around each match),
    /// run the recursive search, then offer a root re-scan of anything that was unreadable.
    fn _gg(args: GgArgs, context: usize) {
        let text_limit = if args.unlimited {
            None
        } else if args.short {
            Some(500)
        } else {
            Some(args.text_limit)
        };
        let opts = treegrep::Options { text_limit, line_number: !args.no_line_number, context };
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
