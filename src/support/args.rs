//! Argument structs shared across command categories.

use clap::Args;

/// An empty argument set, for commands that take no arguments.
#[derive(Args)]
pub struct NoArgs {}

/// The term to match plus where to read the text — shared by the whole `g` family.
#[derive(Args)]
pub struct GrepArgs {
    /// Term to match (case-insensitive; literal by default, or a regex with `-E`).
    pub(crate) pattern: String,
    /// Text to search: a file path, inline text, or omitted to read stdin.
    pub(crate) source: Option<String>,
    /// Show line numbers, like `grep -n`.
    #[arg(short = 'n', long)]
    pub(crate) line_number: bool,
    /// Show N lines of context around each match (like `grep -C`); the `g<N>` variants are
    /// shorthand for this.
    #[arg(short = 'C', long, default_value_t = 0)]
    pub(crate) context: usize,
    /// Treat the pattern as a regular expression (à la `grep -E`) rather than literal text.
    #[arg(short = 'E', long = "extended-regexp")]
    pub(crate) regex: bool,
    /// Print the lines that DON'T match, like `grep -v`.
    #[arg(short = 'v', long = "invert-match")]
    pub(crate) invert: bool,
}

/// Recursive, case-insensitive search across filenames and file contents (skips binaries).
/// Shared by the whole `gg` family; each variant differs only in its context size.
#[derive(Args)]
pub struct GgArgs {
    /// Expression(s) to search for (literal, case-insensitive; multiple are OR'd together).
    #[arg(required = true)]
    pub(crate) expressions: Vec<String>,
    /// Directory to search.
    #[arg(short, long, default_value = ".")]
    pub(crate) directory: String,
    /// Show N lines of context around each file-content match (like `grep -C`); the `gg<N>`
    /// variants are shorthand for this.
    #[arg(short = 'C', long, default_value_t = 0)]
    pub(crate) context: usize,
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
    /// Also write the results (sorted, plain) to `deep_search_<timestamp>` in the current directory,
    /// while still printing them to the terminal (`gg --save`).
    #[arg(short = 's', long = "save")]
    pub(crate) save: bool,
}

/// Words to print, joined with spaces (like `echo`).
#[derive(Args)]
pub struct EchoArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub(crate) words: Vec<String>,
}
