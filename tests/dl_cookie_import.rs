//! Offline end-to-end tests of `dl --cookie-import` and the imported-store plumbing: real sqlite
//! cookie DBs inside fake browser profiles under a scratch HOME, the real binary, python3 doing
//! the actual row filtering, and the scripted yt-dlp stand-in (tests/fixtures/yt_dlp_stub.sh)
//! making the readability verdicts deterministic. The heart of it is the feature's privacy
//! contract: ONLY the target site's rows ever reach bashrs's disk. Network-free; the sqlite-backed
//! tests need a python3 (skip-with-notice otherwise).

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const YT_VIDEO: &str = "https://www.youtube.com/watch?v=stubvid0000";

/// Skip-with-notice: PATH python3 when it can do sqlite work (the same fallback the binary
/// itself lands on under a scratch HOME with no bundled tools), else `None` after a visible
/// SKIP line — the test then passes vacuously instead of failing on a bare machine.
fn python3_or_skip(test: &str) -> Option<PathBuf> {
    let works = Command::new("python3")
        .args(["-c", "import sqlite3"])
        .output()
        .is_ok_and(|out| out.status.success());
    if !works {
        eprintln!("SKIPPED {test}: no python3 (with sqlite3) on PATH");
    }
    works.then(|| PathBuf::from("python3"))
}

/// Run a python snippet with `args`, asserting success.
fn py(python: &Path, code: &str, args: &[&std::ffi::OsStr]) {
    let out = Command::new(python).arg("-c").arg(code).args(args).output().expect("run python3");
    assert!(out.status.success(), "python failed: {}", String::from_utf8_lossy(&out.stderr));
}

/// Run a python snippet with `args`, returning stdout.
fn py_read(python: &Path, code: &str, args: &[&std::ffi::OsStr]) -> String {
    let out = Command::new(python).arg("-c").arg(code).args(args).output().expect("run python3");
    assert!(out.status.success(), "python failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// One test's world: a scratch HOME (profiles + bashrs state), a download dir, and the yt-dlp
/// stand-in on PATH with its state dir. Removed on drop.
struct Rig {
    root: PathBuf,
    home: PathBuf,
    into: PathBuf,
    stub: PathBuf,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let root =
            std::env::temp_dir().join(format!("bashrs_dl_import_{tag}_{}", std::process::id()));
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

    /// Run `bashrs <args…>` in the rig's world, optionally feeding stdin (the `_pick` menu).
    fn run(&self, mode: &str, args: &[&str], stdin: Option<&str>) -> Output {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_bashrs"));
        cmd.args(args)
            .env("HOME", &self.home)
            .env("PATH", path)
            .env("BASHRS_STUB_DIR", &self.stub)
            .env("BASHRS_STUB_MODE", mode);
        match stdin {
            Some(text) => {
                cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
                let mut child = cmd.spawn().expect("spawn bashrs");
                child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
                child.wait_with_output().expect("run bashrs")
            }
            None => {
                cmd.stdin(Stdio::null());
                cmd.output().expect("run bashrs")
            }
        }
    }

    fn import(&self, mode: &str, target: &str) -> Output {
        self.run(mode, &["dl", "--cookie-import", target], None)
    }

    fn import_with_stdin(&self, mode: &str, target: &str, stdin: &str) -> Output {
        self.run(mode, &["dl", "--cookie-import", target], Some(stdin))
    }

    /// The per-site store dir bashrs writes under this rig's HOME.
    fn site_dir(&self, key: &str) -> PathBuf {
        self.home.join(".bashrs/user-data/browser_cookies").join(key)
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

/// Build a realistic Firefox profile DB (schema from tests/fixtures/moz_cookies.sql) in the
/// rig's HOME: two cookies for `site`, plus one from another site whose value is the canary
/// `SECRET`.
fn build_firefox_profile(python: &Path, rig: &Rig, site: &str) {
    let profile = rig.home.join(".mozilla/firefox/test.default-release");
    fs::create_dir_all(&profile).unwrap();
    let schema = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/moz_cookies.sql");
    py(
        python,
        r#"import sqlite3, sys, time
db, schema, site = sys.argv[1], sys.argv[2], sys.argv[3]
con = sqlite3.connect(db)
con.executescript(open(schema).read())
exp = int(time.time()) + 86400
rows = [("." + site, "sessionid", "s1"), ("www." + site, "csrf", "s2"),
        (".evil-bank.com", "account", "SECRET")]
for host, name, value in rows:
    con.execute(
        "INSERT INTO moz_cookies (name, value, host, path, expiry, lastAccessed, creationTime, isSecure, isHttpOnly) "
        "VALUES (?, ?, ?, '/', ?, 0, 0, 1, 1)",
        (name, value, host, exp))
con.commit()"#,
        &[profile.join("cookies.sqlite").as_os_str(), schema.as_os_str(), site.as_ref()],
    );
}

/// A Netscape dump the stub "extracts" for the readability check — 2 readable cookies.
fn readable_dump(rig: &Rig, site: &str) {
    fs::write(
        rig.stub.join("cookie_dump.txt"),
        format!(
            "# Netscape HTTP Cookie File\n.{site}\tTRUE\t/\tTRUE\t0\tsessionid\ts1\n.{site}\tTRUE\t/\tTRUE\t0\tcsrf\ts2\n"
        ),
    )
    .unwrap();
}

/// The cookie names in a store DB, sorted, one per line.
fn store_names(python: &Path, db: &Path) -> String {
    py_read(
        python,
        r#"import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
table = "moz_cookies" if con.execute("SELECT count(*) FROM sqlite_master WHERE name='moz_cookies'").fetchone()[0] else "cookies"
for (n,) in con.execute("SELECT name FROM %s ORDER BY name" % table):
    print(n)"#,
        &[db.as_os_str()],
    )
}

#[test]
fn cookie_import_copies_only_the_target_sites_rows_and_records_the_store() {
    let Some(python) = python3_or_skip("cookie import e2e") else { return };
    let rig = Rig::new("happy");
    build_firefox_profile(&python, &rig, "tiktok.com");
    readable_dump(&rig, "tiktok.com");

    let out = rig.import("ok", "tiktok");
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    assert!(all.contains("one cookie store found"), "a lone store auto-selects, no prompt: {all}");
    assert!(all.contains("imported 2 tiktok cookie(s)"), "{all}");
    assert!(all.contains("validated — 2 tiktok cookie(s) readable"), "readability verified: {all}");

    let site = rig.site_dir("tiktok");
    assert_eq!(fs::read_to_string(site.join("browser.spec")).unwrap().trim(), "firefox");
    let store_db = site.join("store/cookies.sqlite");
    assert_eq!(store_names(&python, &store_db), "csrf\nsessionid\n", "ONLY tiktok rows on disk");
    // The privacy contract, byte-level: the other site's cookie value must not appear ANYWHERE
    // in the copied store file.
    let leaked = fs::read(&store_db).unwrap().windows(6).any(|window| window == b"SECRET");
    assert!(!leaked, "a foreign cookie value leaked into the store file");

    // A re-import replaces the store — same two rows, never an accumulation.
    let out = rig.import("ok", "tiktok");
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(store_names(&python, &store_db), "csrf\nsessionid\n", "wiped and rebuilt, not merged");
}

#[test]
fn a_store_whose_cookies_cannot_be_read_back_is_discarded_not_kept_broken() {
    let Some(python) = python3_or_skip("zero-decrypt verdict") else { return };
    let rig = Rig::new("zerodecrypt");
    build_firefox_profile(&python, &rig, "tiktok.com");
    // The stub's "extraction" yields a dump with no cookie rows — the filtered store is useless.
    fs::write(rig.stub.join("cookie_dump.txt"), "# Netscape HTTP Cookie File\n# empty\n").unwrap();

    let out = rig.import("ok", "tiktok");
    assert!(!out.status.success(), "a discarded import must fail: {}", text(&out));
    assert!(text(&out).contains("none could be read"), "{}", text(&out));
    let site = rig.site_dir("tiktok");
    assert!(!site.join("browser.spec").exists(), "the spec must be forgotten");
    assert!(!site.join("store").exists(), "the store must be forgotten");
}

#[test]
fn an_unverifiable_import_keeps_the_store_provisionally() {
    let Some(python) = python3_or_skip("unverifiable verdict") else { return };
    let rig = Rig::new("unverifiable");
    build_firefox_profile(&python, &rig, "tiktok.com");
    // The readability probe itself dies (mode `fail`) — the count is unknowable, so the store
    // stays on its provisional count rather than being overclaimed or thrown away.
    let out = rig.import("fail", "tiktok");
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    assert!(all.contains("couldn't verify readability"), "{all}");
    assert!(rig.site_dir("tiktok").join("store/cookies.sqlite").exists(), "store kept");
}

#[test]
fn cookie_import_with_a_url_falls_through_to_a_download_using_the_fresh_store() {
    let Some(python) = python3_or_skip("import fall-through") else { return };
    let rig = Rig::new("fallthrough");
    build_firefox_profile(&python, &rig, "youtube.com");
    readable_dump(&rig, "youtube.com");

    let into = rig.into.to_string_lossy().into_owned();
    let out = rig.run(
        "ok",
        &["dl", "--cookie-import", "youtube", YT_VIDEO, "--into", &into],
        None,
    );
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    assert!(all.contains("imported 2 youtube cookie(s)"), "{all}");
    assert!(all.contains("YouTube rotates cookies"), "the rotation note rides a youtube import: {all}");
    // The download that follows must authenticate with the store imported moments ago.
    let calls = fs::read_to_string(rig.stub.join("calls.log")).unwrap_or_default();
    let download = calls.lines().find(|line| !line.contains("--print")).expect("a download ran");
    assert!(
        download.contains("--cookies-from-browser") && download.contains("browser_cookies/youtube/store"),
        "the fresh store must be used: {download}"
    );
}

#[test]
fn multiple_stores_prompt_a_menu_and_honor_or_reject_the_choice() {
    let Some(python) = python3_or_skip("store menu") else { return };
    let rig = Rig::new("menu");
    build_firefox_profile(&python, &rig, "tiktok.com");
    // A second store: a legacy flat Chromium profile (values encrypted; irrelevant to the menu).
    let chrome = rig.home.join(".config/google-chrome/Default");
    fs::create_dir_all(&chrome).unwrap();
    py(
        &python,
        r#"import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
con.execute("CREATE TABLE cookies (creation_utc INTEGER, host_key TEXT, name TEXT, encrypted_value BLOB)")
con.execute("CREATE TABLE meta (key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR)")
con.execute("INSERT INTO meta VALUES ('version', '24')")
con.executemany("INSERT INTO cookies (creation_utc, host_key, name, encrypted_value) VALUES (0, ?, ?, x'76')",
                [(".tiktok.com", "sessionid"), ("www.tiktok.com", "csrf")])
con.commit()"#,
        &[chrome.join("Cookies").as_os_str()],
    );
    readable_dump(&rig, "tiktok.com");

    // A number off the menu is rejected without touching anything.
    let out = rig.import_with_stdin("ok", "tiktok", "9\n");
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("not a listed number"), "{}", text(&out));
    assert!(!rig.site_dir("tiktok").join("browser.spec").exists(), "nothing written on a bad pick");

    // A valid pick imports from the chosen browser.
    let out = rig.import_with_stdin("ok", "tiktok", "1\n");
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    assert!(all.contains("Import tiktok cookies from:"), "{all}");
    assert!(all.contains(" 1) ") && all.contains(" 2) "), "a numbered menu: {all}");
    let spec = fs::read_to_string(rig.site_dir("tiktok").join("browser.spec")).unwrap();
    assert!(["firefox", "chrome"].contains(&spec.trim()), "a real browser was recorded: {spec}");
}

#[test]
fn an_expired_store_warns_in_red_before_the_download_and_a_live_one_stays_silent() {
    let Some(python) = python3_or_skip("expiry warning") else { return };
    // Seed an already-imported store directly (spec + a real cookie DB), with the given expiry.
    let seed = |rig: &Rig, expiry: &str| {
        let site = rig.site_dir("youtube");
        fs::create_dir_all(site.join("store")).unwrap();
        fs::write(site.join("browser.spec"), "firefox").unwrap();
        py(
            &python,
            r#"import sqlite3, sys, time
offset = 86400 if sys.argv[2] == "future" else -86400
con = sqlite3.connect(sys.argv[1])
con.execute("CREATE TABLE moz_cookies (expiry INTEGER)")
con.execute("INSERT INTO moz_cookies VALUES (?)", (int(time.time()) + offset,))
con.commit()"#,
            &[site.join("store/cookies.sqlite").as_os_str(), expiry.as_ref()],
        );
    };

    let rig = Rig::new("expired");
    seed(&rig, "past");
    let into = rig.into.to_string_lossy().into_owned();
    let out = rig.run("ok", &["dl", YT_VIDEO, "--into", &into], None);
    let all = text(&out);
    assert!(out.status.success(), "the warning must not block the download: {all}");
    assert!(
        all.contains("cookies have expired") && all.contains("--cookie-import youtube"),
        "the red heads-up names the fix: {all}"
    );

    let rig = Rig::new("live");
    seed(&rig, "future");
    let into = rig.into.to_string_lossy().into_owned();
    let out = rig.run("ok", &["dl", YT_VIDEO, "--into", &into], None);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!text(&out).contains("have expired"), "a live store must not cry wolf: {}", text(&out));
}

#[test]
fn an_unresolvable_import_target_is_rejected_before_touching_anything() {
    let rig = Rig::new("reject");
    let out = rig.import("ok", "tiktok2");
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("isn't a site I can target"), "{}", text(&out));
    assert!(
        !rig.home.join(".bashrs/user-data").exists(),
        "a rejected target must not create store state"
    );
}

#[test]
fn import_on_a_machine_with_no_browser_stores_reports_and_fails() {
    let rig = Rig::new("bare");
    let out = rig.import("ok", "tiktok");
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("no browser cookie stores found"), "{}", text(&out));
}
