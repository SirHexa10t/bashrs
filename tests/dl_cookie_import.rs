//! Offline end-to-end tests of `dl --cookie-import`: a real sqlite cookie DB inside a fake
//! Firefox profile under a scratch HOME, the real binary, and python3 doing the actual row
//! filtering — pinning the feature's privacy contract: ONLY the target site's rows ever reach
//! bashrs's disk. Network-free; needs a python3 with sqlite3 (skip-with-notice otherwise).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

/// Run a python snippet with `args`, returning stdout (asserting success).
fn py(python: &Path, code: &str, args: &[&Path]) -> String {
    let out = Command::new(python).arg("-c").arg(code).args(args).output().expect("run python3");
    assert!(out.status.success(), "python failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bashrs_dl_import_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `dl --cookie-import <target>` with `home` as the whole world.
fn dl_import(home: &Path, target: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["dl", "--cookie-import", target])
        .env("HOME", home)
        .output()
        .expect("run bashrs dl")
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// Build a realistic Firefox profile DB (schema from tests/fixtures/moz_cookies.sql): two
/// tiktok.com cookies and one from another site whose value is the canary `SECRET`.
fn build_profile_db(python: &Path, db: &Path) {
    let schema = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/moz_cookies.sql");
    py(
        python,
        r#"import sqlite3, sys, time
db, schema = sys.argv[1], sys.argv[2]
con = sqlite3.connect(db)
con.executescript(open(schema).read())
exp = int(time.time()) + 86400
rows = [(".tiktok.com", "sessionid", "s1"), ("www.tiktok.com", "csrf", "s2"),
        (".evil-bank.com", "account", "SECRET")]
for host, name, value in rows:
    con.execute(
        "INSERT INTO moz_cookies (name, value, host, path, expiry, lastAccessed, creationTime, isSecure, isHttpOnly) "
        "VALUES (?, ?, ?, '/', ?, 0, 0, 1, 1)",
        (name, value, host, exp))
con.commit()"#,
        &[db, &schema],
    );
}

/// The cookie names in a store DB, sorted, one per line.
fn store_names(python: &Path, db: &Path) -> String {
    py(
        python,
        r#"import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
for (n,) in con.execute("SELECT name FROM moz_cookies ORDER BY name"):
    print(n)"#,
        &[db],
    )
}

#[test]
fn cookie_import_copies_only_the_target_sites_rows_and_records_the_store() {
    let Some(python) = python3_or_skip("cookie import e2e") else { return };
    let home = scratch("happy");
    let profile = home.join(".mozilla/firefox/test.default-release");
    fs::create_dir_all(&profile).unwrap();
    build_profile_db(&python, &profile.join("cookies.sqlite"));

    let out = dl_import(&home, "tiktok");
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    assert!(all.contains("one cookie store found"), "a lone store auto-selects, no prompt: {all}");
    assert!(all.contains("imported 2 tiktok cookie(s)"), "{all}");

    let site = home.join(".bashrs/user-data/browser_cookies/tiktok");
    assert_eq!(fs::read_to_string(site.join("browser.spec")).unwrap().trim(), "firefox");
    let store_db = site.join("store/cookies.sqlite");
    assert_eq!(store_names(&python, &store_db), "csrf\nsessionid\n", "ONLY tiktok rows on disk");
    // The privacy contract, byte-level: the other site's cookie value must not appear ANYWHERE
    // in the copied store file.
    let leaked = fs::read(&store_db).unwrap().windows(6).any(|window| window == b"SECRET");
    assert!(!leaked, "a foreign cookie value leaked into the store file");

    // A re-import replaces the store — same two rows, never an accumulation.
    let out = dl_import(&home, "tiktok");
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(store_names(&python, &store_db), "csrf\nsessionid\n", "wiped and rebuilt, not merged");

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn an_unresolvable_import_target_is_rejected_before_touching_anything() {
    let home = scratch("reject");
    let out = dl_import(&home, "tiktok2");
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("isn't a site I can target"), "{}", text(&out));
    assert!(
        !home.join(".bashrs/user-data").exists(),
        "a rejected target must not create store state"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn import_on_a_machine_with_no_browser_stores_reports_and_fails() {
    let home = scratch("bare");
    let out = dl_import(&home, "tiktok");
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("no browser cookie stores found"), "{}", text(&out));
    let _ = fs::remove_dir_all(&home);
}
