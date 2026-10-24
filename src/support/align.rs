//! The one aligner behind every hand-shaped block in the generated shell — bind comments,
//! the ALT+L `then` chain, the stainless flag arms, the style wrappers' `{` openers. Callers
//! name the token(s) that should start a column; nothing else about the mechanism varies, so
//! nothing else is a parameter.
//!
//! The mechanism is `table_formatter`'s own. [`align_columns`] puts a column boundary before each
//! requested token, then lets the library do the aligning — so this stays a thin adapter, not a
//! second table engine.
//!
//! The boundary is [`DIVIDER`], not the library's default of two spaces: shell text holds two-space
//! runs of its own — `hg`'s fish probe asks for timestamps in `'%F_%T  '` — and with the default each
//! would be one more boundary, padded out to the column's width, inside the quotes.

/// The column boundary [`align_columns`] writes before each token: a character that never occurs
/// in shell text, spaced as `table_formatter` wants a divider with a visible core. Only it divides;
/// any run of spaces in a line is that line's own text.
pub(crate) const DIVIDER: &str = " \u{1f} ";

/// Align `lines` into a table at each of `columns`: every line's FIRST occurrence of each token
/// starts a column, and the block is aligned as one table, two spaces between columns. Rows
/// missing a token simply contribute no cell there, and `trim_trailing` keeps them unpadded on the
/// right; a line holding none of the tokens is no row at all — it comes back exactly as it went
/// in, and the table's widths are the other lines' alone.
///
/// Lines must arrive WITHOUT indentation, and the caller indents the result. The spaces before a
/// token are the gap the alignment sets, so aligned lines align again unchanged: the function is
/// idempotent.
pub(crate) fn align_columns(lines: Vec<String>, columns: &[&str]) -> Vec<String> {
    let mut divided: Vec<String> = lines.into_iter().map(|line| _divide(line, columns)).collect();
    // Only a line with a column is a row of the table; the rest are spliced back where they stood,
    // or a long one (a comment line, say) would set the first column's width for all of them.
    let rows: Vec<usize> = (0..divided.len()).filter(|&i| divided[i].contains(DIVIDER)).collect();
    let table: Vec<String> = rows.iter().map(|&i| std::mem::take(&mut divided[i])).collect();
    // `reasonable_spacing`: one monstrous row (shell_def's probe loop, at four times anyone
    // else's width) must not drag every neighbour's columns out to meet it — the outlier keeps
    // its head aligned and goes minimal from the offending column, and the rest of the block
    // snaps back to its own natural width.
    let options = table_formatter::FormatOptions {
        divide_by: DIVIDER.to_string(),
        join_with: "  ".to_string(),
        trim_trailing: true,
        reasonable_spacing: true,
        ..Default::default()
    };
    // The fallback is unreachable — only `sort` can error, and it's off — but it must not leave
    // a divider in the text.
    let aligned = table_formatter::format_table(&table, &options)
        .unwrap_or_else(|_| table.iter().map(|line| line.replace(DIVIDER, "  ")).collect());
    for (i, line) in rows.into_iter().zip(aligned) {
        divided[i] = line;
    }
    divided
}

/// `line` with a [`DIVIDER`] before each token's first occurrence, in place of the spaces there
/// (tokens in left-to-right order, since each insertion shifts what follows).
fn _divide(mut line: String, columns: &[&str]) -> String {
    for token in columns {
        let Some(at) = line.find(token) else { continue };
        let start = line[..at].trim_end_matches(' ').len();
        // A token that opens the line (spaces reaching position 0 included) heads no column —
        // there is nothing to its left to align against.
        if start > 0 {
            line.replace_range(start..at, DIVIDER);
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(ToString::to_string).collect()
    }

    /// The whole contract in one place: each named token starts a column, rows of different
    /// widths meet at it, and rows without the token stay untouched on the right.
    #[test]
    fn named_tokens_become_columns() {
        let aligned = align_columns(
            lines(&["short; then run", "a much longer condition; then go", "no marker here"]),
            &["then "],
        );
        let columns: Vec<Option<usize>> =
            aligned.iter().map(|line| line.find("then ")).collect();
        assert_eq!(columns[0], columns[1], "both `then`s share a column: {aligned:?}");
        assert_eq!(aligned[2], "no marker here", "a row without the token is left alone");
    }

    /// Several tokens, several columns — and insertion order can't corrupt later finds.
    #[test]
    fn multiple_tokens_align_independently() {
        let aligned = align_columns(
            lines(&[r#"ai) flags="-h" ;;"#, r#"quick_question) flags="-h --explain" ;;"#]),
            &["flags="],
        );
        let at: Vec<_> = aligned.iter().map(|line| line.find("flags=").unwrap()).collect();
        assert_eq!(at[0], at[1], "{aligned:?}");
    }

    /// Idempotent: aligning aligned output changes nothing — the gaps are already there.
    #[test]
    fn aligning_twice_is_aligning_once() {
        let once = align_columns(lines(&["a() { body; } # hi", "longer() { b; } # yo"]), &["{", "# "]);
        let twice = align_columns(once.clone(), &["{", "# "]);
        assert_eq!(once, twice);
    }

    /// A token at the head of a line has nothing to align against and must not gain a phantom
    /// empty first column.
    #[test]
    fn a_line_opening_token_is_not_padded() {
        let aligned = align_columns(lines(&["then alone", "x then paired"]), &["then "]);
        assert_eq!(aligned[0], "then alone");
    }

    /// Only the tokens start columns: a two-space run that is a line's own text — a timestamp
    /// format, a description — stays exactly as written, wherever it sits.
    #[test]
    fn a_line_s_own_runs_of_spaces_are_not_columns() {
        let aligned = align_columns(
            lines(&[
                "function g -d 'Search'; run g $argv; end",
                "function hg -d 'History'; history --show-time='%F_%T  ' | run hg; end",
            ]),
            &["-d "],
        );
        assert_eq!(aligned[1], "function hg  -d 'History'; history --show-time='%F_%T  ' | run hg; end");
        assert_eq!(aligned[0], "function g   -d 'Search'; run g $argv; end");
    }

    /// A line with none of the tokens is no row of the table: it comes back as it went in, in its
    /// place, however long — and the rows around it align as if it were not there.
    #[test]
    fn a_line_without_a_token_takes_no_part_in_the_table() {
        let aligned = align_columns(
            lines(&["a  # one", "a much much longer line  with no token at all", "bb # two"]),
            &["# "],
        );
        assert_eq!(aligned, ["a   # one", "a much much longer line  with no token at all", "bb  # two"]);
    }
}
