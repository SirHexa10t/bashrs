//! The shells bashrs writes its sourced file for, and the one shape hand-written shell takes so
//! that no shell can be forgotten.
//!
//! Most of the generated surface is derived — every wrapper comes from clap's model — and needs no
//! per-shell authoring: [`crate::cli`]'s generator renders it in each [`Shell`]'s syntax. What is
//! written by hand is a handful of fragments: a command's `#[shell_body]`, a `#[piped]` probe, a
//! contributor's setup lines. Each is a [`ShellText`], which cannot be built without saying what
//! fish does, even when the answer is that fish has no equivalent — and then the reason, which is
//! printed for the person who reaches for the command there.
//!
//! [`fish_function`] is here for the same reason: every layer that contributes fish text writes
//! functions, and they share one layout — one function per line, a block aligned into a table at
//! [`FISH_DESCRIPTION`], as bash's are. So does [`fish_original`], the name under which fish's own
//! definition is kept when bashrs wraps it.
//!
//! [`Shell::caller`] is the other direction: not which file is being written, but which shell a
//! command is running under — for the commands that show the user something in their own shell's
//! syntax.

use crate::support::shell_quote;

/// A shell bashrs writes a sourcefile for. Everything that emits shell text branches on this, so
/// a new shell fails to compile until each of those places knows how to write it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Shell {
    /// Bash — and zsh, which sources the same file and skips its bash-only blocks.
    Bash,
    Fish,
}

impl Shell {
    /// `s` as one literal word of this shell, spelled as its generated file spells literals. Bash's
    /// are always quoted, so an empty or spaced value is visible at a glance; fish's are quoted only
    /// where a word needs it, as `fish_indent` decides that. See [`shell_quote`] for each dialect.
    pub fn quote(self, s: &str) -> String {
        match self {
            Shell::Bash => shell_quote::quote(s),
            Shell::Fish => shell_quote::fish_word(s),
        }
    }

    /// The shell this process was started from — the syntax to show the user wherever a command
    /// prints something for their own shell: its sourcefile, a name to paste back. Read off the
    /// parent process, which is that shell: every generated function runs the binary as its shell's
    /// direct child (in a pipeline, bash runs the function in a child bash; fish runs it in-process).
    /// Bash for anything that is not fish — zsh, which sources bash's file, a script, or a `/proc`
    /// that cannot be read.
    ///
    /// The process's name (`comm`) rather than its executable: when a package upgrade replaces the
    /// shell's binary under a running session, `/proc/<pid>/exe` reads `/usr/bin/fish (deleted)`,
    /// while the name stays `fish`.
    pub fn caller() -> Shell {
        let parent = std::os::unix::process::parent_id();
        Shell::named(&std::fs::read_to_string(format!("/proc/{parent}/comm")).unwrap_or_default())
    }

    /// The shell a process name (as `/proc/<pid>/comm` holds it, newline and all) is: fish exactly
    /// when the name is fish's.
    fn named(process: &str) -> Shell {
        match process.trim_end() {
            "fish" => Shell::Fish,
            _ => Shell::Bash,
        }
    }
}

/// One hand-written piece of shell, in each shell's own syntax — or, for a shell that cannot
/// express it, the reason why not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShellText {
    bash: &'static str,
    fish: Result<&'static str, &'static str>,
}

impl ShellText {
    /// A piece both shells can express.
    pub const fn new(bash: &'static str, fish: &'static str) -> Self {
        ShellText { bash, fish: Ok(fish) }
    }

    /// A piece only bash can express. `why` is shown to whoever runs the command in fish, so it is
    /// written for them: what fish lacks, and what to use there instead.
    pub const fn bash_only(bash: &'static str, why: &'static str) -> Self {
        ShellText { bash, fish: Err(why) }
    }

    /// The piece in bash — which every piece has.
    pub fn bash(&self) -> &'static str {
        self.bash
    }

    /// The piece in fish, or the reason fish has none.
    pub fn fish(&self) -> Result<&'static str, &'static str> {
        self.fish
    }
}

/// A fish function on one line — `function NAME -d 'DESCRIPTION'; BODY; end`, without the line's
/// newline — the form the fish sourcefile gives every command, so that a section reads as a table,
/// as bash's does: aligned at [`FISH_DESCRIPTION`], the descriptions start in one column. The
/// description comes first because fish takes it only there, and fish shows it beside the name
/// when completing commands. Each body element is one statement — a block's `if …`, `else` and
/// `end` included — joined with `; `.
pub fn fish_function<S: AsRef<str>>(name: &str, description: Option<&str>, body: &[S]) -> String {
    let mut out = format!("function {name}");
    if let Some(description) = description.filter(|description| !description.is_empty()) {
        out += &format!(" {FISH_DESCRIPTION}{}", Shell::Fish.quote(description));
    }
    for statement in body {
        out += "; ";
        out += statement.as_ref();
    }
    out + "; end"
}

/// Where a [`fish_function`]'s description starts — the token a section's lines are aligned at,
/// which a name (no spaces in it) can never contain, so the first one in a line is the description's.
pub const FISH_DESCRIPTION: &str = "-d ";

/// The name fish's own definition of `name` is kept under, where bashrs puts a function of its own
/// in front of it and still calls it — the `cd` override, the prompt clock.
pub fn fish_original(name: &str) -> String {
    format!("__bashrs_original_{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fish_function_is_one_line_with_its_description_first() {
        assert_eq!(
            fish_function("shell_new", Some("Start a fresh shell session"), &["exec fish"]),
            "function shell_new -d 'Start a fresh shell session'; exec fish; end"
        );
        assert_eq!(
            fish_function("x", None, &["if a", "b", "else", "c", "end"]),
            "function x; if a; b; else; c; end; end",
            "a block's own keywords are statements like any other"
        );
        assert_eq!(fish_function("x", Some(""), &["a"]), "function x; a; end", "an empty description is none");
        assert!(fish_function("x", Some("the project's root"), &["a"]).contains(r"-d 'the project\'s root';"));
    }

    /// A shell is told by its process name — fish's alone means fish; every other name, an absent
    /// one included, means the bash dialect bashrs has always printed.
    #[test]
    fn the_calling_shell_is_fish_only_where_the_parent_is_named_fish() {
        assert_eq!(Shell::named("fish\n"), Shell::Fish);
        assert_eq!(Shell::named("bash\n"), Shell::Bash);
        assert_eq!(Shell::named("zsh\n"), Shell::Bash, "zsh sources bash's file");
        assert_eq!(Shell::named("cargo\n"), Shell::Bash);
        assert_eq!(Shell::named(""), Shell::Bash, "an unreadable /proc");
        assert_eq!(Shell::named("fishy\n"), Shell::Bash, "the whole name, not a prefix");
    }

    #[test]
    fn a_piece_answers_for_each_shell_or_says_why_not() {
        let both = ShellText::new("exec bash", "exec fish");
        assert_eq!((both.bash(), both.fish()), ("exec bash", Ok("exec fish")));
        let bash_only = ShellText::bash_only("declare -p x", "fish has no declare; try `set --show x`");
        assert_eq!(bash_only.bash(), "declare -p x");
        assert_eq!(bash_only.fish(), Err("fish has no declare; try `set --show x`"));
    }

    #[test]
    fn each_shell_quotes_in_its_own_dialect() {
        assert_eq!(Shell::Bash.quote("it's"), r"$'it\'s'");
        assert_eq!(Shell::Fish.quote("it's"), r"'it\'s'");
        assert_eq!(Shell::Bash.quote("/plain/path"), "'/plain/path'", "bash always quotes");
        assert_eq!(Shell::Fish.quote("/plain/path"), "/plain/path", "fish only where a word needs it");
    }
}
