//! Hand-written style commands — the `StyleCommand` category. These are one-off,
//! non-generated echo verbs that build on the stylized-echo engine (`_wrap`/`_scoped`,
//! `EchoArgs`) living in [`crate::categories::autogen_styles`] beside the generated
//! `recho` family.
//!
//! This is the safe home for manual style commands: add them here. The generated matrix
//! is regenerated separately (see `tests/style_matrix.rs`) and never touches this file.

#[bashrs_macros::category(command = StyleCommand, prefix = "style_")]
mod commands {
    use crate::categories::autogen_styles::{EchoArgs, _scoped, _wrap};

    /// echo in bold red, to stderr
    #[unprefixed]
    pub fn errcho(args: EchoArgs) {
        eprintln!("{}", _scoped(&_wrap(["bo", "", "r"]), &args.words.join(" ")));
    }
}
