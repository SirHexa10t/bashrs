//! Environment-variable and prompt settings emitted into `sourcefile.sh`. Like
//! [`super::session`], this is raw shell (not a `#[category]`), but authored as a small
//! table — variable, shell-quoted value, and comment — that [`settings`] renders into
//! `export` lines. Options that aren't on by default are kept commented out *in this
//! table*, as a reminder to whoever edits it that they're there to switch on.

/// One `export`: the variable, its shell-quoted value (quotes and all, since single vs.
/// double quoting is significant), and a trailing comment (`""` for none).
struct Export {
    var: &'static str,
    value: &'static str,
    comment: &'static str,
}

const EXPORTS: &[Export] = &[
    // Uncomment to set your preferred editor:
    // Export { var: "EDITOR", value: "'vim'", comment: "" },
    // Export { var: "VISUAL", value: "'vim'", comment: "" },
    Export {
        var: "GREP_COLORS",
        value: "'mt=7;31'",
        comment: "sets grep marking for found strings to have red background (instead of being bold red)",
    },
    Export {
        var: "HISTTIMEFORMAT",
        value: r##""%F_%T  ""##,
        comment: r#"Makes "history" command display the time the command ran at. You can find more history settings in your own .bashrc file"#,
    },
    Export {
        var: "PS1",
        value: r##""\[\033[01;31m\][\$(date -u +%T)(UTC)]$PS1""##,
        comment: "prepends UTC-time to prompt, colored in red",
    },
];

/// Render the [`EXPORTS`] table into `export` lines.
pub fn settings() -> String {
    EXPORTS
        .iter()
        .map(|e| {
            let comment = if e.comment.is_empty() { String::new() } else { format!("  # {}", e.comment) };
            format!("export {}={}{comment}\n", e.var, e.value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_active_exports() {
        let s = settings();
        assert!(s.contains("export GREP_COLORS='mt=7;31'  # sets grep marking"));
        assert!(s.contains("export HISTTIMEFORMAT=") && s.contains("%F_%T"));
        assert!(s.contains("Makes \"history\" command")); // comment quotes survive
        assert!(s.contains("export PS1=") && s.contains("(UTC)"));
    }

    #[test]
    fn emits_only_live_exports() {
        // Commented-out options live in the source, not the output — every emitted line is
        // an active `export`.
        for line in settings().lines() {
            assert!(line.starts_with("export "), "unexpected non-export line: {line}");
        }
    }
}
