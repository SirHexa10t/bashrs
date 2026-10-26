//! What bashrs adds to devmounts' `apply` in `symlink_devs`: the config it takes from
//! `~/.bashrs/user-custom/devmounts` when none is named, and what it says when there's no single
//! one there to take. What devmounts does with a config is devmounts', and tested there.
//!
//! The child's world is HERMETIC: a scratch HOME, a PATH with nothing in it, and the machine
//! devmounts reads given as fixture trees through its `DEVMOUNTS_*` redirects — one ext4 device
//! labelled X, mounted, holding `docs/`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

/// One test's world, removed on drop.
struct Rig {
    root: PathBuf,
    home: PathBuf,
    /// Where the links go (`--root`).
    links: PathBuf,
    /// Where the device is mounted.
    data: PathBuf,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let root = std::env::temp_dir().join(format!("bashrs_symlink_devs_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let block = root.join("sys/class/block/sdb1");
        fs::create_dir_all(&block).unwrap();
        fs::write(block.join("dev"), "8:17\n").unwrap();
        fs::write(block.join("uevent"), "DEVNAME=sdb1\n").unwrap();
        fs::write(block.join("partition"), "1\n").unwrap();
        fs::create_dir_all(root.join("udev")).unwrap();
        fs::write(root.join("udev/b8:17"), "E:ID_FS_TYPE=ext4\nE:ID_FS_UUID=u-1\nE:ID_FS_LABEL=X\n").unwrap();
        let data = root.join("media/X");
        fs::create_dir_all(data.join("docs")).unwrap();
        let mounted = format!("1 0 0:1 / / rw - rootfs none rw\n30 1 8:17 / {} rw - ext4 /dev/sdb1 rw\n", data.display());
        fs::write(root.join("mountinfo"), mounted).unwrap();
        let (home, links) = (root.join("home"), root.join("links"));
        for dir in [&home, &links, &root.join("bin")] {
            fs::create_dir_all(dir).unwrap();
        }
        Rig { root, home, links, data }
    }

    /// The store, where the user is told to keep the config.
    fn store(&self) -> PathBuf {
        self.home.join(".bashrs/user-custom/devmounts")
    }

    /// A config for the rig's device: `devmount` for its root, and `docs_<devmount>` into it.
    fn config(devmount: &str) -> String {
        format!("[[device]]\nlabel = 'X'\ndevmount = '{devmount}'\nmounts = {{ docs_{devmount} = 'docs' }}\n")
    }

    /// `bashrs symlink_devs <args> --root <links> --escalate none`, in the rig's world.
    fn run(&self, args: &[&str]) -> Output {
        self.run_escalating(args, "none")
    }

    /// The same, with privileges gained through `escalate`.
    fn run_escalating(&self, args: &[&str], escalate: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .arg("symlink_devs")
            .args(args)
            .args(["--root", self.links.to_str().unwrap(), "--escalate", escalate])
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", self.root.join("bin"))
            .env("DEVMOUNTS_SYSFS", self.root.join("sys"))
            .env("DEVMOUNTS_UDEV_DATA", self.root.join("udev"))
            .env("DEVMOUNTS_MOUNTINFO", self.root.join("mountinfo"))
            .env("DEVMOUNTS_DEV", self.root.join("dev"))
            .env("NO_COLOR", "1")
            .output()
            .expect("run bashrs")
    }

    fn nothing_linked(&self) -> bool {
        fs::read_dir(&self.links).unwrap().next().is_none()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Named nowhere, the CONFIG is the store's one config — applied like any other, and with
/// nothing more to say.
#[test]
fn a_bare_run_applies_the_one_config_in_the_store() {
    let rig = Rig::new("bare");
    fs::create_dir_all(rig.store()).unwrap();
    fs::write(rig.store().join("home.toml"), Rig::config("x")).unwrap();
    let out = rig.run(&[]);
    assert_eq!(out.status.code(), Some(0), "{}{}", stdout(&out), stderr(&out));
    assert_eq!(fs::read_link(rig.links.join("x")).unwrap(), rig.data);
    assert_eq!(fs::read_link(rig.links.join("docs_x")).unwrap(), rig.links.join("x/docs"));
    assert!(stdout(&out).contains("✓ everything is in place"), "{}", stdout(&out));
    assert!(!stderr(&out).contains("symlink_devs:"), "nothing to add: {}", stderr(&out));
}

/// A CONFIG named on the command line is the one applied, whatever the store holds.
#[test]
fn a_named_config_wins_over_the_store() {
    let rig = Rig::new("named");
    fs::create_dir_all(rig.store()).unwrap();
    fs::write(rig.store().join("home.toml"), Rig::config("x")).unwrap();
    let named = rig.root.join("other.toml");
    fs::write(&named, Rig::config("y")).unwrap();
    let out = rig.run(&[named.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}{}", stdout(&out), stderr(&out));
    assert!(rig.links.join("y").is_symlink() && !rig.links.join("x").exists());

    let missing = rig.root.join("missing.toml");
    let out = rig.run(&[missing.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("can't read config file"), "{}", stderr(&out));
    assert!(!stderr(&out).contains("symlink_devs:"), "a named CONFIG needs no word on the store: {}", stderr(&out));
}

/// Without exactly one config in the store, a bare run can't run: devmounts says the CONFIG is
/// missing, in its own words — as an `error:`, never by its own name, which the user needn't know —
/// and then bashrs says how to leave it out, and what the store holds instead.
#[test]
fn without_one_config_in_the_store_a_bare_run_says_how_to_go_without() {
    let rig = Rig::new("none");
    let says = |held: &str| {
        let out = rig.run(&[]);
        assert_eq!(out.status.code(), Some(2), "{}{}", stdout(&out), stderr(&out));
        let err = stderr(&out);
        let missing = err.find("error: no CONFIG given").unwrap_or_else(|| panic!("devmounts' own words: {err}"));
        let how = "symlink_devs: to leave CONFIG out, keep exactly one config (a .toml) in \
                   ~/.bashrs/user-custom/devmounts, ";
        let note = err.find(how).unwrap_or_else(|| panic!("the note: {err}"));
        assert!(missing < note, "devmounts' words first, then the note: {err}");
        assert!(!err.contains("devmounts:"), "nothing speaks as devmounts: {err}");
        assert!(err[note..].contains(held), "{err}");
        assert!(rig.nothing_linked());
    };
    says("which doesn't exist yet");
    fs::create_dir_all(rig.store()).unwrap();
    says("which holds none now");
    fs::write(rig.store().join("a.toml"), Rig::config("x")).unwrap();
    fs::write(rig.store().join("b.toml"), Rig::config("y")).unwrap();
    says("which holds 2 now: a.toml, b.toml");
}

/// The store's one config failing to parse is reported by devmounts — and named by bashrs, since
/// nobody named it on the command line.
#[test]
fn a_store_config_that_cannot_run_is_named() {
    let rig = Rig::new("broken");
    fs::create_dir_all(rig.store()).unwrap();
    let config = rig.store().join("home.toml");
    fs::write(&config, "[[device]\n").unwrap();
    let out = rig.run(&[]);
    assert_eq!(out.status.code(), Some(2), "{}{}", stdout(&out), stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("error: can't parse config") && !err.contains("devmounts:"), "{err}");
    assert!(err.contains(&format!("symlink_devs: that CONFIG was {}", config.display())), "{err}");
    assert!(rig.nothing_linked());
}

/// A step that needs root goes to devmounts' privileged helper — this very binary, started again
/// under the escalation command — and the binary answers as that helper (`REEXEC_HANDLERS`), not
/// as a command line it doesn't know. The stand-in `sudo` here grants nothing, so the helper meets
/// the same permission error and says so: an answer, where a binary deaf to the helper's marker
/// would leave every privileged step at "ended without answering".
#[test]
fn a_privileged_step_is_answered_by_this_binary_as_devmounts_helper() {
    // SAFETY: geteuid has no preconditions and can't fail.
    if unsafe { libc::geteuid() } == 0 {
        return; // root writes anywhere: no helper to ask
    }
    let rig = Rig::new("helper");
    let sudo = rig.root.join("bin/sudo");
    fs::write(&sudo, "#!/bin/sh\nexec \"$@\"\n").unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(rig.store()).unwrap();
    fs::write(rig.store().join("home.toml"), Rig::config("x")).unwrap();

    fs::set_permissions(&rig.links, fs::Permissions::from_mode(0o555)).unwrap();
    let out = rig.run_escalating(&[], sudo.to_str().unwrap());
    fs::set_permissions(&rig.links, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(1), "{}{}", stdout(&out), stderr(&out));
    let text = stdout(&out);
    assert!(text.contains(&format!("can't create {}: Permission denied", rig.links.join("x").display())), "{text}");
    assert!(!text.contains("ended without answering"), "the helper answered: {text}");
}
