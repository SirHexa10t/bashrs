//! Readline keybind → shell-function mappings, resolved into `bind` lines in
//! `sourcefile.sh` by the [`crate::cli`] generator. Each entry maps a readline key
//! sequence to a shell function (defined elsewhere, e.g. in [`super::session`]).

/// `(readline key sequence, shell function to run)` pairs.
///
/// `\en` is ALT+N (ESC-n). The function is run as if typed at the prompt, so the
/// generator appends the Enter (`\n`) that executes it.
pub fn bindings() -> &'static [(&'static str, &'static str)] {
    &[
        (r"\en", "session_new"),       // ALT+N → start a fresh shell session
        (r"\eh", "bashrs_sourcefile"), // ALT+H → run bashrs_sourcefile
        (r"\eq", "bashrs_compile"),    // ALT+Q → run bashrs_compile
    ]
}
