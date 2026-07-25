//! Shared support for the commands backed by my other repos ([`crate::categories::comfy_repos`]) —
//! the pieces those commands need that something *else* needs too, so neither side owns them.
//!
//! Right now that's the `table_fancy` preset: the fancy table shape, defined once and used both by
//! the command itself and by [`crate::support::doc_render`], which renders the embedded templates'
//! markdown tables in that same shape. Helpers for the other wrapped tools (`table`, `backup_*`)
//! belong here too as they appear.

use table_formatter::{FormatOptions, RowSpacing};

/// The `table_fancy` shape at the live terminal width — `table -j " | " --split-lines
/// --space-rows '-' --emit-frame`: columns pipe-joined inside a border, wide rows wrapped to fit,
/// every record ruled off from the next.
///
/// `--split-lines` is a width, resolved by `table_formatter::terminal_width()` (`$COLUMNS`, else
/// the tty size, else 80), so the table fits the window it's printed into. Resolved per call: the
/// terminal can be resized between two runs of the same process.
pub(crate) fn table_fancy_options() -> FormatOptions {
    table_fancy_options_at(table_formatter::terminal_width())
}

/// The `table_fancy` shape at an explicit width — what `--split-lines` resolves to. Split out so a
/// caller can pin the width instead of probing the terminal (what the tests do, for output that
/// doesn't depend on the window it ran in).
pub(crate) fn table_fancy_options_at(width: usize) -> FormatOptions {
    FormatOptions {
        join_with: " | ".to_string(),      // -j " | "
        split_until_width: Some(width),    // --split-lines, already resolved to a width
        space_rows: RowSpacing::Fill('-'), // --space-rows '-' (always, not just where a row wraps)
        emit_frame: true,                  // --emit-frame
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preset_is_the_pipe_joined_framed_split_ruled_shape() {
        // The four flags it stands for, and nothing else: everything unnamed stays at the
        // library's default, so `table_fancy` and a bare `table` differ only by these.
        let opts = table_fancy_options_at(100);
        assert_eq!(opts.join_with, " | ", "-j \" | \"");
        assert!(opts.emit_frame, "--emit-frame");
        assert_eq!(opts.split_until_width, Some(100), "--split-lines, at the given width");
        assert!(matches!(opts.space_rows, RowSpacing::Fill('-')), "--space-rows '-'");
        let default = FormatOptions::default();
        assert_eq!(opts.divide_by, default.divide_by, "the input delimiter is left to the caller");
        assert_eq!(opts.sort, default.sort);
        assert_eq!(opts.trim_trailing, default.trim_trailing);
    }

    #[test]
    fn the_probed_width_is_the_libraries_terminal_width() {
        // `table_fancy_options` differs from the `_at` form only by resolving the width the way
        // --split-lines does — so the two stay in step if that precedence ever changes.
        assert_eq!(
            table_fancy_options().split_until_width,
            Some(table_formatter::terminal_width())
        );
    }

    #[test]
    fn a_wide_table_is_framed_and_split_to_the_width() {
        // The preset's payoff: a table too wide for the window wraps into it, every line framed.
        let rows = ["a | ".to_string() + &"long ".repeat(20), "b | short".to_string()];
        let out = table_formatter::format_table(&rows, &table_fancy_options_at(40)).unwrap();
        for line in &out {
            assert!(table_formatter::visible_len(line) <= 40, "over width: {line:?}");
            // A framed row, or one of the `-` rules the frame/row-spacing draws around records.
            let framed = line.starts_with('|') && line.ends_with('|');
            assert!(framed || line.starts_with('-'), "framed row or rule: {line:?}");
        }
        assert!(out.iter().any(|line| line.starts_with('|')), "framed rows present: {out:?}");
        assert!(out.len() > rows.len(), "the wide row wrapped onto extra lines: {out:?}");
    }
}
