//! The greeting printed when `sourcefile.sh` is sourced — a one-line load confirmation.
//! Emitted last by [`crate::cli`], after the `gecho`/`boecho` wrappers it uses are
//! defined (so it can style itself).

/// The greeting command, run once when the sourcefile is sourced.
pub fn line() -> &'static str {
    r##"gecho "Loaded Bashrs! Press $(boecho ALT+H) to view your functions! Press $(boecho ALT+W) to edit configurations!""##
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets_via_gecho_with_styled_keys() {
        let l = line();
        assert!(l.starts_with("gecho \""), "should print via gecho");
        assert!(l.contains("Loaded Bashrs!"));
        assert!(l.contains("$(boecho ALT+H) to view your functions"), "styled ALT+H");
        assert!(l.contains("$(boecho ALT+W) to edit configurations"), "styled ALT+W");
    }
}
