//! Python-backed commands (`py_*`), backed by [`crate::drivers::python`] over the bundled
//! environment [`crate::tools`] resolves —
//! bundled-first, so they behave the same everywhere regardless of how current the system's
//! python is. Add python-powered commands here; the interpreter is `tools::resolve("python3")`.

#[bashrs_macros::category(command = PythonCommand, prefix = "py_")]
mod commands {
    use crate::support::args::NoArgs;
    use crate::drivers;
    use clap::Args;

    /// Install python package(s) into bashrs's bundled environment, at their latest (upgrading
    /// if already present) — uv-managed
    pub fn install(args: InstallArgs) {
        if !drivers::python::install(&args.packages) {
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
        if !drivers::python::upgrade_all() {
            std::process::exit(1);
        }
    }

    /// Revert the bundled environment's packages to their pre-last-change versions — the escape
    /// hatch when a latest-version install/update breaks something
    pub fn rollback(_args: NoArgs) {
        if !drivers::python::rollback() {
            std::process::exit(1);
        }
    }

}
