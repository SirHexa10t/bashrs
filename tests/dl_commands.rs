//! Offline tests of what bashrs adds to the commands it builds over the `vidl` crate, `dl` and
//! `dl_fix`: the yt-dlp it pins, where it keeps imported cookies, the import flag it narrows to the
//! target alone, and the repair options it passes through whole. What vidl does with them — the
//! import itself, the store's use, the repair — is vidl's, and tested there.
//!
//! The child's world is HERMETIC: its PATH holds only the rig's bin dir — the stand-in as `yt-dlp`
//! (tests/fixtures/yt_dlp_stub.sh) and `bash` for its shebang — and its HOME and working directory
//! are the rig's. Network-free.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const YT_VIDEO: &str = "https://www.youtube.com/watch?v=stubvid0000";

/// The first existing file among fixed guesses for `name`, then every PATH dir — for wiring
/// real binaries (bash) into the rig's hermetic bin dir by absolute path.
fn locate(name: &str, fixed: &[&str]) -> Option<PathBuf> {
    let from_path = std::env::var_os("PATH").map(|paths| {
        std::env::split_paths(&paths).map(|dir| dir.join(name)).collect::<Vec<_>>()
    });
    fixed
        .iter()
        .map(PathBuf::from)
        .chain(from_path.unwrap_or_default())
        .find(|candidate| candidate.is_file())
}

/// One test's world: a scratch HOME (bashrs state), a download dir, the stub's state dir, and the
/// hermetic bin dir that becomes the child's ENTIRE PATH. Removed on drop.
struct Rig {
    root: PathBuf,
    home: PathBuf,
    into: PathBuf,
    stub: PathBuf,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let root = std::env::temp_dir().join(format!("bashrs_dl_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (home, into, stub, bin) =
            (root.join("home"), root.join("into"), root.join("stub"), root.join("bin"));
        for dir in [&home, &into, &stub, &bin] {
            fs::create_dir_all(dir).unwrap();
        }
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/yt_dlp_stub.sh");
        let ytdlp = bin.join("yt-dlp");
        fs::copy(&script, &ytdlp).unwrap();
        fs::set_permissions(&ytdlp, fs::Permissions::from_mode(0o755)).unwrap();
        // The stub's `#!/usr/bin/env bash` resolves bash via the child's (hermetic) PATH.
        if let Some(bash) = locate("bash", &["/bin/bash", "/usr/bin/bash"]) {
            let _ = std::os::unix::fs::symlink(bash, bin.join("bash"));
        }
        Rig { root, home, into, stub }
    }

    fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }

    /// Run `bashrs <args…>` in the rig's world. PATH is the rig's bin dir ALONE — hermetic on
    /// purpose, so the stand-in is the only yt-dlp there is unless a test pins one — and the
    /// working directory is the rig's too, so a `dl_fix` that took the wrong folder repairs a
    /// scratch one, never the checkout.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .args(args)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("PATH", self.bin())
            .env("BASHRS_STUB_DIR", &self.stub)
            .stdin(Stdio::null())
            .output()
            .expect("run bashrs")
    }

    /// Every yt-dlp invocation the run made, one argv per line.
    fn calls(&self) -> String {
        fs::read_to_string(self.stub.join("calls.log")).unwrap_or_default()
    }

    /// Make the stub bashrs's pinned yt-dlp — `~/.bashrs/tools/yt-dlp/bin/yt-dlp`, where a
    /// bundle is looked for — and the only one: it leaves PATH, so a run that reaches it was
    /// handed it.
    fn pin_ytdlp(&self) {
        let bundle = self.home.join(".bashrs/tools/yt-dlp/bin");
        fs::create_dir_all(&bundle).unwrap();
        fs::rename(self.bin().join("yt-dlp"), bundle.join("yt-dlp")).unwrap();
    }

    /// Seed a ready-made store for `site` where bashrs keeps them, skipping the import flow. Its
    /// contents are never read by the stub — what matters downstream is only that a store EXISTS
    /// to be selected.
    fn seed_store(&self, site: &str) {
        let dir = self.home.join(".bashrs/user-appdata/browser_cookies").join(site);
        fs::create_dir_all(dir.join("store")).unwrap();
        fs::write(dir.join("browser.spec"), "firefox").unwrap();
        fs::write(dir.join("store/cookies.sqlite"), "junk").unwrap();
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// Where bashrs keeps imported cookies is where every `dl` run reads them: a store under
/// `~/.bashrs/user-appdata/browser_cookies` is the standing default on its site — the root set on
/// each run, not only an import's, or a store found while importing would be lost straight after.
#[test]
fn a_store_kept_in_bashrs_data_dir_is_read_by_every_dl_run() {
    let rig = Rig::new("store_read");
    rig.seed_store("youtube");
    let into = rig.into.display().to_string();
    let out = rig.run(&["dl", YT_VIDEO, "--into", &into]);
    assert!(out.status.success(), "{}", text(&out));
    let calls = rig.calls();
    assert!(
        calls.contains("--cookies-from-browser") && calls.contains(".bashrs/user-appdata/browser_cookies/youtube/store"),
        "the store bashrs keeps is the one used:\n{calls}"
    );
}

/// `dl --cookies-extract-for-domain` takes the target alone — vidl's two-value flag narrowed,
/// bashrs naming the destination itself — so a target is all an import asks for here. One no
/// import can serve shows it without a browser: refused as a target, not for want of a
/// directory, and leaving nothing behind in bashrs's data dir.
#[test]
fn dl_imports_with_the_target_alone_bashrs_naming_the_destination() {
    let rig = Rig::new("import_target");
    let out = rig.run(&["dl", "--cookies-extract-for-domain", "tiktok2"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("isn't a site I can target"), "{}", text(&out));
    assert!(!rig.home.join(".bashrs/user-appdata").exists(), "a rejected target must not create store state");
}

/// What bashrs hands vidl for a repair: the yt-dlp it pins, proven by being the only one there
/// is, and the cookie store it keeps, proven by its spec on the call that asks YouTube about the
/// video. What the repair then makes of the answer is vidl's, and tested there; here the stand-in
/// gives none, and the hermetic PATH no ffprobe, so the video stays unread and the run fails.
#[test]
fn dl_fix_asks_youtube_with_the_pinned_yt_dlp_and_the_imported_store() {
    let rig = Rig::new("fix_seam");
    rig.pin_ytdlp();
    rig.seed_store("youtube");
    fs::write(rig.into.join("A Talk__abcdefghijk.mkv"), "not really a video").unwrap();
    let into = rig.into.display().to_string();
    let out = rig.run(&["dl_fix", &into, "--check"]);
    let all = text(&out);
    assert_eq!(out.status.code(), Some(1), "a video left unread fails the run: {all}");
    assert!(all.contains("repair --check:") && all.contains("nothing was changed"), "--check reaches the repair: {all}");
    let calls = rig.calls();
    let asked = calls.lines().find(|line| line.contains("watch?v=abcdefghijk")).expect("YouTube was asked about the video");
    assert!(
        asked.contains("--cookies-from-browser") && asked.contains("browser_cookies/youtube/store"),
        "the imported store must be used: {asked}"
    );
}

/// `dl`'s half of the handoff `dl_fix` shares: with the stub made the only yt-dlp there is, a
/// download still reaches it.
#[test]
fn dl_downloads_with_the_pinned_yt_dlp() {
    let rig = Rig::new("pinned_dl");
    rig.pin_ytdlp();
    let into = rig.into.display().to_string();
    let out = rig.run(&["dl", YT_VIDEO, "--into", &into]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(rig.calls().contains(YT_VIDEO), "the pinned copy ran:\n{}", rig.calls());
}

/// `dl_fix` takes everything `vidl repair` does — vidl's own struct, flattened — and no download
/// option: what a repair shares with a download reaches it (`--no-cookies` keeps even an imported
/// store out of what YouTube is asked), and its cookie import takes the target alone, bashrs
/// supplying the store's place as it does for `dl`.
#[test]
fn dl_fix_takes_every_option_vidl_repair_does_and_no_download_option() {
    let rig = Rig::new("fix_options");
    rig.seed_store("youtube");
    let library = rig.root.join("library");
    fs::create_dir_all(&library).unwrap();
    fs::write(library.join("A Talk__abcdefghijk.mkv"), "not really a video").unwrap();
    let library = library.display().to_string();
    rig.run(&["dl_fix", "--no-cookies", &library, "--check"]);
    let asked = rig.calls();
    assert!(
        asked.contains("watch?v=abcdefghijk") && !asked.contains("--cookies-from-browser"),
        "--no-cookies reached the repair:\n{asked}"
    );

    // The empty scratch folder is named, so a run that wrongly went ahead completes nothing.
    let into = rig.into.display().to_string();
    let import = rig.run(&["dl_fix", "--cookies-extract-for-domain", "tiktok2", &into]);
    let all = text(&import);
    assert!(all.contains("isn't a site I can target"), "the import was tried: {all}");
    assert!(
        all.contains(&format!("no downloaded YouTube videos under {into}")),
        "and the folder after it repaired, not taken for the import's destination: {all}"
    );
    let audio = rig.run(&["dl_fix", "--audio", &into]);
    assert!(text(&audio).contains("unexpected argument '--audio'"), "a download's option: {}", text(&audio));
}

/// `dl` takes a download's options alone — no `dl repair` beside `dl_fix`, which completion could
/// not reach anyway — and a path with nothing at it is refused before YouTube is asked anything,
/// with the exit code vidl gives it.
#[test]
fn dl_holds_no_repair_and_dl_fix_refuses_a_path_with_nothing_at_it() {
    let rig = Rig::new("fix_refusals");
    let help = text(&rig.run(&["dl", "--help"]));
    assert!(help.contains("Usage:") && !help.contains("Commands:"), "`dl` holds no commands: {help}");
    let gone = rig.root.join("nothing_here").display().to_string();
    let missing = rig.run(&["dl_fix", &gone]);
    assert_eq!(missing.status.code(), Some(2), "{}", text(&missing));
    assert!(text(&missing).contains("there is nothing at"), "{}", text(&missing));
    assert!(rig.calls().is_empty(), "nothing was asked of YouTube:\n{}", rig.calls());
}
