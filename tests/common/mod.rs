#![allow(dead_code)] // each test file compiles this module and uses its own part of it
//! What the tests that start shells share: a shell started in a session of its own — as a shell
//! started from another by default, or as a terminal's first — and without the bashrs session of
//! whoever runs the tests; what it printed, kept exactly for the assertions and made safe for a
//! failing one to show; and a scratch `HOME` with bashrs installed in it ([`Home`]), which each
//! shell's suite runs its shell under.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

/// A generated sourcefile, as `bashrs generate --shell <shell>` prints it.
pub fn generated(shell: &str) -> Vec<u8> {
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs")).args(["generate", "--shell", shell]).output().expect("generate");
    assert!(out.status.success(), "generate --shell {shell} failed");
    out.stdout
}

/// A stand-in for the installed binary: enough behaviour to watch a wrapper work — `hg` echoes its
/// stdin, `brs_compile` exits with `$FAKE_EXIT` — and silent otherwise. The commands a test needs
/// for real are handed to the real binary by `exec`, which leaves the shell it was called from as
/// its parent, as the installed binary would have it.
fn stand_in() -> String {
    format!(
        "#!/bin/sh\ncase \"$1\" in\n\
         \x20 hg) exec cat ;;\n\
         \x20 brs_compile) exit \"${{FAKE_EXIT:-0}}\" ;;\n\
         \x20 complete-flags|brs_sourcefile|shell_def|shell_args_print|lll|cd-listing) exec '{}' \"$@\" ;;\n\
         esac\n",
        env!("CARGO_BIN_EXE_bashrs")
    )
}

/// A scratch `HOME` holding both generated sourcefiles and — unless the test is about its absence —
/// the stand-in binary at `.bashrs/bashrs`. Removed when dropped, so a failing assertion leaves
/// nothing behind. Each shell's suite adds the methods that run its shell here.
pub struct Home {
    pub dir: PathBuf,
}

impl Home {
    pub fn new(tag: &str) -> Self {
        let home = Self::without_binary(tag);
        let binary = home.dir.join(".bashrs/bashrs");
        std::fs::write(&binary, stand_in()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        home
    }

    pub fn without_binary(tag: &str) -> Self {
        let home = Self::empty(tag);
        std::fs::write(home.dir.join(".bashrs/sourcefile.fish"), generated("fish")).unwrap();
        std::fs::write(home.dir.join(".bashrs/sourcefile.sh"), generated("bash")).unwrap();
        home
    }

    /// Just the directory and its `.bashrs` — for the install to fill.
    pub fn empty(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("bashrs_home_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".bashrs")).unwrap();
        Home { dir }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `text`'s lines, for comparing a shell's output line by line.
pub fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

/// `program` as a `Command` for a shell under test: in a session of its own, led by an `sh` above
/// it, with the variables a bashrs-armed session exports removed, and the tools directory such a
/// session puts on `PATH`. Arguments added to it are `program`'s.
///
/// The session is the part that keeps the terminal running the tests alive. An interactive bash
/// whose stderr is no terminal opens `/dev/tty` — that terminal — and puts itself in front of it
/// for job control, giving it back on exit to whichever group it found there. Run in parallel, one
/// shell finds another's group in front, and gives the terminal back to it once it is gone: the
/// shell that started the tests is left behind a group that no longer exists, unable to read its
/// own terminal. A new session has no controlling terminal, so there is none to take.
///
/// The `sh` leading it makes the shell one started from another, as a shell in a terminal is but
/// the first: the sourcefiles greet only in a session's leader, and a greeting would be in every
/// test's output. [`leading_shell`] starts the one a terminal starts first, for the greeting's
/// tests. (`; exit $?` keeps `sh` from running `program` in its own place, leading the session.)
///
/// The variables are the other half of starting fresh: a test that sources the sourcefile must not
/// find it sourced already, as it is in a terminal where bashrs is installed — `GREP_COLORS` already
/// set, the tools directory already on `PATH`.
pub fn shell(program: &str) -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", "\"$@\"; exit $?", "sh", program]);
    fresh_session(command)
}

/// `program` as [`shell`] starts it, but leading its session itself, as a terminal's first shell
/// does — the one shell the sourcefiles greet in.
pub fn leading_shell(program: &str) -> Command {
    fresh_session(Command::new(program))
}

/// `command` in a session of its own, without the bashrs session of whoever runs the tests.
fn fresh_session(mut command: Command) -> Command {
    // SAFETY: `setsid` is async-signal-safe, which is all a `pre_exec` hook may require.
    unsafe {
        command.pre_exec(|| match libc::setsid() {
            -1 => Err(std::io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
    for variable in ["GREP_COLORS", "HISTTIMEFORMAT", "PS1", "_BASHRS_BARE"] {
        command.env_remove(variable);
    }
    if let Some(path) = std::env::var_os("PATH") {
        let kept = std::env::split_paths(&path).filter(|dir| !dir.ends_with(".bashrs/tools/bin"));
        command.env("PATH", std::env::join_paths(kept).expect("PATH entries rejoin as they came"));
    }
    command
}

/// What a shell printed: the exact text, for the assertions — through `Deref`, as a `str` — and
/// printable for a failing one to show. `Display` writes every control character but a newline or
/// a tab as its escape (`\u{1b}`), because a shell prints terminal controls — bracketed paste,
/// keyboard protocols, a title — that would otherwise switch the terminal running the tests into
/// modes its own shell does not expect.
pub struct Captured(String);

impl Captured {
    pub fn new(bytes: &[u8]) -> Self {
        Captured(String::from_utf8_lossy(bytes).into_owned())
    }

    /// The text with its colour and terminal escapes removed — what a reader of the terminal sees.
    pub fn plain(&self) -> Captured {
        Captured(console::strip_ansi_codes(&self.0).into_owned())
    }
}

impl std::ops::Deref for Captured {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Captured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for ch in self.0.chars() {
            match ch {
                '\n' | '\t' => write!(f, "{ch}")?,
                ch if ch.is_control() => write!(f, "{}", ch.escape_default())?,
                ch => write!(f, "{ch}")?,
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for Captured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}
