//! Reading a command's input from whichever source is handiest — an inline argument, a file, or
//! stdin. [`read_input_bytes`] returns the raw bytes (imposing no encoding, for the byte-oriented
//! grep commands, which match arbitrary streams as GNU grep does); [`read_input`] layers a UTF-8
//! check on top, for consumers that genuinely need text (e.g. the `code_highlight` style command).
//! The shared "text, a file, or a pipe" convenience, so each command doesn't reinvent it.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

/// Resolve a command's input to raw bytes:
/// - an `arg` naming an existing file is read as that file,
/// - any other non-empty `arg` is taken as literal text,
/// - and no `arg` (or `"-"`) reads stdin.
///
/// No encoding is imposed: a stream that isn't valid UTF-8 (a log with an embedded binary blob, say)
/// comes back intact, so a byte-oriented consumer like the grep commands searches it as GNU grep
/// would rather than rejecting the whole stream.
pub(crate) fn read_input_bytes(arg: Option<&str>) -> io::Result<Vec<u8>> {
    match arg {
        Some(path) if Path::new(path).is_file() => fs::read(path),
        None | Some("-") => {
            let mut buf = Vec::new();
            io::stdin().read_to_end(&mut buf)?;
            Ok(buf)
        }
        Some(text) => Ok(text.as_bytes().to_vec()),
    }
}

/// [`read_input_bytes`] decoded as UTF-8 — for consumers that need text (e.g. syntax highlighting),
/// where a non-UTF-8 stream is a genuine error rather than something to scan byte-wise.
pub(crate) fn read_input(arg: Option<&str>) -> io::Result<String> {
    String::from_utf8(read_input_bytes(arg)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
        assert_eq!(read_input_bytes(Some("let x = 5;")).unwrap(), b"let x = 5;");
    }

    #[test]
    fn a_nonexistent_path_is_treated_as_text() {
        assert_eq!(read_input(Some("/no/such/file.xyz")).unwrap(), "/no/such/file.xyz");
    }

    #[test]
    fn non_utf8_input_survives_the_byte_reader_but_the_text_reader_rejects_it() {
        // A file with an invalid UTF-8 byte in the middle: the byte reader returns it whole (so the
        // grep commands can scan it, as GNU grep does), while the text reader errors.
        let path = std::env::temp_dir().join(format!("bashrs_non_utf8_{}.bin", std::process::id()));
        std::fs::write(&path, b"before\xffafter").unwrap();
        let arg = path.to_str().unwrap();
        assert_eq!(read_input_bytes(Some(arg)).unwrap(), b"before\xffafter");
        assert!(read_input(Some(arg)).is_err());
        let _ = std::fs::remove_file(&path);
    }
    // stdin (`None` / `"-"`) is exercised by live use, not unit tests.
}
