#![allow(dead_code)] // each test file compiles this module and uses its own part of it
//! What the tests that start shells share: a shell started the way a new terminal would start one
//! — in a session of its own, and without the bashrs session of whoever runs the tests — and what
//! it printed, kept exactly for the assertions and made safe for a failing one to show.

use std::os::unix::process::CommandExt;
use std::process::Command;

/// `program` as a `Command` for a shell under test: in a session of its own, with the variables a
/// bashrs-armed session exports removed, and the tools directory such a session puts on `PATH`.
///
/// The session is the part that keeps the terminal running the tests alive. An interactive bash
/// whose stderr is no terminal opens `/dev/tty` — that terminal — and puts itself in front of it
/// for job control, giving it back on exit to whichever group it found there. Run in parallel, one
/// shell finds another's group in front, and gives the terminal back to it once it is gone: the
/// shell that started the tests is left behind a group that no longer exists, unable to read its
/// own terminal. A new session has no controlling terminal, so there is none to take.
///
/// The variables are the other half of starting fresh: a test that sources the sourcefile must not
/// find it sourced already, as it is in a terminal where bashrs is installed — `GREP_COLORS` already
/// set, the tools directory already on `PATH`.
pub fn shell(program: &str) -> Command {
    let mut command = Command::new(program);
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
