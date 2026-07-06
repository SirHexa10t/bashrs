//! Text-lookup commands: search the shell's command history (`hg`) or a whole directory tree
//! (`gg`), all in-process on the `grep` crate. The grep-over-a-stream shortcuts (`g`, `g3`, …) are
//! generated in [`crate::categories::autogen_lookup`].

#[bashrs_macros::category(command = LookupCommand, prefix = "look_")]
mod commands {
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

    /// Recursively search a directory for expression(s) — matching filenames, then file contents
    #[unprefixed]
    #[trailing_newline]
    pub fn gg(args: GgArgs) {
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
        };
        treegrep::search(&args.expressions, &opts);
    }

    /// Recursive, case-insensitive search across filenames and file contents (skips binaries).
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
}
