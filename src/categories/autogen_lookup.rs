//! The grep engine and the generated `g`-family of search shortcuts (`GrepCommand`) — bare,
//! memorable verbs (`g`, `g3`, …) that filter their input with the `grep` crate in-process (see
//! [`crate::support::streamgrep`]). The number in a name is how many lines of context to show
//! around each match.
//!
//! Only the region between the `GENERATED-LOOKUP-GREP` markers is generated — `build.rs`
//! rewrites it from `generative_constants.rs` during the build (when either changes), so it's
//! never edited by hand. The engine around it (`_grep`, `GrepArgs`) is hand-written. The
//! history-search command sharing the same in-process grep lives in
//! [`crate::categories::lookup`] (`hg`).

#[bashrs_macros::category(command = GrepCommand, prefix = "look_")]
mod commands {
    use crate::support::{input, streamgrep};
    use clap::Args;

    // GENERATED-LOOKUP-GREP-START

    /// Case-insensitive literal search (no regex), colouring matches
    #[unprefixed]
    pub fn g(args: GrepArgs) { _grep(&args.pattern, 0, args.source.as_deref(), args.line_number); }

    /// Case-insensitive literal search, showing 2 lines of context around each match
    #[unprefixed]
    pub fn g2(args: GrepArgs) { _grep(&args.pattern, 2, args.source.as_deref(), args.line_number); }

    /// Case-insensitive literal search, showing 3 lines of context around each match
    #[unprefixed]
    pub fn g3(args: GrepArgs) { _grep(&args.pattern, 3, args.source.as_deref(), args.line_number); }

    /// Case-insensitive literal search, showing 5 lines of context around each match
    #[unprefixed]
    pub fn g5(args: GrepArgs) { _grep(&args.pattern, 5, args.source.as_deref(), args.line_number); }

    /// Case-insensitive literal search, showing 8 lines of context around each match
    #[unprefixed]
    pub fn g8(args: GrepArgs) { _grep(&args.pattern, 8, args.source.as_deref(), args.line_number); }

    /// Case-insensitive literal search, showing 25 lines of context around each match
    #[unprefixed]
    pub fn g25(args: GrepArgs) { _grep(&args.pattern, 25, args.source.as_deref(), args.line_number); }
    // GENERATED-LOOKUP-GREP-END

    /// The term to match plus where to read the text — shared by the whole `g` family.
    #[derive(Args)]
    pub struct GrepArgs {
        /// Term to match (case-insensitive, literal — no regex).
        pattern: String,
        /// Text to search: a file path, inline text, or omitted to read stdin.
        source: Option<String>,
        /// Show line numbers, like `grep -n`.
        #[arg(short = 'n', long)]
        line_number: bool,
    }

    /// Read the input, then filter it with `grep -iF` semantics (literal, case-insensitive) and
    /// `context` lines around each match (0 = none), using the `grep` crate in-process. With
    /// `line_number`, prefix each line with its number (`grep -n`). Matches are coloured only when
    /// writing to a terminal, like `--color=auto`.
    fn _grep(pattern: &str, context: u32, source: Option<&str>, line_number: bool) {
        let text = match input::read_input(source) {
            Ok(text) => text,
            Err(err) => return eprintln!("g: {err}"),
        };
        streamgrep::filter(pattern, &text, context as usize, line_number);
    }
}
