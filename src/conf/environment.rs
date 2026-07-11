//! Environment-variable and prompt settings emitted into `sourcefile.sh`. Like
//! [`crate::categories::session`], this is raw shell (not a `#[category]`), but authored as a small
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

/// Render the [`EXPORTS`] table into `export` lines, aligning the trailing `#` comments with
/// the shared [`table_formatter`] helper. A 3-space code→comment delimiter plus a split
/// threshold of 3 let it align these without splitting a value like `HISTTIMEFORMAT="%F_%T  "`
/// on its interior double space (the default threshold of 2 would).
pub fn settings() -> String {
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
    // The fallback is unreachable: only `sort` can error, and it's off.
    let opts = table_formatter::FormatOptions {
        separator: 2,
        threshold: 3,
        trim_trailing: true,
        ..Default::default()
    };
    table_formatter::format_table(&lines, &opts)
        .unwrap_or(lines)
        .iter()
        .map(|line| format!("{line}\n"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_active_exports() {
        let s = settings();
        assert!(s.contains("export GREP_COLORS='mt=7;31'"));
        assert!(s.contains("export HISTTIMEFORMAT=") && s.contains("%F_%T"));
        assert!(s.contains("Makes \"history\" command")); // comment quotes survive
        assert!(s.contains("export PS1=") && s.contains("(UTC)"));
    }

    #[test]
    fn comments_are_aligned_into_a_column() {
        let columns: Vec<usize> = settings().lines().filter_map(|line| line.find("  # ")).collect();
        assert!(columns.len() >= 2, "expected several commented exports");
        assert!(columns.windows(2).all(|w| w[0] == w[1]), "comment columns not aligned: {columns:?}");
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
