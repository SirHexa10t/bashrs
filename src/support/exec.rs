//! Running external programs with consistent success/failure reporting.
//!
//! Shared by the categories that shell out (e.g. [`crate::categories::media`],
//! [`crate::categories::packages`]) so the run-and-report wording lives in one place.

use std::ffi::OsStr;
use std::process::{Command, Stdio};

/// Run `program` with `args`, inheriting stdio. Returns whether it succeeded,
/// printing a consistent diagnostic to stderr otherwise.
pub(crate) fn run_reporting<I, S>(program: &str, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    match Command::new(program).args(args).status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("{program} failed with status: {status}");
            false
        }
        Err(err) => {
            eprintln!("could not run {program}: {err}");
            false
        }
    }
}

/// Run `program` with `args`, discarding all output. Returns whether it exited
/// successfully — for capability probes (does this subcommand work here?) that
/// must stay silent.
pub(crate) fn succeeds_quietly<I, S>(program: &str, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Run `program` with `args`, capturing stdout for post-processing; the program's own
/// stderr passes straight through to the user. Returns `Some(stdout)` on success, or
/// `None` on a non-zero exit or a spawn failure (the latter with a diagnostic).
pub(crate) fn capture_stdout<I, S>(program: &str, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    match Command::new(program).args(args).output() {
        Ok(out) => {
            if !out.stderr.is_empty() {
                eprint!("{}", String::from_utf8_lossy(&out.stderr));
            }
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        }
        Err(err) => {
            eprintln!("could not run {program}: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_reporting_maps_exit_status_to_a_bool() {
        assert!(run_reporting("true", &[] as &[&str]), "exit 0 should report success");
        assert!(!run_reporting("false", &[] as &[&str]), "non-zero exit should report failure");
    }

    #[test]
    fn run_reporting_reports_a_missing_program_as_failure() {
        assert!(!run_reporting("bashrs_no_such_program_xyz", &[] as &[&str]));
    }

    #[test]
    fn succeeds_quietly_maps_exit_status_to_a_bool() {
        assert!(succeeds_quietly("true", &[] as &[&str]));
        assert!(!succeeds_quietly("false", &[] as &[&str]));
        assert!(!succeeds_quietly("bashrs_no_such_program_xyz", &[] as &[&str]));
    }

    #[test]
    fn capture_stdout_returns_output_on_success_and_none_on_failure() {
        assert_eq!(capture_stdout("echo", ["hello"]).as_deref(), Some("hello\n"));
        assert_eq!(capture_stdout("false", &[] as &[&str]), None);
        assert_eq!(capture_stdout("bashrs_no_such_program_xyz", &[] as &[&str]), None);
    }
}
