//! Python-backed commands (`py_*`), running on the interpreter [`crate::tools`] resolves —
//! bundled-first, so they behave the same everywhere regardless of how current the system's
//! python is. Add python-powered commands here; the interpreter is `tools::resolve("python3")`.

#[bashrs_macros::category(command = PythonCommand, prefix = "py_")]
mod commands {
    use crate::support::args::NoArgs;
    use crate::support::exec::run_reporting;
    use crate::tools;
    use clap::Args;

    /// Evaluate a Python expression and print its result — e.g. `py "2**32 / 3"`
    #[unprefixed]
    pub fn py(args: PyArgs) {
        run_reporting(tools::resolve("python3"), ["-c".to_string(), _print_wrapped(&args.expression)]);
    }

    /// The expression to evaluate (words are joined, so quoting is optional: `py 2 ** 10`).
    #[derive(Args)]
    pub struct PyArgs {
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        pub(crate) expression: Vec<String>,
    }

    /// Install python package(s) into bashrs's bundled environment, at their latest (upgrading
    /// if already present) — uv-managed
    pub fn install(args: InstallArgs) {
        if !tools::python::install(&args.packages) {
            std::process::exit(1);
        }
    }

    /// Package name(s), as `uv pip install` accepts them. A bare name gets the latest; version
    /// specs pin (`rich==13.7.0`) or bound (`'rich>=13'` — quoted, since `>` redirects in shell).
    #[derive(Args)]
    pub struct InstallArgs {
        #[arg(required = true)]
        pub(crate) packages: Vec<String>,
    }

    /// Upgrade every python package in the bundled environment (python itself updates on compile)
    pub fn update(_args: NoArgs) {
        if !tools::python::upgrade_all() {
            std::process::exit(1);
        }
    }

    /// Revert the bundled environment's packages to their pre-last-change versions — the escape
    /// hatch when a latest-version install/update breaks something
    pub fn rollback(_args: NoArgs) {
        if !tools::python::rollback() {
            std::process::exit(1);
        }
    }

    /// The `python3 -c` program for an expression: joined and wrapped in `print(…)`.
    fn _print_wrapped(expression: &[String]) -> String {
        format!("print({})", expression.join(" "))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn print_wrapping_joins_the_words() {
            assert_eq!(_print_wrapped(&["2".into(), "**".into(), "10".into()]), "print(2 ** 10)");
            assert_eq!(_print_wrapped(&["'a' * 3".into()]), "print('a' * 3)");
        }
    }
}
