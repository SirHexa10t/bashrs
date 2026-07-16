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
    /// An extra dir for the child's PATH — the bundled tools' bin dir, for tests whose flow
    /// probes real media with ffprobe (the scratch HOME hides the bundle, so PATH must carry it).
    extra_path: Option<PathBuf>,
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
        Rig { root, home, into, stub, extra_path: None }
    }

    /// Run `dl <url> --into <rig into> <flags…>` with the rig's HOME/PATH/stub environment.
    fn dl(&self, mode: &str, url: &str, flags: &[&str]) -> Output {
        let extra = self
            .extra_path
            .as_ref()
            .map(|dir| format!("{}:", dir.display()))
            .unwrap_or_default();
        let path = format!(
            "{}:{extra}{}",
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

/// Skip-with-notice: the directory to put on the child's PATH so ffmpeg+ffprobe resolve (the
/// bundled tools' bin dir), `Some(None)` when the inherited PATH already carries them, or `None`
/// (after a visible SKIP line) when no ffmpeg exists at all.
fn ffmpeg_or_skip(test: &str) -> Option<Option<PathBuf>> {
    let works = |dir: Option<&Path>| {
        ["ffmpeg", "ffprobe"].iter().all(|name| {
            let bin = dir.map(|dir| dir.join(name)).unwrap_or_else(|| PathBuf::from(name));
            Command::new(bin).arg("-version").output().is_ok_and(|out| out.status.success())
        })
    };
    let bundled = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".bashrs/tools/bin"));
    if let Some(dir) = bundled.filter(|dir| works(Some(dir))) {
        return Some(Some(dir));
    }
    if works(None) {
        return Some(None);
    }
    eprintln!("SKIPPED {test}: no usable ffmpeg/ffprobe available");
    None
}

/// A tiny real mkv at `path` (needs [`ffmpeg_or_skip`] to have passed).
fn build_mkv(dir: Option<&Path>, path: &Path) {
    let ffmpeg = dir.map(|dir| dir.join("ffmpeg")).unwrap_or_else(|| PathBuf::from("ffmpeg"));
    let ok = Command::new(ffmpeg)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=blue:s=64x64:d=1"])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(path)
        .status()
        .is_ok_and(|status| status.success());
    assert!(ok, "could not build a test mkv");
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
    // Two entries sit far under the pacing threshold — the probe must not sleep between requests.
    let probe = calls.lines().find(|l| l.contains("--print") && l.contains("--playlist-items"));
    assert!(!probe.unwrap().contains("--sleep-requests"), "small probes must not pace:\n{calls}");
}

#[test]
fn a_channel_walks_every_tab_downloading_reporting_missing_and_failed_ones() {
    let rig = Rig::new("channel");
    // The stub keys tab behaviour off the URL: videos → this scan; shorts → yt-dlp's "no such
    // tab" error; streams → a hard failure; playlists → reachable but empty (no
    // scan_playlists.txt). One run exercises every TabScan outcome.
    fs::write(
        rig.stub.join("scan.txt"),
        "1\tvidchan0001\tClip One\tStub Channel\n\
         2\tvidchan0002\tClip Two\tStub Channel\n\
         Stub Channel [UCstub]\n",
    )
    .unwrap();
    fs::write(
        rig.stub.join("probe.txt"),
        "1\tvidchan0001\ten\t{\"en\": [{\"name\": \"English\"}]}\t{}\n\
         2\tvidchan0002\ten\t{\"en\": [{\"name\": \"English\"}]}\t{}\n",
    )
    .unwrap();
    fs::write(rig.stub.join("archive_adds.txt"), "youtube vidchan0001\nyoutube vidchan0002\n")
        .unwrap();

    let out = rig.dl("ok", "https://www.youtube.com/@stubchannel", &[]);
    assert!(out.status.success(), "one good tab means a good run: {}", text(&out));
    let all = text(&out);
    for tab in ["videos", "shorts", "streams", "playlists"] {
        assert!(
            all.contains(&format!("=== https://www.youtube.com/@stubchannel/{tab} ===")),
            "every tab is announced: {all}"
        );
    }
    assert!(all.contains("the channel has no `shorts` tab"), "{all}");
    assert!(all.contains("Unable to download webpage"), "a failed tab's real error passes through: {all}");
    assert!(all.contains("could not read the `streams` tab — moving on"), "{all}");
    assert!(all.contains("the channel has no `playlists` tab"), "reachable-but-empty reads as absent: {all}");
    assert!(all.contains("downloading 2 entries (subs: en)"), "{all}");
    // The channel's own folder (named by the first readable tab's scan) holds the shared archive.
    let archive = fs::read_to_string(rig.into.join("Stub Channel [UCstub]/.dl_video_archive.txt"))
        .expect("archive under the channel home");
    assert!(archive.contains("vidchan0001") && archive.contains("vidchan0002"), "{archive}");
    let calls = rig.calls();
    assert_eq!(calls.lines().filter(|line| line.contains("--flat-playlist")).count(), 4, "one scan per tab:\n{calls}");
    let downloads: Vec<&str> = calls.lines().filter(|line| !line.contains("--print")).collect();
    assert_eq!(downloads.len(), 1, "the videos tab downloads as one group:\n{calls}");
}

#[test]
fn a_probe_over_the_pacing_threshold_sleeps_between_requests() {
    let rig = Rig::new("pacing");
    // 21 pending entries (threshold is 20): a metadata burst that big paces itself.
    let mut scan = String::new();
    let mut archive_adds = String::new();
    for i in 1..=21 {
        scan.push_str(&format!("{i}\tvidpend{i:04}\tEntry {i}\tBig List\n"));
        archive_adds.push_str(&format!("youtube vidpend{i:04}\n"));
    }
    scan.push_str("Big List [PLbig]\n");
    fs::write(rig.stub.join("scan.txt"), scan).unwrap();
    fs::write(rig.stub.join("probe.txt"), "").unwrap(); // probe answers nothing → EN fallback
    fs::write(rig.stub.join("archive_adds.txt"), archive_adds).unwrap();

    let out = rig.dl("ok", "https://www.youtube.com/playlist?list=PLbig", &[]);
    assert!(out.status.success(), "{}", text(&out));
    let calls = rig.calls();
    let probe = calls
        .lines()
        .find(|line| line.contains("--print") && line.contains("--playlist-items"))
        .expect("a batch probe ran");
    assert!(probe.contains("--sleep-requests 1"), "big probes must pace themselves:\n{probe}");
}

#[test]
fn no_cookies_keeps_an_imported_store_out_of_every_invocation() {
    // The same seeded store, with and without --no-cookies: the flag must keep it off the argv.
    let with_store = |tag: &str, flags: &[&str]| {
        let rig = Rig::new(tag);
        let site = rig.home.join(".bashrs/user-data/browser_cookies/youtube");
        fs::create_dir_all(site.join("store")).unwrap();
        fs::write(site.join("browser.spec"), "firefox").unwrap();
        fs::write(site.join("store/cookies.sqlite"), "junk").unwrap();
        let out = rig.dl("ok", YT_VIDEO, flags);
        assert!(out.status.success(), "{}", text(&out));
        rig.calls()
    };
    let used = with_store("cookies_on", &[]);
    assert!(used.contains("--cookies-from-browser"), "the store is the standing default:\n{used}");
    let suppressed = with_store("cookies_off", &["--no-cookies"]);
    assert!(
        !suppressed.contains("--cookies-from-browser"),
        "--no-cookies must keep the store out of every yt-dlp call:\n{suppressed}"
    );
}

#[test]
fn the_subtitle_pass_reports_audio_files_as_tag_carriers_and_skips_them() {
    let rig = Rig::new("subs_audio");
    fs::write(rig.into.join("song__stubvid0000.opus"), b"OPUSDATA").unwrap();
    let out = rig.dl("ok", YT_VIDEO, &["--subtitles"]);
    assert!(out.status.success(), "{}", text(&out));
    let all = text(&out);
    assert!(all.contains("already downloaded — scanning subtitles"), "{all}");
    assert!(all.contains("audio — subtitles kept as tags; nothing to patch"), "{all}");
    assert_eq!(
        fs::read(rig.into.join("song__stubvid0000.opus")).unwrap(),
        b"OPUSDATA",
        "the audio file stays untouched"
    );
}

#[test]
fn an_unreadable_video_file_is_reported_and_left_alone() {
    let rig = Rig::new("subs_junk");
    fs::write(rig.into.join("v__stubvid0000.mkv"), b"not really an mkv").unwrap();
    let out = rig.dl("ok", YT_VIDEO, &["--subtitles"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("could not read (ffprobe) — skipping"), "{}", text(&out));
    assert_eq!(
        fs::read(rig.into.join("v__stubvid0000.mkv")).unwrap(),
        b"not really an mkv",
        "an unreadable file must not be modified"
    );
}

#[test]
fn a_subtitle_fetch_that_yields_no_sidecars_is_reported_as_rate_limited() {
    let Some(ffmpeg_dir) = ffmpeg_or_skip("no-sidecars branch") else { return };
    let mut rig = Rig::new("subs_dry");
    rig.extra_path = ffmpeg_dir.clone();
    build_mkv(ffmpeg_dir.as_deref(), &rig.into.join("v__stubvid0000.mkv"));
    // The probe advertises an `en` track, but the stub's "download" writes no sidecar files.
    fs::write(
        rig.stub.join("video_probe.txt"),
        "stubvid0000\nen\n{\"en\": [{\"name\": \"English\"}]}\n{}\n",
    )
    .unwrap();
    let out = rig.dl("ok", YT_VIDEO, &["--subtitles"]);
    assert!(out.status.success(), "{}", text(&out));
    let all = text(&out);
    assert!(all.contains("missing subtitle(s): en"), "{all}");
    assert!(all.contains("no subtitles arrived (rate-limited?)"), "{all}");
}

#[test]
fn a_video_with_no_subtitles_anywhere_reads_as_nothing_to_embed_not_all_zero() {
    let Some(ffmpeg_dir) = ffmpeg_or_skip("no-subtitles wording") else { return };
    let mut rig = Rig::new("subs_none");
    rig.extra_path = ffmpeg_dir.clone();
    build_mkv(ffmpeg_dir.as_deref(), &rig.into.join("v__stubvid0000.mkv"));
    // Default probe: NA everywhere → the expected set is empty.
    let out = rig.dl("ok", YT_VIDEO, &["--subtitles"]);
    assert!(out.status.success(), "{}", text(&out));
    let all = text(&out);
    assert!(all.contains("no subtitles exist for this video — nothing to embed"), "{all}");
    assert!(!all.contains("all 0 expected"), "the absurd wording must be gone: {all}");
}
