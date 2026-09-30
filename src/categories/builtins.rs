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
    use std::io::Write;
    use std::path::PathBuf;

    use clap::Args;
    use termcolor::{BufferedStandardStream, ColorChoice};

    use crate::support::{diff_highlight, exec};

    /// `cd`, then print what you landed in: `ls -AF`, directories first, colours on
    ///
    /// The follow-up runs only when the `cd` itself succeeded, and `builtin cd "$@"` keeps every
    /// native behavior — `cd -`, `-P`/`-L`, `CDPATH` — exactly as it was. Problematic filenames
    /// arrive escaped by GNU ls's own `--quoting-style=shell-escape`: the same paste-back-safe
    /// discipline `lll` implements through `support::shell_quote` (bare when safe, quoted
    /// otherwise, control characters as `$'…'`; GNU spells the odd case differently — `"it's"`
    /// where `lll` would write `$'it\'s'` — but every spelling reads back as the exact name),
    /// with the `-F` type markers and colour spans kept outside the quotes.
    #[unprefixed]
    #[shell_body(r#"builtin cd "$@" && ls -AF --color=always --group-directories-first --quoting-style=shell-escape"#)]
    pub fn cd() {}

    /// `git diff` between any two files or directories — no repository needed — with what changed
    /// inside each changed line highlighted
    ///
    /// `-` for either side reads stdin. Exits as git does — 0 when the two match, 1 when they
    /// differ or a path cannot be read — and 2 when git itself cannot run, `diff`'s own code for
    /// trouble.
    ///
    /// **Always in colour**, piped or not, so `| less -R` keeps it. For a plain patch, use
    /// `git diff --no-index` itself.
    ///
    /// **Shadows the system `diff` at the prompt.** Scripts still get the system one, and
    /// `command diff` reaches it from a prompt too.
    #[unprefixed]
    pub fn diff(args: DiffArgs) {
        // `--no-index` compares the two paths as plain files, needing no repository and consulting
        // none; `--no-ext-diff` keeps a configured external diff tool from replacing the unified
        // format the highlighter reads; `--color=never` because the colour is ours to add; and
        // `--` keeps a path that starts with `-` a path.
        let flags = ["diff", "--no-index", "--no-ext-diff", "--color=never", "--"].map(OsStr::new);
        let git_args = flags.into_iter().chain([args.old.as_os_str(), args.new.as_os_str()]);
        let Some((code, output)) = exec::capture_with_code("git", git_args) else {
            std::process::exit(2);
        };
        // Coloured whether or not stdout is a terminal: the colour is the point of this command,
        // and the one pipe its output goes into is a pager.
        let mut out = BufferedStandardStream::stdout(ColorChoice::Always);
        if let Err(err) = diff_highlight::render(&output, args.line_number, &mut out).and_then(|()| out.flush()) {
            eprintln!("diff: could not write the diff: {err}");
        }
        std::process::exit(code);
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
    }
}
