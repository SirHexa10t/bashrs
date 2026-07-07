//! The generated `gg`-family of recursive-search shortcuts (`GgCommand`) — bare verbs `gg` plus
//! the context variants `gg2`/`gg3`/`gg5`/`gg10` (the number is how many lines to show around each
//! file-content match). Each runs the parallel tree search in [`crate::support::treegrep`].
//!
//! Only the region between the `GENERATED-TREEGREP` markers is generated — `build.rs` rewrites it
//! from `treegrep_vocab.rs` during the build (when either changes), so it's never edited by hand.
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
    /// then run the recursive search.
    fn _gg(args: GgArgs, context: usize) {
        let text_limit = if args.unlimited {
            None
        } else if args.short {
            Some(500)
        } else {
            Some(args.text_limit)
        };
        let opts = treegrep::Options {
            dir: args.directory,
            text_limit,
            line_number: !args.no_line_number,
            context,
        };
        treegrep::search(&args.expressions, &opts);
    }
}
