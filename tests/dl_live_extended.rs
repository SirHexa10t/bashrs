//! The one live test bashrs still owns: that the cookie store `dl --cookies-extract-for-domain` writes is
//! readable by the genuine yt-dlp loader. Downloading is the `vidl` crate's business and its
//! live tests moved there with it; what stays here is the claim bashrs alone makes — that its
//! filtered, per-site copy of a browser's cookie DB is a thing yt-dlp will actually accept.
//!
//! Run it via TEST.sh, or directly:
//!
//! ```text
//! cargo test --test dl_live_extended -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! It skips-with-notice when no store is imported (`dl --cookies-extract-for-domain youtube` sets one up),
//! reads the real session ON PURPOSE — that is what it tests — and sends no network requests:
//! yt-dlp is asked to dump the store to a throwaway file, with no URL to fetch.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::Mutex;

static SERIAL: Mutex<()> = Mutex::new(());

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bashrs_dl_ext_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
            eprintln!("SKIPPED {test}: no imported youtube cookie store (run `dl --cookies-extract-for-domain youtube` first)");
            None
        }
    }
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
