//! Commands for keeping the system's package managers and dev toolchains current.

#[bashrs_macros::category(command = PackagesCommand, prefix = "packages_")]
mod commands {
    use crate::support::args::NoArgs;
    use crate::support::exec::{run_reporting, succeeds_quietly};

    /// Update and upgrade every package manager present on the system
    #[prefixed]
    #[unprefixed]
    pub fn upup(_args: NoArgs) {
        _upgrade(MANAGERS);
    }

    /// Update the development toolchains (rustup, uv, ...) that are installed
    pub fn update_toolchains(_args: NoArgs) {
        _upgrade(TOOLCHAINS);
    }

    /// Update everything: package managers, then dev toolchains
    #[name("UPUP")]
    pub fn upgrade_all(_args: NoArgs) {
        upup(NoArgs {});
        update_toolchains(NoArgs {});
    }

    /// List installed packages, grouped under their package manager
    pub fn print(_args: NoArgs) {
        for mgr in _active(MANAGERS) {
            if let Some(list) = mgr.list {
                println!("== {} ==", mgr.probe);
                _print_listing(list);
            }
        }
    }

    /// A managed tool: how to detect it and the commands that update, upgrade, and
    /// (for package managers) list it. Backs both the package managers and toolchains.
    struct Manager {
        /// Binary looked up on `PATH` to detect it.
        probe: &'static str,
        /// Extra capability check (a `"program arg .."` command); the tool is skipped
        /// unless it succeeds. E.g. `nix profile` needs flakes enabled.
        precheck: Option<&'static str>,
        /// Others this one makes redundant when both are present (e.g. `yay` wraps
        /// `pacman`), so the superseded one is skipped.
        supersedes: &'static [&'static str],
        /// Update+upgrade commands, run in order. Each is `"program arg .."`, split
        /// on whitespace — fine here because no token contains a space.
        steps: &'static [&'static str],
        /// Command listing what's installed, for `print`. `None` for toolchains,
        /// which have no meaningful package list.
        list: Option<&'static str>,
    }

    // Only `precheck`/`supersedes`/`list` are taken from here via `..DEFAULTS`; every
    // row always sets its own `probe` and `steps`.
    const DEFAULTS: Manager = Manager { probe: "", precheck: None, supersedes: &[], steps: &[], list: None };

    /// System package managers `upup`/`print` drive; add a row to support another.
    /// `autoremove` steps prune no-longer-needed packages, so upgrades can remove things.
    /// List commands are chosen to emit only packages — no progress/header lines.
    const MANAGERS: &[Manager] = &[
        Manager { probe: "apt", steps: &["sudo apt update", "sudo apt full-upgrade -y", "sudo apt autoremove -y"], list: Some("dpkg-query -W"), ..DEFAULTS },
        Manager { probe: "dnf", steps: &["sudo dnf upgrade --refresh -y", "sudo dnf autoremove -y"], list: Some("rpm -qa"), ..DEFAULTS },
        Manager { probe: "yay", supersedes: &["pacman"], steps: &["yay -Syu --noconfirm"], list: Some("yay -Q"), ..DEFAULTS }, // wraps pacman; invokes sudo itself
        Manager { probe: "pacman", steps: &["sudo pacman -Syu --noconfirm"], list: Some("pacman -Q"), ..DEFAULTS },
        Manager { probe: "zypper", steps: &["sudo zypper refresh", "sudo zypper update -y"], list: Some("rpm -qa"), ..DEFAULTS },
        Manager { probe: "apk", steps: &["sudo apk update", "sudo apk upgrade"], list: Some("apk info"), ..DEFAULTS },
        Manager { probe: "guix", steps: &["guix pull", "guix upgrade"], list: Some("guix package --list-installed"), ..DEFAULTS },
        Manager { probe: "nix-env", steps: &["nix-channel --update", "nix-env --upgrade"], list: Some("nix-env -q"), ..DEFAULTS },
        Manager { probe: "nix", precheck: Some("nix profile list"), steps: &["nix profile upgrade"], list: Some("nix profile list"), ..DEFAULTS }, // new-style flake profiles
        Manager { probe: "snap", steps: &["sudo snap refresh"], list: Some("snap list"), ..DEFAULTS },
        Manager { probe: "flatpak", steps: &["flatpak upgrade --assumeyes"], list: Some("flatpak list"), ..DEFAULTS },
        Manager { probe: "brew", steps: &["brew update", "brew upgrade"], list: Some("brew list"), ..DEFAULTS }, // brew refuses to run under sudo
    ];

    /// Dev toolchains `update_toolchains` drives. Each `steps` self-updates the tool
    /// itself, which only works if it was installed via its own installer — a
    /// package-managed copy is handled by `upup` instead (and self-update would error
    /// here, harmlessly, since steps report-and-continue). Shell-sourced version
    /// managers (nvm, sdkman, asdf, pyenv, ...) aren't `PATH` binaries and are out of scope.
    const TOOLCHAINS: &[Manager] = &[
        Manager { probe: "rustup", steps: &["rustup update"], ..DEFAULTS },
        Manager { probe: "ghcup", steps: &["ghcup upgrade"], ..DEFAULTS },
        Manager { probe: "uv", steps: &["uv self update"], ..DEFAULTS },
        Manager { probe: "poetry", steps: &["poetry self update"], ..DEFAULTS },
        Manager { probe: "pnpm", steps: &["pnpm self-update"], ..DEFAULTS },
        Manager { probe: "stack", steps: &["stack upgrade"], ..DEFAULTS },
        Manager { probe: "cpanm", steps: &["cpanm --self-upgrade"], ..DEFAULTS },
    ];

    /// Detect, then run each active tool's `steps` under a header. Shared by `upup`
    /// (package managers) and `update_toolchains` (dev toolchains).
    fn _upgrade(table: &'static [Manager]) {
        for tool in _active(table) {
            println!("== {} ==", tool.probe);
            for &step in tool.steps {
                _run_line(step); // report + continue, so one failure won't abort the rest
            }
        }
    }

    /// The tools in `table` to act on: present on `PATH`, not superseded by another
    /// present tool, and passing their capability precheck (if any).
    fn _active(table: &'static [Manager]) -> Vec<&'static Manager> {
        let present: Vec<&'static Manager> = table.iter().filter(|m| _on_path(m.probe)).collect();
        present
            .iter()
            .copied()
            .filter(|tool| !present.iter().any(|other| other.supersedes.contains(&tool.probe)))
            .filter(|tool| tool.precheck.is_none_or(_run_quietly))
            .collect()
    }

    /// Whether `program` is an executable found in any `PATH` directory — a
    /// dependency-free `command -v` (checks the executable bit, not mere presence).
    fn _on_path(program: &str) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                std::fs::metadata(dir.join(program))
                    .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            })
        })
    }

    /// Split a `"program arg .."` line and run it, inheriting stdio and reporting failure.
    fn _run_line(line: &str) {
        let mut parts = line.split_whitespace();
        if let Some(program) = parts.next() {
            run_reporting(program, parts);
        }
    }

    /// Split a `"program arg .."` line and run it with output suppressed; true on success.
    fn _run_quietly(line: &str) -> bool {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some(program) => succeeds_quietly(program, parts),
            None => false,
        }
    }

    /// Run a listing command and print its output indented under the tool header.
    fn _print_listing(line: &str) {
        let mut parts = line.split_whitespace();
        let Some(program) = parts.next() else { return };
        match std::process::Command::new(program).args(parts).output() {
            Ok(output) => {
                for entry in String::from_utf8_lossy(&output.stdout).lines() {
                    println!("  {entry}");
                }
            }
            Err(err) => eprintln!("  could not run {program}: {err}"),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn assert_table_integrity(table: &[Manager]) {
            // upup/update_toolchains/print take the first whitespace token as the
            // program, so every step and list command must have one; `supersedes`
            // must reference a tool that actually exists in the same table.
            let probes: Vec<&str> = table.iter().map(|m| m.probe).collect();
            for tool in table {
                assert!(!tool.steps.is_empty(), "{}: needs at least one step", tool.probe);
                for step in tool.steps {
                    assert!(step.split_whitespace().next().is_some(), "{}: blank step", tool.probe);
                }
                if let Some(list) = tool.list {
                    assert!(list.split_whitespace().next().is_some(), "{}: blank list command", tool.probe);
                }
                for superseded in tool.supersedes {
                    assert!(probes.contains(superseded), "{}: supersedes unknown `{superseded}`", tool.probe);
                }
            }
        }

        #[test]
        fn manager_and_toolchain_tables_are_well_formed() {
            assert_table_integrity(MANAGERS);
            assert_table_integrity(TOOLCHAINS);
        }

        #[test]
        fn every_package_manager_has_a_list_command() {
            // `print` needs a list command per package manager; toolchains have none.
            for mgr in MANAGERS {
                assert!(mgr.list.is_some(), "{}: package managers need a `list` command", mgr.probe);
            }
        }

        #[test]
        fn on_path_finds_a_ubiquitous_binary_and_rejects_a_bogus_one() {
            assert!(_on_path("sh"), "expected to find `sh` on PATH");
            assert!(!_on_path("bashrs_no_such_program_xyz"));
        }
    }
}
