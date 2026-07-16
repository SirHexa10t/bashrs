//! Offline end-to-end tests of `dl`'s failure handling and collection orchestration, driven by a
//! scripted yt-dlp stand-in (tests/fixtures/yt_dlp_stub.sh). Each test gets a scratch HOME (no
//! bundled tools, so the binary's resolution falls through to PATH) with the stub first on PATH —
//! deterministic and network-free. These complement the live network tests in dl_media_flags.rs;
//! they don't replace them.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const YT_VIDEO: &str = "https://www.youtube.com/watch?v=stubvid0000";

/// One test's world: a scratch root holding a fake HOME, a download dir, the stub's state dir,
/// and a bin dir with the stub installed as `yt-dlp`. Removed on drop.
struct Rig {
    root: PathBuf,
    home: PathBuf,
    into: PathBuf,
    stub: PathBuf,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let root = std::env::temp_dir().join(format!("bashrs_dl_stub_{tag}_{}", std::process::id()));
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
        Rig { root, home, into, stub }
    }

    /// Run `dl <url> --into <rig into> <flags…>` with the rig's HOME/PATH/stub environment.
    fn dl(&self, mode: &str, url: &str, flags: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .args(["dl", url, "--into"])
            .arg(&self.into)
            .args(flags)
            .env("HOME", &self.home)
            .env("PATH", path)
            .env("BASHRS_STUB_DIR", &self.stub)
            .env("BASHRS_STUB_MODE", mode)
            .output()
            .expect("run bashrs dl")
    }

    /// Every yt-dlp invocation the run made, one argv per line.
    fn calls(&self) -> String {
        fs::read_to_string(self.stub.join("calls.log")).unwrap_or_default()
    }

    fn ledger(&self) -> String {
        fs::read_to_string(self.into.join(".dl_video_failed_download.txt")).unwrap_or_default()
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

#[test]
fn a_youtube_geo_block_is_reported_as_ip_enforced_without_any_xff_sweep() {
    let rig = Rig::new("geo_ip");
    let out = rig.dl("geo", YT_VIDEO, &[]);
    assert!(!out.status.success(), "a dead geo-block must exit nonzero:\n{}", text(&out));
    let ledger = rig.ledger();
    assert!(
        ledger.contains("[stubvid0000]") && ledger.contains("geo-blocked (enforced by IP"),
        "honest IP-enforced line, keyed by id: {ledger}"
    );
    assert!(!ledger.contains("tried"), "no region list on the IP-enforced path: {ledger}");
    assert!(!rig.calls().contains("--xff"), "spoofing must not even be attempted:\n{}", rig.calls());
}

#[test]
fn a_generic_geo_block_sweeps_xff_regions_and_stops_at_the_first_win() {
    let rig = Rig::new("geo_xff");
    let out = rig.dl("geo_unless_xff_us", "https://media.example.com/v/1", &[]);
    assert!(out.status.success(), "the US spoof rescues the download:\n{}", text(&out));
    assert!(text(&out).contains("region US worked"), "{}", text(&out));
    let calls = rig.calls();
    assert_eq!(calls.matches("--xff").count(), 1, "stops at the first working region:\n{calls}");
    assert!(calls.contains("--xff US"), "US is the first region tried:\n{calls}");
    assert!(rig.ledger().is_empty(), "a rescued download must not be ledgered: {}", rig.ledger());
}

#[test]
fn a_transient_failure_succeeds_on_the_diagnostic_retry() {
    let rig = Rig::new("retry");
    let out = rig.dl("fail_once_then_ok", YT_VIDEO, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("succeeded on retry"), "{}", text(&out));
    assert!(rig.ledger().is_empty(), "a retried success must not be ledgered: {}", rig.ledger());
}

#[test]
fn gated_failures_land_honest_cookie_aware_ledger_lines() {
    // Members-only: no retry pretence, just the release-later reality.
    let rig = Rig::new("members");
    rig.dl("members", YT_VIDEO, &[]);
    assert!(rig.ledger().contains("members-only"), "{}", rig.ledger());

    // Age gate, no cookies anywhere: the fix (import) is named.
    let rig = Rig::new("age_plain");
    rig.dl("age", YT_VIDEO, &[]);
    assert!(
        rig.ledger().contains("needs cookies from a signed-in 18+ account"),
        "{}",
        rig.ledger()
    );

    // Age gate WITH an imported store in play: "add cookies" would be a lie — the harder-gate
    // wording must appear instead (the store below is junk, which is fine: only its presence
    // puts --cookies-from-browser on the argv).
    let rig = Rig::new("age_cookies");
    let site = rig.home.join(".bashrs/user-data/browser_cookies/youtube");
    fs::create_dir_all(site.join("store")).unwrap();
    fs::write(site.join("browser.spec"), "firefox").unwrap();
    fs::write(site.join("store/cookies.sqlite"), "junk").unwrap();
    rig.dl("age", YT_VIDEO, &[]);
    assert!(rig.ledger().contains("age-restricted despite cookies"), "{}", rig.ledger());

    // Bot-wall: honest that it may be undownloadable.
    let rig = Rig::new("botwall");
    rig.dl("botwall", YT_VIDEO, &[]);
    assert!(rig.ledger().contains("anti-bot/CAPTCHA"), "{}", rig.ledger());
}

#[test]
fn patch_flags_on_a_non_youtube_site_name_the_supported_platforms_and_skip() {
    let rig = Rig::new("notice");
    let out = rig.dl("ok", "https://media.example.com/v/2", &["--thumbnail", "--subtitles"]);
    assert!(out.status.success(), "{}", text(&out));
    let all = text(&out);
    assert!(all.contains("supported platforms: youtube"), "names what IS supported: {all}");
    assert!(
        !all.contains("thumbnails: scanning") && !all.contains("subtitles: scanning"),
        "the passes themselves must not run off-platform: {all}"
    );
}

#[test]
fn a_playlist_scan_reports_tombstones_skips_archived_and_downloads_pending_in_one_group() {
    let rig = Rig::new("playlist");
    // Four entries: one pending, one tombstone, one already archived, one more pending — the
    // two pending share a subtitle plan, so they must download as ONE grouped invocation.
    fs::write(
        rig.stub.join("scan.txt"),
        "1\tvidpend0001\tKeep One\tStub List\n\
         2\tvidpriv0002\t[Private video]\tStub List\n\
         3\tvidarch0003\tAlready Have\tStub List\n\
         4\tvidpend0004\tKeep Two\tStub List\n\
         Stub List [PLstub]\n",
    )
    .unwrap();
    fs::write(
        rig.stub.join("probe.txt"),
        "1\tvidpend0001\ten\t{\"en\": [{\"name\": \"English\"}]}\t{}\n\
         4\tvidpend0004\ten\t{\"en\": [{\"name\": \"English\"}]}\t{}\n",
    )
    .unwrap();
    let home = rig.into.join("Stub List [PLstub]");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join(".dl_video_archive.txt"), "youtube vidarch0003\n").unwrap();
    // What a successful group download archives (the real yt-dlp marks each completed entry) —
    // the post-mortem reads this to know nothing needs a diagnostic re-run.
    fs::write(rig.stub.join("archive_adds.txt"), "youtube vidpend0001\nyoutube vidpend0004\n")
        .unwrap();

    let out = rig.dl("ok", "https://www.youtube.com/playlist?list=PLstub", &[]);
    assert!(out.status.success(), "{}", text(&out));
    let all = text(&out);
    assert!(all.contains("1 of 4 entries are unplayable"), "{all}");
    let report = fs::read_to_string(home.join("unplayable__PLstub.txt")).expect("report written");
    assert!(report.contains("vidpriv0002"), "the tombstone is traced by id: {report}");
    assert!(all.contains("2 entries already archived (or unplayable) — skipped"), "{all}");
    assert!(all.contains("probing subtitles of 2 entries"), "{all}");
    assert!(all.contains("downloading 2 entries (subs: en)"), "{all}");
    let calls = rig.calls();
    assert_eq!(
        calls.lines().filter(|line| line.contains("--flat-playlist")).count(),
        1,
        "one scan:\n{calls}"
    );
    let downloads: Vec<&str> = calls.lines().filter(|line| !line.contains("--print")).collect();
    assert_eq!(downloads.len(), 1, "the shared subtitle plan downloads as one group:\n{calls}");
    assert!(downloads[0].contains("--playlist-items 1,4"), "both pending entries: {}", downloads[0]);
}
