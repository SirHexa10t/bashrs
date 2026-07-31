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

use crate::tools;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Run the bundled ffmpeg and pass its exit code back.
///
/// The one place `media` launches ffmpeg, because of the stdin handling. ffmpeg reads stdin for
/// interactive keys (`q` to quit) and briefly puts the terminal into raw mode — `ECHO`, `ICANON`,
/// `IXON` and friends off — restoring what it saved on exit. One ffmpeg is invisible; two
/// overlapping interleave, the second saving the *already-raw* state and restoring that, and the
/// terminal is left with no echo until `stty sane`. Both halves of the answer live here: a null
/// stdin (which also covers any ffmpeg a child spawns) and `-nostdin` in the argv, which says the
/// same thing somewhere a reader can see it.
///
/// NOT folded into [`crate::support::exec`]: its runners deliberately hand the terminal to the
/// child, because they also launch editors, git and package managers that must be able to prompt.
/// This is an ffmpeg rule, so it lives with the ffmpeg callers. `ffprobe` needs neither half — it
/// never reads stdin, rejects `-nostdin` outright, and is only ever run through a capturing
/// runner (which nulls stdin already).
fn _run_ffmpeg(argv: Vec<OsString>) -> i32 {
    let mut full: Vec<OsString> = vec!["-nostdin".into()];
    full.extend(argv);
    match std::process::Command::new(tools::resolve("ffmpeg"))
        .stdin(std::process::Stdio::null())
        .args(full)
        .status()
    {
        Ok(status) => status.code().unwrap_or(1),
        Err(err) => {
            eprintln!("could not run ffmpeg: {err}");
            1
        }
    }
}

/// The tail every ffmpeg-writing command shares: refuse writing onto the input itself, run,
/// and pass ffmpeg's exit code through — it keeps ignorable warnings out of its status (a
/// warned-but-clean run exits 0), so its code is the honest signal for chaining scripts.
fn _run_writing(command: &str, input: &Path, output: &Path, argv: Vec<OsString>) {
    if output == input {
        eprintln!("{command}: the output is the input itself ({})", input.display());
        std::process::exit(1);
    }
    let code = _run_ffmpeg(argv);
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
