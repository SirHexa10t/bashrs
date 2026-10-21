//! The shells bashrs writes its sourced file for, and the one shape hand-written shell takes so
//! that no shell can be forgotten.
//!
//! Most of the generated surface is derived — every wrapper comes from clap's model — and needs no
//! per-shell authoring: [`crate::cli`]'s generator renders it in each [`Shell`]'s syntax. What is
//! written by hand is a handful of fragments: a command's `#[shell_body]`, a `#[piped]` probe, a
//! contributor's setup lines. Each is a [`ShellText`], which cannot be built without saying what
//! fish runs.
//!
//! [`fish_function`] is here for the same reason: every layer that contributes fish text writes
//! functions, and they share one layout — one function per line, a block aligned into a table at
//! [`FISH_COLUMNS`], as bash's are. [`bash_function`] is bash's: the one form an alias cannot
//! break. So does [`fish_original`], the name under which fish's own definition is kept when
//! bashrs wraps it. And [`bash_or_zsh`], for the file bash and zsh share: where the two need
//! different words, it takes both.
//!
//! [`Shell::caller`] is the other direction: not which file is being written, but which shell a
//! command is running under — for the commands that show the user something in their own shell's
//! syntax.

use crate::support::shell_quote;

/// A shell bashrs writes a sourcefile for. Everything that emits shell text branches on this, so
/// a new shell fails to compile until each of those places knows how to write it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Shell {
    /// Bash — and zsh, which sources the same file, each skipping the other's blocks
    /// ([`bash_or_zsh`]).
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

/// One hand-written piece of shell, in each shell's own syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShellText {
    bash: &'static str,
    fish: &'static str,
}

impl ShellText {
    pub const fn new(bash: &'static str, fish: &'static str) -> Self {
        ShellText { bash, fish }
    }

    /// The piece in bash.
    pub fn bash(&self) -> &'static str {
        self.bash
    }

    /// The piece in fish.
    pub fn fish(&self) -> &'static str {
        self.fish
    }
}

/// A fish function on one line — `function NAME; BODY; end; functions -d 'DESCRIPTION' NAME`,
/// without the line's newline — the form the fish sourcefile gives every command, so that a section
/// reads as a table, as bash's does: aligned at [`FISH_COLUMNS`], name, body and description each
/// start a column. The description comes last, as bash's comment does, set by `functions -d` once
/// the function exists — fish shows it beside the name when completing commands, which it would not
/// for a comment. Each body element is one statement — a block's `if …`, `else` and `end` included —
/// joined with `; `.
pub fn fish_function<S: AsRef<str>>(name: &str, description: Option<&str>, body: &[S]) -> String {
    let mut out = format!("function {name}");
    for statement in body {
        out += "; ";
        out += statement.as_ref();
    }
    out += "; end";
    if let Some(description) = description.filter(|description| !description.is_empty()) {
        out += &format!("; {FISH_DESCRIPTION}{} {name}", Shell::Fish.quote(description));
    }
    out
}

/// The tokens a block of [`fish_function`]s is aligned at, by `align_columns`: the body's column
/// opens at the header's `;` — a line's first, since no name holds one — and the description's at
/// its `functions -d`. The `;` opens the body rather than closing the name because fish keeps a
/// function's source from after it: padding there would be part of the body, and `functions`,
/// `funced` and `shell_def` would show every body indented by its row's padding. Before the `;` it
/// is the header's, which fish writes anew whenever it prints a function back.
pub const FISH_COLUMNS: [&str; 2] = [";", FISH_DESCRIPTION];

/// Where a [`fish_function`]'s description starts: the statement that sets it, which no body runs —
/// so the first in a line is the description's.
const FISH_DESCRIPTION: &str = "functions -d ";

/// The name fish's own definition of `name` is kept under, where bashrs puts a function of its own
/// in front of it and still calls it — the `cd` override, the prompt clock.
pub fn fish_original(name: &str) -> String {
    format!("__bashrs_original_{name}")
}

/// A bash function on one line — `function NAME { BODY; }`, without the line's newline — the form
/// the bash sourcefile gives every command, which zsh reads too. The keyword, not `NAME()`, because
/// no alias reaches a name after `function`, in either shell; in `NAME()`, an alias of the name is
/// expanded first, and zsh refuses the definition — its parse error ends the sourcing there —
/// while bash defines the aliased command instead: `alias g=git` makes `g() {…}` define `git`.
/// Typed at the prompt, an alias still outranks any function, so the file sets the aliases of its
/// names aside before it defines them ([`SET_ASIDE_RECORD`]).
pub fn bash_function(name: &str, body: &str) -> String {
    format!("function {name} {{ {body}; }}")
}

/// The record of the aliases bash's file — zsh's too — set aside under bashrs's names, for
/// `shell_def` to show: an associative array, per name the line that brings its alias back
/// (`alias g='git'`). Fish's file sets nothing aside.
pub const SET_ASIDE_RECORD: &str = "__bashrs_set_aside";

/// A block of the file bash and zsh share, for a part the two write differently — the keybinds,
/// the greeting: `bash`'s lines run in bash and `zsh`'s in zsh, each behind a guard naming its
/// shell and indented under it. Zsh sources bash's file whole, so unguarded, either shell would
/// run the other's words. A shell with nothing to run gets no branch — bash rejects an empty one,
/// and with it the rest of the file.
pub fn bash_or_zsh(bash: &str, zsh: &str) -> String {
    let indented = |lines: &str| lines.lines().map(|line| format!("    {line}\n")).collect::<String>();
    let mut out = String::new();
    for (shell, lines) in [("BASH_VERSION", bash), ("ZSH_VERSION", zsh)] {
        if lines.trim().is_empty() {
            continue;
        }
        let keyword = if out.is_empty() { "if" } else { "elif" };
        out += &format!("{keyword} [ -n \"${shell}\" ]; then\n{}", indented(lines));
    }
    if !out.is_empty() {
        out += "fi\n";
    }
    out
}

/// A `printf` writing `text`, then a newline — one statement bash, zsh and fish all read alike,
/// `%b` turning the escapes in its argument back into the characters they spell. Each escape
/// character is spelled `\033` (and each backslash doubled), so that the file shows a styled
/// text's codes instead of holding them raw — which would style a listing of the file, as Alt+H
/// prints it.
pub fn printf_line(text: &str, shell: Shell) -> String {
    let spelled = text.replace('\\', "\\\\").replace('\x1b', "\\033");
    format!("printf '%b\\n' {}", shell.quote(&spelled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fish_function_is_one_line_with_its_description_last() {
        assert_eq!(
            fish_function("shell_new", Some("Start a fresh shell session"), &["exec fish"]),
            "function shell_new; exec fish; end; functions -d 'Start a fresh shell session' shell_new"
        );
        assert_eq!(
            fish_function("x", None, &["if a", "b", "else", "c", "end"]),
            "function x; if a; b; else; c; end; end",
            "a block's own keywords are statements like any other"
        );
        assert_eq!(fish_function("x", Some(""), &["a"]), "function x; a; end", "an empty description is none");
        assert!(fish_function("x", Some("the project's root"), &["a"]).ends_with(r"; functions -d 'the project\'s root' x"));
    }

    /// Each shell runs its own lines of a shared block and skips the other's — bash here, and zsh
    /// through the real file in tests/sourcefile_zsh.rs.
    #[test]
    fn a_shared_block_runs_bash_s_lines_in_bash_and_zsh_s_in_zsh() {
        let block = bash_or_zsh("echo bash one\necho bash two\n", "echo zsh");
        assert_eq!(
            block,
            "if [ -n \"$BASH_VERSION\" ]; then\n    echo bash one\n    echo bash two\nelif [ -n \"$ZSH_VERSION\" ]; then\n    echo zsh\nfi\n"
        );
        let out = std::process::Command::new("bash").args(["--norc", "-c", &block]).output().expect("bash");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "bash one\nbash two\n");
    }

    /// A shell with nothing to run gets no branch, never an empty one — which bash would reject,
    /// and the rest of the file with it.
    #[test]
    fn a_shell_with_nothing_to_run_gets_no_branch() {
        assert_eq!(bash_or_zsh("echo bash", ""), "if [ -n \"$BASH_VERSION\" ]; then\n    echo bash\nfi\n");
        assert_eq!(bash_or_zsh("", "echo zsh\n"), "if [ -n \"$ZSH_VERSION\" ]; then\n    echo zsh\nfi\n");
        assert_eq!(bash_or_zsh("", ""), "", "nothing for either shell, no block at all");
        let out = std::process::Command::new("bash").args(["--norc", "-c", &bash_or_zsh("", "echo zsh")]).output().expect("bash");
        assert!(out.status.success() && out.stdout.is_empty(), "{out:?}");
    }

    /// The keyword form, and what it is for: in bash, with an alias of the name in place, it still
    /// defines that name — where `NAME()` defines the aliased command instead. (Zsh's side of it
    /// runs through the real file in tests/sourcefile_zsh.rs.)
    #[test]
    fn a_bash_function_is_out_of_an_alias_s_reach() {
        assert_eq!(bash_function("g", r#"run g "$@""#), r#"function g { run g "$@"; }"#);
        let script = format!(
            "shopt -s expand_aliases; alias g=git\n{}\nunalias g\ntype -t git; g",
            bash_function("g", "echo defined as g")
        );
        let out = std::process::Command::new("bash").args(["--norc", "-c", &script]).output().expect("bash");
        let out = String::from_utf8_lossy(&out.stdout);
        assert!(!out.starts_with("function"), "`git` must stay what it was: {out}");
        assert!(out.ends_with("defined as g\n"), "{out}");
    }

    /// Aligned at its tokens, a block of fish functions is a table of three columns — name,
    /// `;`-opened body, description — and a function with no description simply ends at its body.
    #[test]
    fn fish_functions_align_into_name_body_and_description() {
        let block = crate::support::align::align_columns(
            vec![
                fish_function("g", Some("Search"), &["run g $argv"]),
                fish_function("shell_new", Some("Start a fresh shell session"), &["exec fish"]),
                fish_function("hop", None, &["cd .. $argv"]),
            ],
            &FISH_COLUMNS,
        );
        assert_eq!(
            block,
            [
                "function g          ; run g $argv; end;  functions -d Search g",
                "function shell_new  ; exec fish; end;    functions -d 'Start a fresh shell session' shell_new",
                "function hop        ; cd .. $argv; end",
            ]
        );
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
    fn a_piece_holds_each_shell_s_own_text() {
        let both = ShellText::new("exec bash", "exec fish");
        assert_eq!((both.bash(), both.fish()), ("exec bash", "exec fish"));
    }

    #[test]
    fn each_shell_quotes_in_its_own_dialect() {
        assert_eq!(Shell::Bash.quote("it's"), r"$'it\'s'");
        assert_eq!(Shell::Fish.quote("it's"), r"'it\'s'");
        assert_eq!(Shell::Bash.quote("/plain/path"), "'/plain/path'", "bash always quotes");
        assert_eq!(Shell::Fish.quote("/plain/path"), "/plain/path", "fish only where a word needs it");
    }

    /// A styled line is written with its escapes spelled out — no raw escape in the file — and
    /// prints back byte for byte: its styling, a single quote, a backslash. (Run through bash here;
    /// each shell's suite runs its own file's greeting.)
    #[test]
    fn a_styled_line_is_spelled_out_and_prints_back_exactly() {
        let text = "\x1b[1;32mit's a back\\slash\x1b[0m";
        for shell in [Shell::Bash, Shell::Fish] {
            let line = printf_line(text, shell);
            assert!(!line.contains('\x1b') && line.contains(r"\033[1;32m"), "{shell:?}: {line}");
        }
        let out = std::process::Command::new("bash").args(["--norc", "-c", &printf_line(text, Shell::Bash)]).output().expect("bash");
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{text}\n"));
    }
}
