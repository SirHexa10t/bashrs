//! The greeting printed when `sourcefile.sh` is sourced — a one-line load confirmation.
//! Emitted last by [`crate::cli`], after the `gecho`/`boecho` wrappers it uses are
//! defined (so it can style itself).

/// The greeting command, run once when the sourcefile is sourced.
pub fn line() -> &'static str {
    r##"gecho "Loaded Bashrs! Press $(boecho ALT+H) to view your functions!""##
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets_via_gecho_with_a_styled_key() {
        let l = line();
        assert!(l.starts_with("gecho \""), "should print via gecho");
        assert!(l.contains("Loaded Bashrs!"));
        assert!(l.contains("$(boecho ALT+H)"), "the key should be styled via boecho");
    }
}
