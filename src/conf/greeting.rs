//! The greeting printed when a sourcefile is sourced — a one-line load confirmation.
//! Emitted last by [`crate::cli`], after the `gecho`/`boecho` wrappers it uses are
//! defined (so it can style itself).

use crate::support::shell_dialect::Shell;

/// The greeting command for `shell`, run once when its sourcefile is sourced — the same words, each
/// shell's own command substitution: fish splices `(…)` between quoted pieces, where bash writes
/// `$(…)` inside the quotes. (Fish 3.4 learned bash's spelling too; its own reads as fish.)
pub fn line(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => {
            r##"gecho "Loaded Bashrs! Press $(boecho ALT+H) to view your functions! Press $(boecho ALT+W) to edit configurations!""##
        }
        Shell::Fish => {
            r##"gecho "Loaded Bashrs! Press "(boecho ALT+H)" to view your functions! Press "(boecho ALT+W)" to edit configurations!""##
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets_via_gecho_with_styled_keys() {
        let l = line(Shell::Bash);
        assert!(l.starts_with("gecho \""), "should print via gecho");
        assert!(l.contains("Loaded Bashrs!"));
        assert!(l.contains("$(boecho ALT+H) to view your functions"), "styled ALT+H");
        assert!(l.contains("$(boecho ALT+W) to edit configurations"), "styled ALT+W");
    }

    /// Fish says the same, in its own substitution syntax: removing the substitutions and the
    /// quote marks around them leaves bash's words exactly.
    #[test]
    fn fish_greets_with_the_same_words() {
        let fish = line(Shell::Fish);
        assert!(fish.contains(r#""(boecho ALT+H)""#) && !fish.contains("$("), "{fish}");
        let words = |line: &str| line.replace("\"(", "").replace(")\"", "").replace("$(", "").replace(')', "");
        assert_eq!(words(fish), words(line(Shell::Bash)));
    }
}
