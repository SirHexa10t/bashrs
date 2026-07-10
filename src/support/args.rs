//! Argument structs shared across command categories.

use clap::Args;
use std::path::PathBuf;

/// An empty argument set, for commands that take no arguments.
#[derive(Args)]
pub struct NoArgs {}

/// Everything the `g` family accepts EXCEPT the context knob — the numbered variants (`g2`,
/// `g3`, …) take exactly this, so the `-C` they pin doesn't exist for them (instead of being
/// silently overridden).
#[derive(Args)]
pub struct GrepBase {
    /// Term to match (case-insensitive; literal by default, or a regex with `-E`). When `-e` is
    /// used, this positional becomes the input instead (grep semantics).
    #[arg(required_unless_present = "regexp")]
    pub(crate) pattern: Option<String>,
    /// Text to search: a file path, inline text, or omitted to read stdin.
    pub(crate) source: Option<String>,
    /// Term(s) to match, like `grep -e` — protects a term starting with `-`; repeatable (OR'd).
    #[arg(short = 'e', long = "regexp", value_name = "PATTERN", allow_hyphen_values = true)]
    pub(crate) regexp: Vec<String>,
    /// Show line numbers, like `grep -n`.
    #[arg(short = 'n', long)]
    pub(crate) line_number: bool,
    /// Treat the pattern as a regular expression (à la `grep -E`) rather than literal text.
    #[arg(short = 'E', long = "extended-regexp")]
    pub(crate) regex: bool,
    /// Print the lines that DON'T match, like `grep -v`.
    #[arg(short = 'v', long = "invert-match")]
    pub(crate) invert: bool,
}

/// The full `g` argument set: the base plus the context knob the numbered variants pin.
#[derive(Args)]
pub struct GrepArgs {
    #[command(flatten)]
    pub(crate) base: GrepBase,
    /// Show N lines of context around each match (like `grep -C`); the `g<N>` variants are
    /// shorthand for this.
    #[arg(short = 'C', long, default_value_t = 0)]
    pub(crate) context: usize,
}

/// Everything the `gg` family accepts EXCEPT the context knob — the numbered variants (`gg2`,
/// `gg3`, …) take exactly this, so the `-C` they pin doesn't exist for them (instead of being
/// silently overridden).
#[derive(Args)]
pub struct GgBase {
    /// Expression(s) to search for (case-insensitive; multiple are OR'd together). One starting
    /// with `-`? Use `-e`, or put it after `--`.
    #[arg(required_unless_present = "regexp")]
    pub(crate) expressions: Vec<String>,
    /// Extra expression(s) to search for, like `grep -e` — protects an expression starting with
    /// `-`; repeatable, OR'd with the positional ones.
    #[arg(short = 'e', long = "regexp", value_name = "EXPRESSION", allow_hyphen_values = true)]
    pub(crate) regexp: Vec<String>,
    /// Directory to search.
    #[arg(short, long, default_value = ".")]
    pub(crate) directory: PathBuf,
    /// Don't prefix matches with line numbers (they're on by default for `gg`).
    #[arg(long)]
    pub(crate) no_line_number: bool,
    /// Also search inside files normally skipped as binary, by decoding known formats
    /// (video subtitle tracks, `.torrent` text).
    #[arg(long)]
    pub(crate) delve: bool,
    /// Treat the expression(s) as regular expressions (à la `grep -E`) rather than literal text.
    #[arg(short = 'E', long = "extended-regexp")]
    pub(crate) regex: bool,
    /// Also write the results (plain) to `deep_search_<timestamp>` in the current directory while
    /// searching, leaving a sorted `deep_search_<timestamp>_sorted` once done (`gg --save`).
    #[arg(short = 's', long = "save")]
    pub(crate) save: bool,
}

/// The full `gg` argument set: the base plus the context knob the numbered variants pin.
/// Recursive, case-insensitive search across filenames and file contents (skips binaries).
#[derive(Args)]
pub struct GgArgs {
    #[command(flatten)]
    pub(crate) base: GgBase,
    /// Show N lines of context around each file-content match (like `grep -C`); the `gg<N>`
    /// variants are shorthand for this.
    #[arg(short = 'C', long, default_value_t = 0)]
    pub(crate) context: usize,
}

/// Words to print, joined with spaces (like `echo`).
#[derive(Args)]
pub struct EchoArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub(crate) words: Vec<String>,
}
