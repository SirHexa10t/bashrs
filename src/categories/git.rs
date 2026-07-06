//! Git helpers (`git_*`), each shelling out to `git` (or `find`) via [`crate::support::exec`].

#[bashrs_macros::category(command = GitCommand, prefix = "git_")]
mod commands {
    use crate::support::args::NoArgs;
    use crate::support::exec;
    use clap::Args;

    /// A pretty, one-line commit graph across all branches
    pub fn tree(args: TreeArgs) {
        let mut git = vec!["log", "--all", "--graph", "--decorate", "--abbrev-commit", TREE_FORMAT];
        git.extend(args.rest.iter().map(String::as_str));
        exec::run_reporting("git", git);
        // Trailing blank line (as with `lll`): spares the last row when the terminal is
        // enlarged afterward. `tformat:` ends `git log`'s output in a newline, so this one
        // adds the blank line — unconditional, since it's just a display nicety.
        println!();
    }

    /// Extra arguments passed straight to `git log` (e.g. `-20`, or `-- <path>`).
    #[derive(Args)]
    pub struct TreeArgs {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        pub rest: Vec<String>,
    }

    /// Recursively run `git pull` in every directory below here that holds a `.git`
    pub fn rec_pull(_args: NoArgs) {
        // Find each repo (a directory containing `.git`) and pull it in place, announcing
        // each one. `-execdir` runs from the repo directory, so `$(pwd)` and `git pull`
        // both act on that repo.
        exec::run_reporting(
            "find",
            [".", "-type", "d", "-name", ".git", "-execdir", "sh", "-c", REC_PULL_SH, ";"],
        );
    }

    /// Show the git identity in effect here: last commit's author, the configured
    /// `user.email` (and where it's set), who GitHub authenticates you as, and the remotes
    pub fn profile(_args: NoArgs) {
        // A dump of diagnostic commands — their exit statuses don't matter (`ssh -T
        // git@github.com` always exits 1), so each just runs and shows its output.
        println!("previous commit:");
        exec::run("git", ["log", "-1", "--format=%an <%ae>"]);
        println!("currently using:");
        exec::run("git", ["config", "--show-origin", "user.email"]);
        println!("GitHub identified user:");
        exec::run("ssh", ["-T", "git@github.com"]);
        println!("Owner/Repo:");
        exec::run("git", ["remote", "-v"]);
    }

    /// `git log` pretty format: blue short-hash, green relative age, subject, dim author + refs.
    const TREE_FORMAT: &str = "--format=tformat:%C(bold blue)%h%C(reset) %C(bold green)(%ar)%C(reset) %C(white)%s%C(reset) %C(dim white)— %an%C(reset)%C(auto)%d%C(reset)";
    /// Shell run per repo by `git_rec_pull` (via `find -execdir`): announce the dir, then pull.
    const REC_PULL_SH: &str = r#"echo "Updating $(pwd)"; git pull"#;

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn tree_format_carries_the_expected_fields() {
            assert!(TREE_FORMAT.starts_with("--format=tformat:"));
            for field in ["%h", "%ar", "%s", "%an", "%d"] {
                assert!(TREE_FORMAT.contains(field), "missing {field} in the format");
            }
        }

        #[test]
        fn rec_pull_announces_then_pulls_each_repo() {
            assert!(REC_PULL_SH.contains("Updating"));
            assert!(REC_PULL_SH.contains("git pull"));
        }
    }
}
