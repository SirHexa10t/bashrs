//! Argument structs shared across command categories.

use clap::Args;
use std::path::PathBuf;

/// An empty argument set, for commands that take no arguments.
#[derive(Args)]
pub struct NoArgs {}

/// Paths a tree-walking command should not descend into — shared by every command that walks,
/// so the flag is defined once and reads identically wherever it appears.
///
/// Matching is by whole path **components**, not substring: `target/debug` skips
/// `…/target/debug/…` and leaves `…/target/bebuggers/…` alone. A trailing (or leading) `/` on
/// the argument is trimmed, so `target/debug/` and `target/debug` mean the same thing — the
/// separator is how a person writes a directory, not a matching rule.
///
/// No globbing. A literal component list has one meaning, where `*`/`?`/`[…]` would have to be
/// escaped in the very paths people most want to skip.
#[derive(Args)]
pub struct SkipArgs {
    /// Skip any path containing these components; repeatable
    /// (e.g. `--skip-pattern target/debug --skip-pattern .git`)
    #[arg(long, value_name = "PATTERN")]
    pub skip_pattern: Vec<String>,
    /// Skip machine-written data — libraries, compile output, caches (git's included). The
    /// list lives in [`crate::support::prog_langs`]; `_arg_lean_spec` prints it.
    #[arg(short = 'l', long, help = lean_help())]
    pub lean: bool,
}

/// `--lean`'s help: the categories, not the entries — the list grew past what a help line
/// carries, so the details moved to the `_arg_lean_spec` command (built from the same table
/// in [`crate::support::prog_langs`], so behaviour and spec cannot drift).
fn lean_help() -> String {
    "Skip machine-written data: libraries, compile output and caches (git's object store \
     included); adds to --skip-pattern rather than replacing it. For the exact list, run: \
     _arg_lean_spec"
        .to_string()
}

impl SkipArgs {
    /// Every skip this run asked for: `--lean`'s list first, then the user's.
    #[must_use]
    pub fn skips(&self) -> Vec<String> {
        let mut all: Vec<String> = if self.lean {
            crate::support::prog_langs::lean_patterns().map(str::to_string).collect()
        } else {
            Vec::new()
        };
        all.extend(self.skip_pattern.iter().cloned());
        all
    }
}

/// Whether `path` sits inside any of `skips`.
///
/// Both sides are reduced to component lists and compared as a contiguous run, which is what
/// makes `target/debug` miss `target/bebuggers` without any string anchoring. Non-`Normal`
/// components (the `.` of a relative walk root, `..`, a leading `/`) are dropped from the path
/// first, so where the walk started cannot change the answer.
#[must_use]
pub fn is_skipped(path: &std::path::Path, skips: &[String]) -> bool {
    let parts: Vec<String> = path
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    skips.iter().any(|skip| {
        let needle: Vec<&str> = skip.split('/').filter(|piece| !piece.is_empty()).collect();
        !needle.is_empty()
            && parts.len() >= needle.len()
            && parts.windows(needle.len()).any(|window| {
                // `*` matches exactly one component, whatever it is — enough to express
                // "any Cargo target triple" without inviting glob syntax back in.
                window
                    .iter()
                    .zip(&needle)
                    .all(|(part, want)| *want == "*" || part == want)
            })
    })
}

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
    /// Show line numbers, like `grep -n`. Already on whenever a SOURCE is named; this asks for
    /// them when reading piped input instead.
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
    /// Replace every match with this string, IN PLACE: matched files/dirs are renamed and matched
    /// lines rewritten (a literal swap of each matched span). Shows what would change and asks
    /// before touching anything; there is no undo. Skips `--delve`-decoded content, and anything
    /// `--skip-pattern`/`--lean` excluded is never rewritten either.
    #[arg(long = "re", value_name = "REPLACEMENT")]
    pub(crate) re: Option<String>,
    /// Paths to leave out of the walk.
    #[command(flatten)]
    pub(crate) skip: SkipArgs,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The reason skips match components and not substrings: `target/bebuggers` starts with
    /// `target/` and contains `debug` — a substring rule skips it, and the user loses a
    /// directory they never asked to lose.
    #[test]
    fn a_skip_matches_whole_components_not_substrings() {
        let skip = vec!["target/debug".to_string()];
        assert!(is_skipped(std::path::Path::new("a/target/debug/x.rs"), &skip));
        assert!(!is_skipped(std::path::Path::new("a/target/bebuggers/x.rs"), &skip));
        assert!(!is_skipped(std::path::Path::new("a/retarget/debug/x.rs"), &skip));
        assert!(!is_skipped(std::path::Path::new("a/target/x.rs"), &skip), "the run must be whole");
    }

    /// A trailing or leading slash is how a person writes a directory, not a matching rule.
    #[test]
    fn slashes_around_a_skip_do_not_change_its_meaning() {
        let path = std::path::Path::new("a/.git/config");
        for spelling in [".git", ".git/", "/.git", "/.git/"] {
            assert!(is_skipped(path, &[spelling.to_string()]), "{spelling:?}");
        }
    }

    /// Where the walk started must not change the answer, so `.`, `..` and a leading `/` are
    /// dropped before matching.
    #[test]
    fn the_walk_root_spelling_is_irrelevant() {
        let skip = vec!["target".to_string()];
        for spelling in ["./target/x", "target/x", "/home/u/target/x", "a/../target/x"] {
            assert!(is_skipped(std::path::Path::new(spelling), &skip), "{spelling:?}");
        }
        assert!(!is_skipped(std::path::Path::new("./src/x"), &skip));
    }

    #[test]
    fn lean_adds_its_list_ahead_of_the_users_and_is_off_by_default() {
        let plain = SkipArgs { skip_pattern: vec!["mine".to_string()], lean: false };
        assert_eq!(plain.skips(), vec!["mine".to_string()], "--lean is opt-in");

        let lean = SkipArgs { skip_pattern: vec!["mine".to_string()], lean: true };
        let skips = lean.skips();
        for expected in crate::support::prog_langs::lean_patterns() {
            assert!(skips.contains(&expected.to_string()), "{expected} is pinned");
        }
        assert!(skips.contains(&"mine".to_string()), "and the user's are kept, not replaced");
    }

    /// The list outgrew a help line, so the help carries the categories and a referral —
    /// this guards that the referral names a command that exists, since a dangling pointer
    /// in help text is worse than a long line.
    #[test]
    fn lean_help_refers_to_the_spec_command() {
        let command = GgArgs::augment_args(clap::Command::new("gg"));
        let lean = command
            .get_arguments()
            .find(|arg| arg.get_long() == Some("lean"))
            .expect("gg exposes --lean");
        let help = lean.get_help().expect("--lean is documented").to_string();
        assert!(help.contains("_arg_lean_spec"), "the referral is the whole point: {help}");
    }

    /// The wildcard is one whole component: a cross-compile triple matches, the plain layout
    /// does not double-match through it, and nothing smaller than a component wildcards.
    #[test]
    fn a_star_component_matches_any_single_directory() {
        let skip = vec!["target/*/release".to_string()];
        assert!(is_skipped(
            std::path::Path::new("p/target/aarch64-unknown-linux-gnu/release/lib.rlib"),
            &skip
        ));
        assert!(is_skipped(std::path::Path::new("p/target/wasm32-wasi/release/x"), &skip));
        assert!(
            !is_skipped(std::path::Path::new("p/target/release/x"), &skip),
            "* consumes exactly one component; the non-cross layout has its own entry"
        );
        assert!(
            !is_skipped(std::path::Path::new("p/target/a/b/release/x"), &skip),
            "and never more than one"
        );
    }
}
