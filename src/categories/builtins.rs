//! Overrides of names the shell already answers to — a builtin's or a system command's. Same
//! name, the original's behaviour, plus what you wanted it to do anyway.
//!
//! Two kinds, told apart by where the work has to happen:
//!
//! - **A builtin's override is a `#[shell_body]`**, because it exists to act on the *calling*
//!   shell (a child process can't `cd` its parent): a shell function carrying the builtin's exact
//!   name (`#[unprefixed]`), reaching the real thing through the `builtin` keyword — which is also
//!   its escape hatch: `builtin cd` is always the plain one. Anything beyond a one-liner belongs
//!   in the binary, piped or called, not in the body (see `shell_def` for the pattern).
//! - **A system command's override is an ordinary command** wearing the tool's exact name
//!   (`#[unprefixed]`), whose generated shell function shadows the system one at the prompt.
//!   Scripts never see it, and `command diff` reaches the original. Shadowing a name the system
//!   already has is a decision taken one name at a time, and the naming test lists each.
//!
//! Related but deliberately elsewhere: `..` (filesystem) is a shortcut *named* like nothing the
//! shell owns, not an override of something it does.

#[bashrs_macros::category(command = BuiltinsCommand, prefix = "overwritten_")]
mod commands {
    use std::ffi::OsStr;
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};

    use clap::Args;
    use termcolor::{BufferedStandardStream, ColorChoice, WriteColor};

    use crate::support::dir_diff::{self, Finding};
    use crate::support::{diff_highlight, exec};

    /// `cd`, then print what you landed in: `ls -AF`, directories first, colours on
    ///
    /// The follow-up runs only when the `cd` itself succeeded, and `builtin cd "$@"` keeps every
    /// native behavior — `cd -`, `-P`/`-L`, `CDPATH` — exactly as it was. The listing is the
    /// binary's (`cd-listing`): GNU ls's selection, order, colours and columns, with every name
    /// spelled for the shell it is shown in by `support::shell_quote` — `lll`'s paste-back-safe
    /// discipline: bare when safe, quoted otherwise, control characters escaped the way that shell
    /// reads them back, the `-F` type markers outside the quotes. No quoting style of GNU ls's own
    /// is fish's. The binary runs `ls` itself, so no shell alias of `ls` (an `eza` stand-in) can
    /// take the listing over.
    ///
    /// In fish the original is not the builtin: `cd -` and the directory history live in fish's own
    /// `cd` *function*, so `{ORIGINAL}` calls a copy of that, saved before this replaces it.
    #[unprefixed]
    #[shell_body(bash = r#"builtin cd "$@" && {BINARY} cd-listing"#, fish = "{ORIGINAL} $argv && {BINARY} cd-listing")]
    pub fn cd() {}

    /// `git diff` between any two files or directories — no repository needed — with what changed
    /// inside each changed line highlighted
    ///
    /// `-` for either side reads stdin. Two directories are compared as GNU diff compares them:
    /// one level deep unless `-r`, what one side lacks named in a line of its own (`Only in b:
    /// x`), and git's diff for each pair of files that differ. Exits 0 when the two match and 1
    /// when they differ; 2 is trouble — something that cannot be read, or no git to run. (A
    /// missing file is git's to report, and git reports it with 1.)
    ///
    /// **Always in colour**, piped or not, so `| less -R` keeps it. For a plain patch, use
    /// `git diff --no-index` itself.
    ///
    /// **Shadows the system `diff` at the prompt.** Scripts still get the system one, and
    /// `command diff` reaches it from a prompt too.
    #[unprefixed]
    pub fn diff(args: DiffArgs) {
        // Coloured whether or not stdout is a terminal: the colour is the point of this command,
        // and the one pipe its output goes into is a pager.
        let mut out = BufferedStandardStream::stdout(ColorChoice::Always);
        let compared = if args.old.is_dir() && args.new.is_dir() {
            _diff_directories(&args, &mut out)
        } else {
            _git_diff(&args.old, &args.new, args.line_number, &mut out).map(|code| code.unwrap_or(2))
        };
        match compared.and_then(|code| out.flush().map(|()| code)) {
            Ok(code) => std::process::exit(code),
            Err(err) => {
                eprintln!("diff: could not write the diff: {err}");
                std::process::exit(2);
            }
        }
    }

    /// Two directories compared GNU's way ([`dir_diff`]), each finding painted into `out` as the
    /// walk reaches it: git's diff for a pair that differs, and GNU's line for the rest — which
    /// the highlighter paints as it paints git's headers, both being news about files rather than
    /// lines. GNU's exit code: 0 when nothing was found, 1 when something was, and 2 for trouble,
    /// which outranks both.
    fn _diff_directories(args: &DiffArgs, out: &mut impl WriteColor) -> io::Result<i32> {
        let mut code = 0;
        for finding in dir_diff::compare(&args.old, &args.new, args.recursive) {
            let found = match finding {
                Finding::Differ(old, new) => match _git_diff(&old, &new, args.line_number, out)? {
                    Some(code) => code.min(2),
                    // No git for this pair is no git for any: nothing after it could be shown.
                    None => return Ok(2),
                },
                Finding::Line(line) => {
                    diff_highlight::render(&line, args.line_number, out)?;
                    1
                }
                Finding::Unreadable(path, err) => {
                    eprintln!("diff: {}: {err}", path.display());
                    2
                }
            };
            code = code.max(found);
        }
        Ok(code)
    }

    /// One `git diff --no-index` of two paths, painted into `out`: git's exit code, or `None` when
    /// git could not run (already reported).
    fn _git_diff(old: &Path, new: &Path, numbered: bool, out: &mut impl WriteColor) -> io::Result<Option<i32>> {
        // `--no-index` compares the two paths as plain files, needing no repository and consulting
        // none; `--no-ext-diff` keeps a configured external diff tool from replacing the unified
        // format the highlighter reads; `--color=never` because the colour is ours to add; and
        // `--` keeps a path that starts with `-` a path.
        let flags = ["diff", "--no-index", "--no-ext-diff", "--color=never", "--"].map(OsStr::new);
        let git_args = flags.into_iter().chain([old.as_os_str(), new.as_os_str()]);
        let Some((code, output)) = exec::capture_with_code("git", git_args) else {
            return Ok(None);
        };
        diff_highlight::render(&output, numbered, out)?;
        Ok(Some(code))
    }

    /// The two sides of a `diff`.
    #[derive(Args)]
    pub struct DiffArgs {
        /// The old side, whose lines show as removed: a file, a directory, or `-` for stdin
        pub(crate) old: PathBuf,
        /// The new side, whose lines show as added: a file, a directory, or `-` for stdin
        pub(crate) new: PathBuf,
        /// Number the lines, like `grep -n`: each line's number in the old file and in the new
        /// before it (a removed line has only the old, an added line only the new)
        #[arg(short = 'n', long)]
        pub(crate) line_number: bool,
        /// Compare subdirectories too, all the way down, as `diff -r` does. Without it two
        /// directories are compared one level deep, and a subdirectory both have is only named
        #[arg(short, long)]
        pub(crate) recursive: bool,
    }
}
