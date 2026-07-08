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
//! [`crate::support::args`], and the hand-written `GG`/`GGG` commands sit beside the runners in
//! [`crate::categories::lookup`].

#[bashrs_macros::category(command = GrepCommand, prefix = "lookup_")]
mod grep_commands {
    use crate::categories::lookup::_grep;
    use crate::support::args::{GrepArgs, GrepBase};

    // GENERATED-LOOKUP-GREP-START

    /// Case-insensitive search (literal, or regex with -E), colouring matches
    #[unprefixed]
    pub fn g(args: GrepArgs) { _grep(&args); }

    /// Case-insensitive search, showing 2 lines of context around each match
    #[unprefixed]
    pub fn g2(args: GrepBase) { _grep(&GrepArgs { base: args, context: 2 }); }

    /// Case-insensitive search, showing 3 lines of context around each match
    #[unprefixed]
    pub fn g3(args: GrepBase) { _grep(&GrepArgs { base: args, context: 3 }); }

    /// Case-insensitive search, showing 5 lines of context around each match
    #[unprefixed]
    pub fn g5(args: GrepBase) { _grep(&GrepArgs { base: args, context: 5 }); }

    /// Case-insensitive search, showing 8 lines of context around each match
    #[unprefixed]
    pub fn g8(args: GrepBase) { _grep(&GrepArgs { base: args, context: 8 }); }

    /// Case-insensitive search, showing 25 lines of context around each match
    #[unprefixed]
    pub fn g25(args: GrepBase) { _grep(&GrepArgs { base: args, context: 25 }); }
    // GENERATED-LOOKUP-GREP-END
}

#[bashrs_macros::category(command = GgCommand, prefix = "lookup_")]
mod tree_commands {
    use crate::categories::lookup::_gg;
    use crate::support::args::{GgArgs, GgBase};

    // GENERATED-TREEGREP-START

    /// Recursively search a directory for expression(s) — filenames, then file contents
    #[unprefixed]
    #[trailing_newline]
    pub fn gg(args: GgArgs) { _gg(&args); }

    /// Recursive search, showing 2 lines of context around each file-content match
    #[unprefixed]
    #[trailing_newline]
    pub fn gg2(args: GgBase) { _gg(&GgArgs { base: args, context: 2 }); }

    /// Recursive search, showing 3 lines of context around each file-content match
    #[unprefixed]
    #[trailing_newline]
    pub fn gg3(args: GgBase) { _gg(&GgArgs { base: args, context: 3 }); }

    /// Recursive search, showing 5 lines of context around each file-content match
    #[unprefixed]
    #[trailing_newline]
    pub fn gg5(args: GgBase) { _gg(&GgArgs { base: args, context: 5 }); }

    /// Recursive search, showing 10 lines of context around each file-content match
    #[unprefixed]
    #[trailing_newline]
    pub fn gg10(args: GgBase) { _gg(&GgArgs { base: args, context: 10 }); }
    // GENERATED-TREEGREP-END
}
