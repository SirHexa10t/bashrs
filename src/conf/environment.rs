//! Environment-variable and prompt settings emitted into the sourcefiles. Like
//! [`crate::categories::shell`], this is raw shell (not a `#[category]`), but authored as a small
//! table — variable, shell-quoted value, and comment — that [`settings`] renders into
//! `export` lines. Options that aren't on by default are kept commented out *in this
//! table*, as a reminder to whoever edits it that they're there to switch on.
//!
//! Each row also says what it becomes in fish (`InFish`): the same variable, or nothing, where
//! the variable configures bash itself and fish does that job its own way — `hg` asks fish's
//! history for the timestamps `HISTTIMEFORMAT` gives bash. The prompt clock is no row: bash
//! prepends it to `PS1`, while fish draws its prompt with a function, which bashrs wraps — see
//! `prompt_clock`.

use crate::support::shell_dialect::{fish_original, Shell};

/// One `export`: the variable, its shell-quoted value (quotes and all, since single vs.
/// double quoting is significant), a trailing comment (`""` for none), and its fish counterpart.
struct Export {
    var: &'static str,
    value: &'static str,
    comment: &'static str,
    fish: InFish,
}

/// What an [`Export`] becomes in the fish sourcefile.
enum InFish {
    /// The same variable, set with `set -gx` to the same value — which must therefore be a plain
    /// `'…'` string with no backslash, read identically by both shells (a test holds every such
    /// row to that).
    Same,
    /// No variable: it configures bash itself. The text says how fish does the same job, and the
    /// fish file carries it as a comment, so whoever reads that file finds the setting accounted for.
    BashOnly(&'static str),
}

const EXPORTS: &[Export] = &[
    // Uncomment to set your preferred editor:
    // Export { var: "EDITOR", value: "'vim'", comment: "", fish: InFish::Same },
    // Export { var: "VISUAL", value: "'vim'", comment: "", fish: InFish::Same },
    Export {
        var: "GREP_COLORS",
        value: "'mt=7;31'",
        comment: "sets grep marking for found strings to have red background (instead of being bold red)",
        fish: InFish::Same,
    },
    Export {
        var: "HISTTIMEFORMAT",
        value: r##""%F_%T  ""##,
        comment: r#"Makes "history" command display the time the command ran at. You can find more history settings in your own .bashrc file"#,
        fish: InFish::BashOnly(
            "fish stamps every history entry itself; `hg` shows the stamps in this same format, as `history --show-time='%F_%T  '` does",
        ),
    },
];

/// The settings for `shell`: `EXPORTS` in that shell's syntax, then the prompt clock.
pub fn settings(shell: Shell) -> String {
    match shell {
        Shell::Bash => bash_exports() + &bash_prompt_clock(),
        Shell::Fish => fish_exports() + &fish_prompt_clock(),
    }
}

/// [`EXPORTS`] as `export` lines, aligning the trailing `#` comments with the shared
/// [`table_formatter`] helper. A 3-space code→comment delimiter plus a split threshold of 3 let it
/// align these without splitting a value like `HISTTIMEFORMAT="%F_%T  "` on its interior double
/// space (the default threshold of 2 would).
fn bash_exports() -> String {
    let lines: Vec<String> = EXPORTS
        .iter()
        .map(|e| {
            if e.comment.is_empty() {
                format!("export {}={}", e.var, e.value)
            } else {
                format!("export {}={}   # {}", e.var, e.value, e.comment)
            }
        })
        .collect();
    // divide_by "   " (3 spaces) splits code from its `# comment` on the 3-space gap — the old
    // threshold:3 — while join_with keeps the default 2-space output gap. Fallback unreachable:
    // sort is off and the delimiters are valid.
    let opts = table_formatter::FormatOptions {
        divide_by: "   ".to_string(),
        trim_trailing: true,
        ..Default::default()
    };
    table_formatter::format_table(&lines, &opts)
        .unwrap_or(lines)
        .iter()
        .map(|line| format!("{line}\n"))
        .collect()
}

/// The [`InFish::Same`] rows as `set -gx` lines, their comments trailing as bash's do, and each
/// [`InFish::BashOnly`] row as the comment saying what fish does instead.
fn fish_exports() -> String {
    EXPORTS
        .iter()
        .map(|e| match (&e.fish, e.comment) {
            (InFish::Same, "") => format!("set -gx {} {}\n", e.var, e.value),
            (InFish::Same, comment) => format!("set -gx {} {}  # {comment}\n", e.var, e.value),
            (InFish::BashOnly(instead), _) => format!("# {}: {instead}\n", e.var),
        })
        .collect()
}

/// The UTC clock at the front of the prompt, as bash's prompt string evaluates it at every prompt.
const CLOCK: &str = "$(date -u +%T)(UTC)]";

/// Bash's prompt clock: `PS1` with the time prepended, in red — once. Sourcing the file again would
/// otherwise stack a second clock, and so would a nested shell, which inherits the exported `PS1`
/// with the clock already in it. Told by the clock's own text, which nothing else puts in a prompt.
fn bash_prompt_clock() -> String {
    format!(
        "# prepends UTC-time to prompt, colored in red — unless it is there already, as it is after sourcing again or in a nested shell\n\
         case $PS1 in *'{CLOCK}'*) ;; *) export PS1=\"\\[\\033[01;31m\\][\\{CLOCK}$PS1\" ;; esac\n"
    )
}

/// Fish's prompt clock: the same time, in front of whatever prompt fish draws. Fish draws its
/// prompt with the `fish_prompt` function rather than a string, so the clock wraps that function,
/// keeping the original under [`fish_original`]'s name to call.
///
/// Wrapped at the first prompt, by a handler of fish's `fish_prompt` event that removes itself —
/// not as this file is sourced: fish runs conf.d first, so a prompt that `config.fish` sets up
/// would replace a wrapper made then, clock and all. Once only, so sourcing the file again never
/// wraps the wrapper. The original prompt runs first, its output kept: running anything before it
/// would hand it the status of that instead of the last command's, which a prompt shows.
fn fish_prompt_clock() -> String {
    let original = fish_original("fish_prompt");
    format!(
        "# prepends UTC-time to prompt, colored in red. Fish draws its prompt with the fish_prompt function, not PS1, so the clock\n\
         # wraps that — at the first prompt, so that a prompt config.fish sets up (it runs after this file) is wrapped too\n\
         function __bashrs_prompt_clock --on-event fish_prompt\n    \
         functions -e __bashrs_prompt_clock\n    \
         functions -q {original}; and return  # wrapped already: this file was sourced again\n    \
         functions --copy fish_prompt {original}; or return\n    \
         function fish_prompt -d 'The UTC time in red, then the prompt it was put in front of'\n        \
         set -l prompt ({original} | string collect -N -a)  # first, so it sees the last command's $status and $pipestatus\n        \
         set_color --bold red; printf '[%s(UTC)]' (date -u +%T); set_color normal\n        \
         printf '%s' $prompt\n    \
         end\n\
         end\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_active_exports() {
        let s = settings(Shell::Bash);
        assert!(s.contains("export GREP_COLORS='mt=7;31'"));
        assert!(s.contains("export HISTTIMEFORMAT=") && s.contains("%F_%T"));
        assert!(s.contains("Makes \"history\" command")); // comment quotes survive
        assert!(s.contains("export PS1=") && s.contains("(UTC)"));
    }

    #[test]
    fn comments_are_aligned_into_a_column() {
        let columns: Vec<usize> = bash_exports().lines().filter_map(|line| line.find("  # ")).collect();
        assert!(columns.len() >= 2, "expected several commented exports");
        assert!(columns.windows(2).all(|w| w[0] == w[1]), "comment columns not aligned: {columns:?}");
    }

    #[test]
    fn emits_only_live_exports() {
        // Commented-out options live in the source, not the output — every emitted line is
        // an active `export`.
        for line in bash_exports().lines() {
            assert!(line.starts_with("export "), "unexpected non-export line: {line}");
        }
    }

    /// Bash itself evaluates the clock line, twice over: a prompt without the clock gains it, and
    /// one that has it — sourced again, or inherited by a nested shell — does not gain a second.
    #[test]
    fn the_bash_clock_is_prepended_once() {
        let clock = bash_prompt_clock();
        let script = format!("PS1='$ '\n{clock}{clock}printf '%s' \"$PS1\"");
        let out = std::process::Command::new("bash").args(["--norc", "-c", &script]).output().expect("bash");
        let ps1 = String::from_utf8_lossy(&out.stdout);
        assert_eq!(ps1, "\\[\\033[01;31m\\][$(date -u +%T)(UTC)]$ ", "one clock, then the prompt it was put in front of");
    }

    /// Fish gets the variables it shares with bash, and for each bash-only one the comment saying
    /// what fish does instead — never the variable itself, which would sit inert there.
    #[test]
    fn fish_gets_the_shared_exports_and_the_bash_only_ones_as_comments() {
        let s = fish_exports();
        assert!(s.contains("set -gx GREP_COLORS 'mt=7;31'  # sets grep marking"), "{s}");
        assert!(s.contains("# HISTTIMEFORMAT: fish stamps every history entry itself"), "{s}");
        assert!(!s.contains("set -gx HISTTIMEFORMAT") && !s.contains("PS1"), "bash-only rows stay out: {s}");
    }

    /// The fish clock wraps whatever prompt is in place at the first prompt, once, and runs that
    /// prompt before anything else; tests/sourcefile_fish.rs drives it through a real fish.
    #[test]
    fn the_fish_clock_wraps_the_prompt_at_the_first_prompt() {
        let s = fish_prompt_clock();
        assert!(s.contains("function __bashrs_prompt_clock --on-event fish_prompt\n"), "{s}");
        assert!(s.contains("    functions -e __bashrs_prompt_clock\n"), "the handler runs once: {s}");
        assert!(s.contains("functions -q __bashrs_original_fish_prompt; and return"), "never wraps the wrapper: {s}");
        assert!(s.contains("functions --copy fish_prompt __bashrs_original_fish_prompt; or return"), "{s}");
        let body: Vec<&str> = s.lines().skip_while(|line| !line.contains("function fish_prompt")).skip(1).collect();
        assert!(body[0].trim_start().starts_with("set -l prompt (__bashrs_original_fish_prompt"), "the original first: {s}");
        assert!(s.contains("printf '[%s(UTC)]' (date -u +%T)"), "the same clock as bash's: {s}");
    }

    /// The condition `InFish::Same` rests on: a value spelled identically for both shells. Bash's
    /// `'…'` and fish's agree exactly when there is no backslash or quote inside.
    #[test]
    fn every_row_fish_shares_is_spelled_the_same_in_both_shells() {
        for e in EXPORTS.iter().filter(|e| matches!(e.fish, InFish::Same)) {
            let inner = e.value.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\''));
            let inner = inner.unwrap_or_else(|| panic!("{}'s value must be single-quoted to share it with fish", e.var));
            assert!(!inner.contains(['\\', '\'']), "{}'s value would read differently in fish", e.var);
        }
    }
}
