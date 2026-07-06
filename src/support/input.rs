//! Reading a command's text input from whichever source is handiest — an inline argument,
//! a file, or stdin. The "text, a file, or a pipe" convenience shared by text-consuming
//! commands (e.g. the `highlight` style command), so each doesn't reinvent it.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

/// Resolve a command's text input:
/// - an `arg` naming an existing file is read as that file,
/// - any other non-empty `arg` is taken as literal text,
/// - and no `arg` (or `"-"`) reads stdin.
pub(crate) fn read_input(arg: Option<&str>) -> io::Result<String> {
    match arg {
        Some(path) if Path::new(path).is_file() => fs::read_to_string(path),
        None | Some("-") => {
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf)?;
            Ok(buf)
        }
        Some(text) => Ok(text.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_file_is_read() {
        let cargo_toml = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        assert!(read_input(Some(cargo_toml)).unwrap().contains("[package]"));
    }

    #[test]
    fn inline_text_passes_through() {
        assert_eq!(read_input(Some("let x = 5;")).unwrap(), "let x = 5;");
    }

    #[test]
    fn a_nonexistent_path_is_treated_as_text() {
        assert_eq!(read_input(Some("/no/such/file.xyz")).unwrap(), "/no/such/file.xyz");
    }
    // stdin (`None` / `"-"`) is exercised by live use, not unit tests.
}
