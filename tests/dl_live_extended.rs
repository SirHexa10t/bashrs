//! The EXTENDED live category: real YouTube, very short videos, and — unlike every other suite —
//! deliberate use of the machine's own setup (bundled tools, and for two tests the user's
//! imported cookie store). These verify the contracts stubs freeze in time: yt-dlp's real scan
//! output, the CDN's thumbnail convention, the cookie store read back by the genuine loader, and
//! a restricted download through a real session.
//!
//! Run them via TEST.sh, or directly:
//!
//! ```text
//! cargo test --test dl_live_extended -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! House rules, because these touch live services and real credentials:
//! - Everything is serialized (a mutex, belt-and-braces with `--test-threads=1`) and each
//!   network test opens with a courtesy pause — a burst of requests is how IPs get flagged.
//! - Fixtures are tiny and historically stable (the first YouTube channel's 19-second video);
//!   the playlist test pre-flights the entry count and refuses to run if the fixture grew.
//! - Cookie-needing tests skip-with-notice when no store is imported (`dl --cookie-import
//!   youtube` sets one up). They use the real session ON PURPOSE — that's what they test — so
//!   they stay few, small, and paced.
//! - There is intentionally NO live channel test: a channel walk recurses into its playlists
//!   tab, and no public channel can promise that stays bounded. The channel path is covered by
//!   the stubbed suite; the uploads-playlist test validates the shared scan/download machinery
//!   live.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

/// 19 seconds, public, real `en`+`de` subtitles — the historically stable sample.
const VIDEO: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
const VIDEO_ID: &str = "jNQXAC9IVRw";
/// The same channel's uploads playlist: exactly that one video, for 20 years.
const TINY_PLAYLIST: &str = "https://www.youtube.com/playlist?list=UU4QobU6STFB0P71PMvOGN5A";
const TINY_PLAYLIST_ID: &str = "UU4QobU6STFB0P71PMvOGN5A";
/// A known age-restricted video (public knowledge from yt-dlp's issue tracker) — the smallest
/// honest exercise of a cookie-gated download.
const RESTRICTED: &str = "https://www.youtube.com/watch?v=zykMWuCsKyw";
const RESTRICTED_ID: &str = "zykMWuCsKyw";

static SERIAL: Mutex<()> = Mutex::new(());

/// A moment of courtesy before each network-hitting test — spread the load, don't burst.
fn courtesy_pause() {
    std::thread::sleep(std::time::Duration::from_secs(3));
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bashrs_dl_ext_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `dl <url> <flags…> --into <dir>` with the REAL environment (bundled tools, and the
/// machine's imported cookie store unless the caller passes `--no-cookies`).
fn dl(url: &str, dir: &Path, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["dl", url, "--into"])
        .arg(dir)
        .args(flags)
        .output()
        .expect("run bashrs dl")
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// A bundled tool when the bundle exists, else the bare PATH name.
fn tool(name: &str) -> PathBuf {
    let bundled = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".bashrs/tools/bin")
        .join(name);
    if bundled.exists() {
        bundled
    } else {
        PathBuf::from(name)
    }
}

/// The downloaded file carrying `__<id>.`, any media extension.
fn find_by_id(dir: &Path, id: &str) -> Option<PathBuf> {
    let marker = format!("__{id}.");
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|name| name.to_string_lossy().contains(&marker))
            {
                return Some(path);
            }
        }
    }
    None
}

/// Skip-with-notice: the machine's imported youtube cookie store as a `--cookies-from-browser`
/// spec, or `None` when nothing is imported (the cookie-gated tests then pass vacuously).
fn youtube_store_or_skip(test: &str) -> Option<String> {
    let site = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".bashrs/user-data/browser_cookies/youtube");
    let browser = std::fs::read_to_string(site.join("browser.spec")).ok();
    let store = site.join("store");
    match browser {
        Some(browser) if store.is_dir() => Some(format!("{}:{}", browser.trim(), store.display())),
        _ => {
            eprintln!("SKIPPED {test}: no imported youtube cookie store (run `dl --cookie-import youtube` first)");
            None
        }
    }
}

#[test]
#[ignore = "live-extended: hits YouTube; run via TEST.sh"]
fn a_tiny_uploads_playlist_downloads_end_to_end() {
    let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    courtesy_pause();
    // Pre-flight: refuse to run if the fixture ever grows — a live collection test must never
    // be an unbounded download.
    let scan = Command::new(tool("yt-dlp"))
        .args(["--force-ipv4", "--flat-playlist", "--print", "%(id)s", TINY_PLAYLIST])
        .output()
        .expect("run yt-dlp");
    assert!(scan.status.success(), "pre-flight scan failed: {}", text(&scan));
    let entries = String::from_utf8_lossy(&scan.stdout).lines().count();
    if entries == 0 || entries > 5 {
        eprintln!("SKIPPED tiny-playlist: fixture has {entries} entries (expected 1..=5)");
        return;
    }

    let dir = scratch("playlist");
    let out = dl(TINY_PLAYLIST, &dir, &["--no-cookies"]);
    assert!(out.status.success(), "{}", text(&out));
    // The playlist got its own folder, the archive recorded the entry, and the video landed.
    let home = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.is_dir() && path.file_name().is_some_and(|n| n.to_string_lossy().contains(TINY_PLAYLIST_ID))
        })
        .expect("a playlist folder named by the scan");
    let archive = std::fs::read_to_string(home.join(".dl_video_archive.txt")).expect("archive");
    assert!(archive.contains(VIDEO_ID), "the entry is archived: {archive}");
    assert!(find_by_id(&home, VIDEO_ID).is_some(), "the video file landed in the folder");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "live-extended: hits YouTube; run via TEST.sh"]
fn an_audio_download_lands_a_tagged_audio_file() {
    let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    courtesy_pause();
    let dir = scratch("audio");
    let out = dl(VIDEO, &dir, &["--audio", "--no-cookies"]);
    assert!(out.status.success(), "{}", text(&out));
    let file = find_by_id(&dir, VIDEO_ID).expect("an audio file landed");
    assert_ne!(file.extension().and_then(|e| e.to_str()), Some("mkv"), "audio mode, not video");
    // Whatever sidecars arrived must be folded into tags — never left as loose .vtt files.
    let stray_vtt = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .any(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("vtt"));
    assert!(!stray_vtt, "sidecars must be folded into tags:\n{}", text(&out));
    // Subtitle tracks are must-try: YouTube's caption endpoint 429s freely (datacenter IPs
    // especially), and a refused track downgrades to a warning by design. Only assert the
    // tagging when the subtitles actually arrived.
    if text(&out).contains("Unable to download video subtitles") {
        eprintln!("SKIPPED audio-tag verification: YouTube refused the subtitle tracks (429) this run");
    } else {
        let tags = Command::new(tool("ffprobe"))
            .args(["-v", "error", "-show_entries", "format_tags:stream_tags", "-of", "default=nw=1"])
            .arg(&file)
            .output()
            .expect("run ffprobe");
        let tags = String::from_utf8_lossy(&tags.stdout).to_lowercase();
        assert!(tags.contains("subtitles_en"), "the subtitle tag reads back: {tags}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "live-extended: hits the thumbnail CDN; run via TEST.sh"]
fn the_thumbnail_cdn_serves_hq_when_maxres_is_absent() {
    let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // Pins the convention fetch_youtube_thumbnail relies on: maxres is optional, hq is always
    // there. Two tiny HEAD-sized requests against the sample video.
    let code = |quality: &str| {
        let out = Command::new("curl")
            .args(["-fsSL", "-o", "/dev/null", "-w", "%{http_code}", "--max-time", "20"])
            .arg(format!("https://i.ytimg.com/vi/{VIDEO_ID}/{quality}.jpg"))
            .output()
            .expect("run curl");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert_eq!(code("hqdefault"), "200", "hqdefault must always exist");
    assert_eq!(code("maxresdefault"), "404", "this sample has no maxres — the fallback case");
}

#[test]
#[ignore = "live-extended (cookies): reads the user's imported store; run via TEST.sh"]
fn the_imported_store_reads_back_through_the_real_ytdlp() {
    let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(spec) = youtube_store_or_skip("store read-back") else { return };
    // Offline on purpose: no URL, so yt-dlp only extracts the store to a throwaway Netscape
    // file — the genuine loader validating what our filter wrote, with zero requests sent.
    // Modern yt-dlp then exits 2 ("You must provide at least one URL"), but only AFTER writing
    // the jar — the same tolerated-failure shape the product's readability check relies on. The
    // dump on disk is the verdict, not the exit code.
    let dir = scratch("readback");
    let probe = dir.join("probe-cookies.txt");
    let out = Command::new(tool("yt-dlp"))
        .args(["--cookies-from-browser", &spec, "--cookies"])
        .arg(&probe)
        .output()
        .expect("run yt-dlp");
    let dump = std::fs::read_to_string(&probe).unwrap_or_else(|_| {
        panic!("no cookie dump written — the loader failed outright:\n{}", text(&out))
    });
    let cookies = dump
        .lines()
        .map(|line| line.strip_prefix("#HttpOnly_").unwrap_or(line)) // an HttpOnly row is a cookie too
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .count();
    assert!(cookies > 0, "the real loader must read rows out of the imported store:\n{dump}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "live-extended (cookies): downloads a restricted video with the user's session; run via TEST.sh"]
fn an_age_restricted_video_downloads_with_the_users_cookies() {
    let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if youtube_store_or_skip("restricted download").is_none() {
        return;
    }
    courtesy_pause();
    // The one test that sends the real session to YouTube — deliberately a single, small
    // download (NOT --no-cookies: the auto-selected store is the point).
    let dir = scratch("restricted");
    let out = dl(RESTRICTED, &dir, &[]);
    assert!(
        out.status.success(),
        "a signed-in, age-verified session should clear the gate:\n{}",
        text(&out)
    );
    assert!(find_by_id(&dir, RESTRICTED_ID).is_some(), "the restricted video landed");
    let _ = std::fs::remove_dir_all(&dir);
}
