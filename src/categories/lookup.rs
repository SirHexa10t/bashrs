//! Text-lookup commands: search the shell's command history (`hg`), filtered in-process with the
//! `grep` crate. The recursive `gg` family is generated in
//! [`crate::categories::autogen_treegrep`]; the grep-over-a-stream shortcuts (`g`, `g3`, …) in
//! [`crate::categories::autogen_lookup`].

#[bashrs_macros::category(command = LookupCommand, prefix = "look_")]
mod commands {
    use crate::support::{input, streamgrep};
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
}
