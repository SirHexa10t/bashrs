//! Media commands (`media_*`) — local operations on audio, video and image files, each driven by
//! the bundled `ffmpeg`/`ffprobe` ([`crate::tools::resolve`]) through [`crate::support::exec`].
//!
//! One product per module, each its own `#[category]` block and clap enum, all flattened under the
//! single `media` group by [`crate::cli`]: [`transcode`] (convert + trim), [`metadata`] (report),
//! [`audio_fx`] (vocal removal), [`images`] (canvas merges), [`correction`] (in-place repairs).
//! They share nothing but the file-writing helpers below, the [`OverwriteArgs`] flag every
//! file-writing command takes, and — for the two that rewrite a file's streams, the trim and the
//! correction — `streams`, the stream probe and Matroska's cover rules. So a new product is a new
//! module here, and a new command joins the module it belongs to without touching the others.

pub mod audio_fx;
pub mod correction;
pub mod images;
pub mod metadata;
mod streams;
pub mod transcode;

use crate::tools;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The flag every media command writing a file takes, flattened into its arguments as `overwrite`
/// — so a command reads it as `overwrite.yes`.
#[derive(clap::Args)]
pub struct OverwriteArgs {
    /// Overwrite output file if it exists
    #[arg(short = 'y', long = "overwrite")]
    pub yes: bool,
}

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
    let mut ffmpeg = std::process::Command::new(tools::resolve("ffmpeg"));
    // SVT-AV1 writes its banner and configuration to stderr whatever ffmpeg's `-v` says — twenty
    // lines on every AV1 encode. 2 keeps its warnings and errors; a level set by hand stands.
    if std::env::var_os("SVT_LOG").is_none() {
        ffmpeg.env("SVT_LOG", "2");
    }
    match ffmpeg.stdin(std::process::Stdio::null()).args(full).status() {
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
/// Without `-y` it refuses an existing output too, before ffmpeg runs: the one failure
/// ffmpeg's code hides ([`_refuse_existing`]).
fn _run_writing(command: &str, input: &Path, output: &Path, overwrite: bool, argv: Vec<OsString>) {
    _run_writing_then(command, input, output, overwrite, argv, || {});
}

/// [`_run_writing`], running `tidy` once the writing is over, whichever way it ended — refused,
/// failed, or saved. What a command made for ffmpeg to read (the trim's extracted covers) must not
/// outlive the run, and every end but saving leaves the process on the spot.
fn _run_writing_then(command: &str, input: &Path, output: &Path, overwrite: bool, argv: Vec<OsString>, tidy: impl FnOnce()) {
    let refusal = if output == input {
        Some(format!("{command}: the output is the input itself ({})", input.display()))
    } else {
        _refuse_existing(output, overwrite).err().map(|why| format!("{command}: {why}"))
    };
    if let Some(refusal) = refusal {
        tidy();
        eprintln!("{refusal}");
        std::process::exit(1);
    }
    let code = _run_ffmpeg(argv);
    tidy();
    if code != 0 {
        std::process::exit(code);
    }
    _report_saved(output.to_owned());
}

/// Refuse to write onto an existing `output` unless `-y` was given — checked here, because
/// ffmpeg's own refusal can't stand in for it: ffmpeg says "already exists. Exiting." and exits
/// 0 (n9.0, and with `-n` too), which a caller can only read as success, reporting a file it
/// never wrote as saved.
fn _refuse_existing(output: &Path, overwrite: bool) -> Result<(), String> {
    if !overwrite && output.exists() {
        return Err(format!("'{}' already exists — pass -y/--overwrite to replace it", output.display()));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `-y` and `--overwrite` are one flag, off unless given — the spelling every file-writing
    /// media command answers to, since each flattens this one.
    #[test]
    fn overwrite_is_off_unless_asked_for_in_either_spelling() {
        #[derive(clap::Parser)]
        struct Cli {
            #[command(flatten)]
            overwrite: OverwriteArgs,
        }
        use clap::Parser;
        let yes = |argv: &[&str]| {
            Cli::try_parse_from(std::iter::once("media").chain(argv.iter().copied())).expect("parses").overwrite.yes
        };
        assert!(!yes(&[]), "off unless given");
        assert!(yes(&["-y"]) && yes(&["--overwrite"]));
    }

    /// Without `-y` an existing output is refused, naming the file and the way out; with `-y`,
    /// or with nothing there yet, the write goes ahead.
    #[test]
    fn an_existing_output_is_refused_unless_overwrite_is_given() {
        let dir = std::env::temp_dir().join(format!("bashrs_refuse_existing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let taken = dir.join("taken.mp4");
        std::fs::write(&taken, b"x").unwrap();

        let refusal = _refuse_existing(&taken, false).unwrap_err();
        assert!(refusal.contains(&taken.display().to_string()) && refusal.contains("-y/--overwrite"), "{refusal}");
        assert_eq!(_refuse_existing(&taken, true), Ok(()));
        assert_eq!(_refuse_existing(&dir.join("free.mp4"), false), Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
