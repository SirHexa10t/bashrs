//! Session commands, emitted into `sourcefile.sh` as raw shell functions.
//!
//! A category by nature (user-facing commands), not by mechanism: these must run in the shell
//! itself — the binary is a child process and can't restart its parent shell — so instead of a
//! `#[category]` of clap commands, this module contributes the shell text directly.
//! `session_new` is referenced by the ALT+N keybind (see [`crate::conf::keybinds`]) and by
//! `bashrs_compile` (via `#[after]`); `session_bare` by CTRL+ALT+N.
//!
//! The pair around `_BASHRS_BARE` (underscore: machinery-internal, not a user knob):
//! `session_bare` starts a fresh shell with the flag exported, and the sourcefile's guard
//! (emitted by [`crate::cli`]'s generator, beside the interactivity check) consumes it —
//! `unset` + return — so the shell comes up bashrs-free AND with a clean environment. One-shot
//! by design: the flag never lingers, so any new shell (or re-sourcing the file by hand)
//! arms bashrs again; there is no special way back to remember.

/// Shell defining the session functions.
pub fn functions() -> &'static str {
    "session_new() { exec bash; }  # start a fresh shell session\n\
     session_bare() { _BASHRS_BARE=1 exec bash; }  # fresh shell WITHOUT bashrs (one-shot: any later shell, or re-sourcing, arms it again)\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_new_is_a_plain_fresh_shell() {
        // No flag-clearing needed: the sourcefile's guard consumes `_BASHRS_BARE` on read, so a
        // bare session's environment is already clean by the time anyone starts a new shell.
        assert!(functions().contains("session_new() { exec bash; }"));
    }

    #[test]
    fn session_bare_exports_the_flag_into_the_fresh_shell() {
        // A prefix assignment rides the exec into the new shell's environment, where the
        // sourcefile's guard consumes it — skip once, stay clean.
        assert!(functions().contains("session_bare() { _BASHRS_BARE=1 exec bash; }"));
    }
}
