//! The one aligner behind every hand-shaped block in the generated shell — bind comments,
//! the ALT+L `then` chain, the stainless flag arms, the style wrappers' `{` openers. Callers
//! name the token(s) that should start a column; nothing else about the mechanism varies, so
//! nothing else is a parameter.
//!
//! The mechanism is `table_formatter`'s own: a run of two-plus spaces is a column boundary.
//! [`align_columns`] guarantees that gap before each requested token, then lets the library do
//! the aligning — so this stays a thin adapter, not a second table engine.

/// Align `lines` into a table at each of `columns`: every line's FIRST occurrence of each token
/// gains the two-space gap `table_formatter` reads as a column boundary, then the block is
/// aligned as one table. Rows missing a token simply contribute no cell there, and
/// `trim_trailing` keeps them unpadded on the right.
///
/// Lines must arrive WITHOUT indentation — a leading run of spaces would read as an empty first
/// column — and the caller indents the result. A token already at a column boundary (or opening
/// the line) is left alone, so the function is idempotent.
pub(crate) fn align_columns(lines: Vec<String>, columns: &[&str]) -> Vec<String> {
    let padded: Vec<String> = lines.into_iter().map(|line| _pad(line, columns)).collect();
    let options = table_formatter::FormatOptions { trim_trailing: true, ..Default::default() };
    table_formatter::format_table(&padded, &options).unwrap_or(padded)
}

/// `line` with at least two spaces before each token's first occurrence (tokens in left-to-right
/// order, since each insertion shifts what follows).
fn _pad(mut line: String, columns: &[&str]) -> String {
    for token in columns {
        let Some(at) = line.find(token) else { continue };
        let spaces = line[..at].chars().rev().take_while(|ch| *ch == ' ').count();
        // A token that opens the line (spaces reaching position 0 included) heads no column —
        // there is nothing to its left to align against.
        if at > spaces && spaces < 2 {
            line.insert_str(at, if spaces == 0 { "  " } else { " " });
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
}
