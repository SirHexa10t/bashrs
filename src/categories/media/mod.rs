//! Media commands (`media_*`) — local operations on audio, video and image files, each driven by
//! the bundled `ffmpeg`/`ffprobe` ([`crate::tools::resolve`]) through [`crate::support::exec`].
//!
//! One product per module, each its own `#[category]` block and clap enum, all flattened under the
//! single `media` group by [`crate::cli`]: [`transcode`] (convert + trim), [`metadata`] (report),
//! [`audio_fx`] (vocal removal), [`images`] (canvas merges). They share nothing but the two
//! file-writing helpers below — so a new product is a new module here, and a new command joins the
//! module it belongs to without touching the others.

pub mod audio_fx;
pub mod images;
pub mod metadata;
pub mod transcode;

use crate::support::exec::run_reporting_code;
use crate::tools;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The tail every ffmpeg-writing command shares: refuse writing onto the input itself, run,
/// and pass ffmpeg's exit code through — it keeps ignorable warnings out of its status (a
/// warned-but-clean run exits 0), so its code is the honest signal for chaining scripts.
fn _run_writing(command: &str, input: &Path, output: &Path, argv: Vec<OsString>) {
    if output == input {
        eprintln!("{command}: the output is the input itself ({})", input.display());
        std::process::exit(1);
    }
    let code = run_reporting_code(tools::resolve("ffmpeg"), argv);
    if code != 0 {
        std::process::exit(code);
    }
    _report_saved(output.to_owned());
}

/// Report a just-written output file by its canonical path.
fn _report_saved(path: PathBuf) {
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    println!("Saved: {}", path.display());
}

/// Argv as plain strings — the shape every product's `*_argv` test asserts against.
#[cfg(test)]
pub(crate) fn strs(argv: &[OsString]) -> Vec<String> {
    argv.iter().map(|a| a.to_string_lossy().into_owned()).collect()
}
