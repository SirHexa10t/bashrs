//! The generated search-shortcut families of the `lookup` category — bare, memorable verbs, each
//! its own `#[category]` block and clap command enum:
//!
//! - the `g`-family (`GrepCommand`): `g`, `g3`, … — grep over a single stream.
//! - the `gg`-family (`GgCommand`): `gg`, `gg2`, … — the recursive parallel tree search.
//!
//! Each block is nothing but its generated shim region (between the `GENERATED-*` markers, rewritten
//! by `build.rs` from `generative_constants.rs`) plus the imports those shims need — so this file is
//! never edited by hand. Every shim just forwards its parsed args to a runner in
//! [`crate::categories::lookup`] (`_grep` / `_gg`); the argument structs live in
//! [`crate::support::args`], and the hand-written `GG` command sits beside the runners in
//! [`crate::categories::lookup`].

#[bashrs_macros::category(command = GrepCommand, prefix = "lookup_")]
mod grep_commands {
    use crate::categories::lookup::_grep;
    use crate::support::args::GrepArgs;

    // GENERATED-LOOKUP-GREP-START

    /// Case-insensitive literal search (no regex), colouring matches
    #[unprefixed]
    pub fn g(args: GrepArgs) { _grep(&args, 0); }

    /// Case-insensitive literal search, showing 2 lines of context around each match
    #[unprefixed]
    pub fn g2(args: GrepArgs) { _grep(&args, 2); }

    /// Case-insensitive literal search, showing 3 lines of context around each match
    #[unprefixed]
    pub fn g3(args: GrepArgs) { _grep(&args, 3); }

    /// Case-insensitive literal search, showing 5 lines of context around each match
    #[unprefixed]
    pub fn g5(args: GrepArgs) { _grep(&args, 5); }

    /// Case-insensitive literal search, showing 8 lines of context around each match
    #[unprefixed]
    pub fn g8(args: GrepArgs) { _grep(&args, 8); }

    /// Case-insensitive literal search, showing 25 lines of context around each match
    #[unprefixed]
    pub fn g25(args: GrepArgs) { _grep(&args, 25); }
    // GENERATED-LOOKUP-GREP-END
}

#[bashrs_macros::category(command = GgCommand, prefix = "lookup_")]
mod tree_commands {
    use crate::categories::lookup::_gg;
    use crate::support::args::GgArgs;

    // GENERATED-TREEGREP-START

    /// Recursively search a directory for expression(s) — filenames, then file contents
    #[unprefixed]
    #[trailing_newline]
    pub fn gg(args: GgArgs) { _gg(&args, 0); }

    /// Recursive search, showing 2 lines of context around each file-content match
    #[unprefixed]
    #[trailing_newline]
    pub fn gg2(args: GgArgs) { _gg(&args, 2); }

    /// Recursive search, showing 3 lines of context around each file-content match
    #[unprefixed]
    #[trailing_newline]
    pub fn gg3(args: GgArgs) { _gg(&args, 3); }

    /// Recursive search, showing 5 lines of context around each file-content match
    #[unprefixed]
    #[trailing_newline]
    pub fn gg5(args: GgArgs) { _gg(&args, 5); }

    /// Recursive search, showing 10 lines of context around each file-content match
    #[unprefixed]
    #[trailing_newline]
    pub fn gg10(args: GgArgs) { _gg(&args, 10); }
    // GENERATED-TREEGREP-END
}
