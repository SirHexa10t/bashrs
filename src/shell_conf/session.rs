//! Session shell functions emitted into `sourcefile.sh`.
//!
//! These must run in the shell itself (the binary is a child process and can't
//! restart its parent shell), so this is a plain module contributing raw shell,
//! not a `#[category]`. `session_new` is referenced by the ALT+N keybind (see
//! [`super::keybinds`]) and by `bashrs_compile` (via `#[after]`).

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
