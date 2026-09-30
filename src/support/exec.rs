//! Running external programs with consistent output and status handling.
//!
//! Shared by the categories that shell out (e.g. [`crate::categories::media`],
//! [`crate::categories::packages`], [`crate::categories::git`], [`crate::categories::builtins`])
//! so the run / report / capture logic lives in one place.

use std::ffi::OsStr;
use std::process::{Command, Stdio};

/// Run `program` with `args`, inheriting stdio. Returns whether it succeeded,
/// printing a consistent diagnostic to stderr otherwise.
pub(crate) fn run_reporting<P, I, S>(program: P, args: I) -> bool
where
    P: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_reporting_code(program, args) == 0
}

/// Like [`run_reporting`], but yields the child's exit code so a wrapped tool's own status can be
/// passed through: 0 on success, the child's code on failure. Spawn errors and signal deaths —
/// which carry no code — report and yield 1 (a signal that kills the child usually reaches this
/// process too, so that fallback is all but unreachable).
pub(crate) fn run_reporting_code<P, I, S>(program: P, args: I) -> i32
where
    P: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = program.as_ref();
    match Command::new(program).args(args).status() {
        Ok(status) if status.success() => 0,
        Ok(status) => {
            eprintln!("{} failed with status: {status}", program.to_string_lossy());
            status.code().unwrap_or(1)
        }
        Err(err) => {
            eprintln!("could not run {}: {err}", program.to_string_lossy());
            1
        }
    }
}

/// Run `program` with `args`, discarding all output. Returns whether it exited
/// successfully — for capability probes (does this subcommand work here?) that
/// must stay silent.
pub(crate) fn succeeds_quietly<P, I, S>(program: P, args: I) -> bool
where
    P: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(program.as_ref())
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
pub(crate) fn capture_stdout<P, I, S>(program: P, args: I) -> Option<String>
where
    P: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = program.as_ref();
    match Command::new(program).args(args).output() {
        Ok(out) => {
            if !out.stderr.is_empty() {
                eprint!("{}", String::from_utf8_lossy(&out.stderr));
            }
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        }
        Err(err) => {
            eprintln!("could not run {}: {err}", program.to_string_lossy());
            None
        }
    }
}

/// Run `program` with `args` reading NO standard input, capturing stdout — and returning it
/// whatever the exit status. Both departures serve probes ([`crate::support::net`]): `openssl
/// s_client` forwards stdin to the socket and would sit waiting on the terminal forever without
/// `/dev/null`, and it exits non-zero on an unverifiable certificate while still printing the
/// certificate the report wants. `None` only on a spawn failure (reported).
pub(crate) fn capture_without_input<P, I, S>(program: P, args: I) -> Option<String>
where
    P: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = program.as_ref();
    match Command::new(program).args(args).stdin(std::process::Stdio::null()).output() {
        Ok(out) => Some(String::from_utf8_lossy(&out.stdout).into_owned()),
        Err(err) => {
            eprintln!("could not run {}: {err}", program.to_string_lossy());
            None
        }
    }
}

/// Run `program` with `args`, capturing stdout as raw bytes and returning it with the exit code,
/// whatever that code is — for tools whose non-zero exit is an answer rather than a failure: `git
/// diff --no-index` exits 1 exactly when its inputs differ. Stdin and stderr are inherited, so
/// the tool can read piped input (`-`) and its diagnostics reach the user as it prints them.
/// Bytes rather than text because the output is re-printed and must survive unchanged: a Latin-1
/// file's diff is not UTF-8. A signal death, which carries no code, is reported and yields 1 (as
/// in [`run_reporting_code`]); `None` only on a spawn failure (reported).
pub(crate) fn capture_with_code<P, I, S>(program: P, args: I) -> Option<(i32, Vec<u8>)>
where
    P: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = program.as_ref();
    match Command::new(program).args(args).stdin(Stdio::inherit()).stderr(Stdio::inherit()).output() {
        Ok(out) => {
            let code = out.status.code().unwrap_or_else(|| {
                eprintln!("{} failed with status: {}", program.to_string_lossy(), out.status);
                1
            });
            Some((code, out.stdout))
        }
        Err(err) => {
            eprintln!("could not run {}: {err}", program.to_string_lossy());
            None
        }
    }
}

// Elapsed-time profiling helpers — no callers in normal builds (hence the `allow(dead_code)`s);
// kept for the next latency hunt: `_stamp` marks a phase, `run_timed` swaps in for a
// `run_reporting_code`/`capture_stdout` call to timestamp a child's every output line.
#[allow(dead_code)]
fn _elapsed_s() -> f64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}
#[allow(dead_code)]
pub(crate) fn _stamp(label: &str) {
    eprintln!("[t+{:>7.3}s] {label}", _elapsed_s());
}
/// Stamp one line of a child's output (`|` marks it child output, vs a phase stamp).
#[allow(dead_code)]
fn _stamp_line(line: &str) {
    eprintln!("[t+{:>7.3}s] |   {line}", _elapsed_s());
}
/// Run `program`+`args` with both streams piped, stamping EVERY child output line with elapsed
/// time — so a subprocess's own phase lines reveal where the wall-time actually goes. Captures
/// stdout for the caller; returns `(exit_code, stdout)`.
#[allow(dead_code)]
pub(crate) fn run_timed(program: &OsStr, args: &[std::ffi::OsString]) -> Option<(i32, String)> {
    use std::io::{BufRead, BufReader};
    let mut child = match Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            eprintln!("could not run {}: {err}", program.to_string_lossy());
            return None;
        }
    };
    let stderr = child.stderr.take().expect("stderr piped");
    let err_thread = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            _stamp_line(&line);
        }
    });
    let stdout = child.stdout.take().expect("stdout piped");
    let mut captured = String::new();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        _stamp_line(&line);
        captured.push_str(&line);
        captured.push('\n');
    }
    let _ = err_thread.join();
    let code = child.wait().ok().and_then(|status| status.code()).unwrap_or(-1);
    Some((code, captured))
}

/// Run `program` with `args`, inheriting stdio; the exit status is ignored — for commands
/// run to show output or for a side effect, where a non-zero exit isn't a failure (e.g.
/// `ssh -T git@github.com`, which always exits 1). A spawn error is still reported.
pub(crate) fn run<P, I, S>(program: P, args: I)
where
    P: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = program.as_ref();
    if let Err(err) = Command::new(program).args(args).status() {
        eprintln!("could not run {}: {err}", program.to_string_lossy());
    }
}

/// Whether `program` is an executable found in any `PATH` directory — a dependency-free
/// `command -v` (checks the executable bit, not mere presence). Backs the package-manager
/// detection and the bundled-tools skip ([`crate::tools`]).
pub(crate) fn on_path(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            std::fs::metadata(dir.join(program))
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
    })
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
    fn run_reporting_code_passes_the_childs_exit_code_through() {
        assert_eq!(run_reporting_code("sh", ["-c", "exit 0"]), 0);
        assert_eq!(run_reporting_code("sh", ["-c", "exit 7"]), 7);
        assert_eq!(run_reporting_code("bashrs_no_such_program_xyz", &[] as &[&str]), 1);
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

    #[test]
    fn capture_with_code_returns_the_raw_output_with_any_exit_code() {
        // A non-zero exit is an answer, not a failure — and the bytes come back untouched, even
        // ones that are not UTF-8 (`\377` is a lone 0xFF).
        assert_eq!(capture_with_code("sh", ["-c", r"printf 'a\377'; exit 3"]), Some((3, b"a\xff".to_vec())));
        assert_eq!(capture_with_code("sh", ["-c", "exit 0"]), Some((0, Vec::new())));
        // A signal death carries no code of its own.
        assert_eq!(capture_with_code("sh", ["-c", "kill -9 $$"]).map(|(code, _)| code), Some(1));
        assert_eq!(capture_with_code("bashrs_no_such_program_xyz", &[] as &[&str]), None);
    }

    #[test]
    fn run_ignores_exit_status_without_panicking() {
        run("true", &[] as &[&str]);
        run("false", &[] as &[&str]); // non-zero exit ignored: no report, no panic
        run("bashrs_no_such_program_xyz", &[] as &[&str]); // spawn error: reported, no panic
    }

    #[test]
    fn on_path_finds_a_ubiquitous_binary_and_rejects_a_bogus_one() {
        assert!(on_path("sh"), "expected to find `sh` on PATH");
        assert!(!on_path("bashrs_no_such_program_xyz"));
    }
}
