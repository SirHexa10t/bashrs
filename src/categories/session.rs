//! Session commands, emitted into `sourcefile.sh` as raw shell functions.
//!
//! A category by nature (user-facing commands), not by mechanism: these must run in the shell
//! itself — the binary is a child process and can't restart its parent shell — so instead of a
//! `#[category]` of clap commands, this module contributes the shell text directly.
//! `session_new` is referenced by the ALT+N keybind (see [`crate::conf::keybinds`]) and by
//! `bashrs_compile` (via `#[after]`).

/// Shell defining the session functions.
pub fn functions() -> &'static str {
    "session_new() { exec bash; }\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defines_session_new_as_exec_bash() {
        assert!(functions().contains("session_new() { exec bash; }"));
    }
}
