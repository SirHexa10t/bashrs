//! Argument structs shared across command categories.

use clap::Args;

/// An empty argument set, for commands that take no arguments.
#[derive(Args)]
pub struct NoArgs {}

/// The term to match plus where to read the text — shared by the whole `g` family.
#[derive(Args)]
pub struct GrepArgs {
    /// Term to match (case-insensitive, literal — no regex).
    pub(crate) pattern: String,
    /// Text to search: a file path, inline text, or omitted to read stdin.
    pub(crate) source: Option<String>,
    /// Show line numbers, like `grep -n`.
    #[arg(short = 'n', long)]
    pub(crate) line_number: bool,
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
    /// Don't prefix matches with line numbers (they're on by default for `gg`).
    #[arg(long)]
    pub(crate) no_line_number: bool,
    /// Also search inside files normally skipped as binary, by decoding known formats
    /// (video subtitle tracks, `.torrent` text).
    #[arg(long)]
    pub(crate) delve: bool,
}

/// Words to print, joined with spaces (like `echo`).
#[derive(Args)]
pub struct EchoArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub(crate) words: Vec<String>,
}
