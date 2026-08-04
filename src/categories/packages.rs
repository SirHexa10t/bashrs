//! Commands for keeping the system's package managers and dev toolchains current.

#[bashrs_macros::category(command = PackagesCommand, prefix = "packages_")]
mod commands {
    use crate::support::args::NoArgs;
    use crate::support::exec::{capture_stdout, on_path, run_reporting, succeeds_quietly};
    use crate::support::package_management as pm;
    use crate::support::superuser::{self, CMD};

    /// Update and upgrade every package manager present on the system
    #[prefixed]
    #[unprefixed]
    pub fn upup(_args: NoArgs) {
        // Probed before the first elevated step; a ticket the user held going in is theirs
        // to keep, so only an elevation this run itself earned is dropped afterwards.
        let had_ticket = superuser::ticket_exists();
        _upgrade(_managers_active());
        superuser::revoke_ours(had_ticket);
    }

    /// Update the development toolchains (rustup, uv, ...) that are installed
    pub fn update_toolchains(_args: NoArgs) {
        _upgrade(_toolchains_active()); // toolchains self-update as the user; none elevate, so nothing to revoke
    }

    /// Update everything: package managers, then dev toolchains, then bashrs's own bundled
    /// tools and companion repos (the same provisioning COMPILE.sh runs)
    #[name("UPUP")]
    pub fn upgrade_all(_args: NoArgs) {
        upup(NoArgs {});
        update_toolchains(NoArgs {});
        // The bundled side last: tools, yt-dlp's python helpers, companion repos — recorded in
        // Carstay.toml with the usual revert notice, exactly as COMPILE.sh provisions them.
        crate::drivers::install_stainless(false);
    }

    /// List installed packages, grouped under their package manager
    pub fn print(_args: NoArgs) {
        for mgr in _managers_active() {
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
        /// Extra capability check (a `["program", "arg", ..]` word-list); the tool is skipped
        /// unless it succeeds. E.g. `nix profile` needs flakes enabled.
        precheck: Option<&'static [&'static str]>,
        /// Others this one makes redundant when both are present (e.g. `yay` wraps
        /// `pacman`), so the superseded one is skipped.
        supersedes: &'static [&'static str],
        /// Update+upgrade commands, run in order. Each is a `["program", "arg", ..]` word-list;
        /// a step whose first word is [`CMD`] runs under the superuser (see [`superuser`]), which
        /// is how a step opts into elevation without naming the escalation tool itself.
        steps: &'static [&'static [&'static str]],
        /// Command listing what's installed, for `print`. `None` for toolchains,
        /// which have no meaningful package list.
        list: Option<&'static [&'static str]>,
    }

    // Only `precheck`/`supersedes`/`list` are taken from here via `..DEFAULTS`; every
    // row always sets its own `probe` and `steps`.
    const DEFAULTS: Manager = Manager { probe: "", precheck: None, supersedes: &[], steps: &[], list: None };

    /// System package managers `upup`/`print` drive; add a row to support another.
    /// `autoremove` steps prune no-longer-needed packages, so upgrades can remove things.
    /// List commands are chosen to emit only packages — no progress/header lines.
    /// A step beginning with [`CMD`] runs elevated; the escalation tool itself lives in [`superuser`].
    const MANAGERS: &[Manager] = &[
        Manager { probe: "apt", steps: &[&[CMD, "apt", "update"], &[CMD, "apt", "full-upgrade", "-y"], &[CMD, "apt", "autoremove", "-y"]], list: Some(&["dpkg-query", "-W"]), ..DEFAULTS },
        Manager { probe: "dnf", steps: &[&[CMD, "dnf", "upgrade", "--refresh", "-y"], &[CMD, "dnf", "autoremove", "-y"]], list: Some(&["rpm", "-qa"]), ..DEFAULTS },
        Manager { probe: "yay", supersedes: &["pacman"], steps: &[&["yay", "-Syu", "--noconfirm"]], list: Some(&["yay", "-Q"]), ..DEFAULTS }, // wraps pacman; invokes the superuser command itself
        Manager { probe: "pacman", steps: &[&[CMD, "pacman", "-Syu", "--noconfirm"]], list: Some(&["pacman", "-Q"]), ..DEFAULTS },
        Manager { probe: "zypper", steps: &[&[CMD, "zypper", "refresh"], &[CMD, "zypper", "update", "-y"]], list: Some(&["rpm", "-qa"]), ..DEFAULTS },
        Manager { probe: "apk", steps: &[&[CMD, "apk", "update"], &[CMD, "apk", "upgrade"]], list: Some(&["apk", "info"]), ..DEFAULTS },
        Manager { probe: "guix", steps: &[&["guix", "pull"], &["guix", "upgrade"]], list: Some(&["guix", "package", "--list-installed"]), ..DEFAULTS },
        Manager { probe: "nix-env", steps: &[&["nix-channel", "--update"], &["nix-env", "--upgrade"]], list: Some(&["nix-env", "-q"]), ..DEFAULTS },
        Manager { probe: "nix", precheck: Some(&["nix", "profile", "list"]), steps: &[&["nix", "profile", "upgrade"]], list: Some(&["nix", "profile", "list"]), ..DEFAULTS }, // new-style flake profiles
        Manager { probe: "snap", steps: &[&[CMD, "snap", "refresh"]], list: Some(&["snap", "list"]), ..DEFAULTS },
        Manager { probe: "flatpak", steps: &[&["flatpak", "upgrade", "--assumeyes"]], list: Some(&["flatpak", "list"]), ..DEFAULTS },
        Manager { probe: "brew", steps: &[&["brew", "update"], &["brew", "upgrade"]], list: Some(&["brew", "list"]), ..DEFAULTS }, // brew refuses to run as root
    ];

    /// Dev toolchains `update_toolchains` drives. Each `steps` self-updates the tool
    /// itself, which only works if it was installed via its own installer — a
    /// package-managed copy is handled by `upup` instead (and self-update would error
    /// here, harmlessly, since steps report-and-continue). Shell-sourced version
    /// managers (nvm, sdkman, asdf, pyenv, ...) aren't `PATH` binaries and are out of scope.
    const TOOLCHAINS: &[Manager] = &[
        Manager { probe: "rustup", steps: &[&["rustup", "update"]], ..DEFAULTS },
        Manager { probe: "ghcup", steps: &[&["ghcup", "upgrade"]], ..DEFAULTS },
        // No `uv` row: uv is a bundled tool here (the PATH shim resolves to bashrs's copy, which
        // `uv self update` refuses — it only updates standalone-installer binaries). The bundled
        // copy is version-managed by the tools sync instead: COMPILE.sh, and `UPUP`'s final step.
        Manager { probe: "poetry", steps: &[&["poetry", "self", "update"]], ..DEFAULTS },
        Manager { probe: "pnpm", steps: &[&["pnpm", "self-update"]], ..DEFAULTS },
        Manager { probe: "stack", steps: &[&["stack", "upgrade"]], ..DEFAULTS },
        Manager { probe: "cpanm", steps: &[&["cpanm", "--self-upgrade"]], ..DEFAULTS },
    ];

    /// Run each active tool's `steps` under a header. Shared by `upup` (package managers) and
    /// `update_toolchains` (dev toolchains).
    fn _upgrade(active: Vec<&'static Manager>) {
        for tool in active {
            println!("== {} ==", tool.probe);
            for &step in tool.steps {
                _run_line(step); // report + continue, so one failure won't abort the rest
            }
        }
    }

    /// The package managers to act on. Detection is delegated to the shared registry
    /// ([`pm::present`]) — so Nix/Homebrew installs outside `PATH` are seen, and the manager
    /// list has a single home — then this table's `supersedes`/`precheck` are applied.
    fn _managers_active() -> Vec<&'static Manager> {
        let present = pm::present(&crate::conf::home());
        _active(MANAGERS, |probe| present.iter().any(|m| m.command == probe))
    }

    /// The dev toolchains to act on. These are self-updating installers, not system package
    /// managers, so they're detected plainly on `PATH` (and aren't in the shared registry).
    fn _toolchains_active() -> Vec<&'static Manager> {
        _active(TOOLCHAINS, on_path)
    }

    /// The rows of `table` to act on: detected present by `is_present`, not superseded by
    /// another present tool, and passing their capability precheck (if any).
    fn _active(
        table: &'static [Manager],
        is_present: impl Fn(&str) -> bool,
    ) -> Vec<&'static Manager> {
        let present: Vec<&'static Manager> = table.iter().filter(|m| is_present(m.probe)).collect();
        present
            .iter()
            .copied()
            .filter(|tool| !present.iter().any(|other| other.supersedes.contains(&tool.probe)))
            .filter(|tool| tool.precheck.is_none_or(_run_quietly))
            .collect()
    }

    /// Run a `["program", "arg", ..]` word-list — the first word is the command, the rest its
    /// args — inheriting stdio and reporting failure. A list beginning with [`CMD`] runs elevated.
    fn _run_line(words: &[&str]) {
        if let Some((&program, args)) = words.split_first() {
            run_reporting(program, args);
        }
    }

    /// Run a word-list with output suppressed; true on success (false if empty).
    fn _run_quietly(words: &[&str]) -> bool {
        match words.split_first() {
            Some((&program, args)) => succeeds_quietly(program, args),
            None => false,
        }
    }

    /// Run a listing word-list and print its output indented under the tool header. Goes through
    /// [`capture_stdout`], so a failing list command shows its stderr and prints no partial
    /// listing, rather than failing silently.
    fn _print_listing(words: &[&str]) {
        let Some((&program, args)) = words.split_first() else { return };
        if let Some(listing) = capture_stdout(program, args) {
            for entry in listing.lines() {
                println!("  {entry}");
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn assert_table_integrity(table: &[Manager]) {
            // upup/update_toolchains/print take the first word as the program, so every step and
            // list command must be a non-empty word-list; `supersedes` must reference a tool that
            // actually exists in the same table.
            let probes: Vec<&str> = table.iter().map(|m| m.probe).collect();
            for tool in table {
                assert!(!tool.steps.is_empty(), "{}: needs at least one step", tool.probe);
                for step in tool.steps {
                    assert!(!step.is_empty(), "{}: blank step", tool.probe);
                }
                if let Some(list) = tool.list {
                    assert!(!list.is_empty(), "{}: blank list command", tool.probe);
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
        fn upgrade_recipes_and_the_shared_registry_stay_one_to_one() {
            // The single-source-of-truth guarantee: this table's package managers and the shared
            // registry's must be exactly the same set. A manager added to one and not the other
            // fails here instead of silently going unhandled somewhere (the bug that once hid Nix
            // from the cookie scan).
            use std::collections::BTreeSet;
            let recipes: BTreeSet<&str> = MANAGERS.iter().map(|m| m.probe).collect();
            let registry: BTreeSet<&str> = pm::MANAGERS.iter().map(|m| m.command).collect();
            assert_eq!(recipes, registry, "packages.rs recipes must match support::package_management");
        }
    }
}
