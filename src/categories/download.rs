//! Download commands (`dl_*`) — pulling files off the web. Pages are fetched and files
//! downloaded through `curl` (the same external the tool fetcher trusts); YouTube goes through
//! the bundled `yt-dlp` ([`crate::tools`]).

#[bashrs_macros::category(command = DownloadCommand, prefix = "dl_")]
mod commands {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    use crate::support::exec::{capture_stdout, run_reporting};
    use crate::drivers::youtube;
    use crate::support::browsers;
    use crate::support::doc_render;
    use crate::support::doc_style::{self, _header};
    use clap::Args;

    /// Download every link of the given file types found in a webpage, into the current dir
    pub fn page_links(args: PageLinksArgs) {
        let PageLinksArgs { page, types, list } = args;
        let Some(html) = capture_stdout("curl", ["-fsSL", &page]) else {
            eprintln!("dl_page_links: could not fetch {page}");
            std::process::exit(1);
        };
        let exts: Vec<String> = types.iter().map(|ext| _normalize_ext(ext)).collect();
        let mut found = 0u32;
        let mut failed = 0u32;
        for (ext, links) in exts.iter().zip(_links_by_type(&html, &exts)) {
            if links.is_empty() {
                eprintln!("no .{ext} links in {page}");
                continue;
            }
            for link in links {
                found += 1;
                let url = _resolve_link(&page, &link);
                if list {
                    println!("{url}");
                } else {
                    println!("downloading {url}");
                    // `--no-progress-meter`, not `-s`: transfer noise off, real errors still on.
                    if !run_reporting("curl", ["-fSLO", "--retry", "2", "--no-progress-meter", &url]) {
                        failed += 1;
                    }
                }
            }
        }
        if found == 0 || failed > 0 {
            std::process::exit(1);
        }
    }

    #[derive(Args)]
    pub struct PageLinksArgs {
        /// The webpage to scan
        pub page: String,
        /// File types to fetch — e.g. `.png mp3 WEBP` (leading dot optional, case-insensitive)
        #[arg(required = true, num_args = 1..)]
        pub types: Vec<String>,
        /// Only print the resolved links, downloading nothing
        #[arg(short, long)]
        pub list: bool,
    }

    /// A type argument as a bare lowercase extension: `.PNG` and `png` both mean `png`.
    fn _normalize_ext(ext: &str) -> String {
        ext.trim_start_matches('.').to_lowercase()
    }

    /// One pass over the page: every `"`-delimited token is a link candidate (that's where HTML
    /// keeps its URLs), matched case-insensitively against every wanted type. Each hit lands
    /// once — under its first matching type — and whitespace-bearing tokens are prose, not URLs.
    fn _links_by_type(html: &str, exts: &[String]) -> Vec<Vec<String>> {
        let suffixes: Vec<String> = exts.iter().map(|ext| format!(".{ext}")).collect();
        let mut groups: Vec<Vec<String>> = vec![Vec::new(); exts.len()];
        let mut seen = HashSet::new();
        for token in html.split('"') {
            if token.is_empty() || token.chars().any(char::is_whitespace) {
                continue;
            }
            let lower = token.to_lowercase();
            if let Some(hit) = suffixes.iter().position(|suffix| lower.ends_with(suffix)) {
                if seen.insert(token.to_string()) {
                    groups[hit].push(token.to_string());
                }
            }
        }
        groups
    }

    /// Resolve one found link against the page's URL, the way a browser would: absolute links
    /// pass through; `//host/…` borrows the page's scheme; `/rooted` starts at the page's host;
    /// anything else is relative to the page's directory, with `./` and `dir/../` collapsed.
    fn _resolve_link(page: &str, link: &str) -> String {
        if link.contains("://") {
            return link.to_string();
        }
        let (scheme, rest) = page.split_once("://").unwrap_or(("https", page));
        if let Some(host_and_path) = link.strip_prefix("//") {
            return format!("{scheme}://{host_and_path}");
        }
        let host = rest.split('/').next().unwrap_or(rest);
        if link.starts_with('/') {
            return format!("{scheme}://{host}{link}");
        }
        let dir = match rest.rsplit_once('/') {
            Some((dir, _page_name)) => dir,
            None => rest, // the page sits at the host root
        };
        format!("{scheme}://{}", _squash(&format!("{dir}/{link}")))
    }

    /// Collapse `.`, empty (`//`), and `segment/..` path steps — never popping the leading
    /// segment (the host), so `../` can't climb above it.
    fn _squash(path: &str) -> String {
        let absolute = path.starts_with('/');
        let mut stack: Vec<&str> = Vec::new();
        for segment in path.split('/') {
            match segment {
                "" | "." => {}
                ".." if stack.len() > 1 => {
                    stack.pop();
                }
                ".." => {}
                other => stack.push(other),
            }
        }
        let joined = stack.join("/");
        if absolute {
            format!("/{joined}")
        } else {
            joined
        }
    }

    // --- dl ------------------------------------------------------------------

    /// `~/.bashrs/user-data/browser_cookies` — the base holding one subdir per imported site
    /// (`<key>/store/` + `<key>/browser.spec`). `--cookie-import` writes a site's subdir; each
    /// `dl <url>` run reads the subdir matching the URL's site.
    fn _cookie_store_dir() -> PathBuf {
        crate::conf::user_data_dir().join("browser_cookies")
    }

    /// The `--cookie-import <target>` action: resolve the target site, scan for browser cookie
    /// stores, let the user pick one, and copy *only that site's* cookies into a per-site dir
    /// under bashrs's data (which a running browser can't lock). Later `dl` runs on that site
    /// use it automatically. `target` is a site keyword, a domain, or a URL. Returns whether a
    /// usable store was imported.
    fn _import_cookies(target: &str) -> bool {
        // Reject a target we can't map to cookie domains (a bare word like `tiktok2`) up front —
        // otherwise we'd build an empty store and report a misleading "no cookies found".
        if !browsers::is_importable_target(target) {
            eprintln!(
                "dl: '{target}' isn't a site I can target — pass a keyword ({}), a domain (example.com), or a video URL",
                browsers::known_site_names()
            );
            return false;
        }
        let site = browsers::resolve_target(target);
        let home = crate::conf::home();
        let stores = browsers::cookie_stores(&home);
        if stores.is_empty() {
            if browsers::any_browser_installed(&home) {
                eprintln!("dl: found browsers but no cookie stores yet — browse (and sign in) once, then retry");
            } else {
                eprintln!("dl: no browser cookie stores found on this system");
            }
            return false;
        }
        let Some(store) = _pick(&stores, &site.label) else { return false };
        let site_dir = _cookie_store_dir().join(&site.key);
        let store_dir = match browsers::reset_site(&site_dir) {
            Ok(dir) => dir,
            Err(err) => {
                eprintln!("dl: could not prepare the {} cookie store: {err}", site.label);
                return false;
            }
        };
        // Cherry-pick: only the target site's cookies are copied — never the whole DB.
        let Some(matched) = youtube::filter_cookie_db(store, &store_dir, &site.domains) else {
            let _ = browsers::forget(&site_dir);
            eprintln!("dl: could not filter the cookie store (is the bundled python available?)");
            return false;
        };
        if matched == 0 {
            let _ = browsers::forget(&site_dir);
            eprintln!(
                "dl: no {} cookies in {} — sign in to {} in that browser/profile, then re-import",
                site.label, store.label, site.label
            );
            return false;
        }
        if let Err(err) = browsers::write_spec(&site_dir, store.browser) {
            let _ = browsers::forget(&site_dir);
            eprintln!("dl: could not record the {} cookie store: {err}", site.label);
            return false;
        }
        println!("imported {matched} {} cookie(s) from {}", site.label, store.label);
        if site.key == "youtube" {
            println!("note: YouTube rotates cookies on open tabs — if auth later fails, re-export via a private window (sign in, open only youtube.com/robots.txt, then close it) and re-import");
        }
        _report_cookie_check(&site, store.browser, matched, &site_dir)
    }

    /// Warn (in red) when the imported store a download is about to use holds only expired cookies,
    /// so a gated fetch that's about to fail for a stale session says so up front — with the
    /// re-import fix — rather than surfacing as a bare auth error. Silent when the store is fresh or
    /// the check can't run.
    fn _warn_if_cookies_expired(site_dir: &Path, site: &browsers::SiteTarget) {
        let Some((db, kind)) = browsers::imported_db(site_dir) else { return };
        if youtube::cookies_expired(&db, kind) == Some(true) {
            eprintln!(
                "{}",
                doc_style::problematic(&format!(
                    "dl: the imported {} cookies have expired — re-import with `dl --cookie-import {}`",
                    site.label, site.key
                ))
            );
        }
    }

    /// Read the freshly imported (already domain-filtered) store back through yt-dlp to confirm
    /// the cookies actually *decrypt* — a keyring-locked Chromium store filters fine but reads
    /// back empty — and report the concrete result, catching that failure now instead of on
    /// every later run. Returns whether a usable store remains. When the read-back can't run
    /// (yt-dlp not bundled yet), keeps the store on the provisional count rather than overclaiming.
    fn _report_cookie_check(site: &browsers::SiteTarget, browser: &str, matched: usize, site_dir: &Path) -> bool {
        // The WAL caveat holds whenever cookies came through: a copy captures only what the
        // browser already flushed to the DB, so a sign-in from moments ago may not be in it yet.
        let wal_note = || println!("note: if a fresh sign-in isn't recognized, fully quit the browser and re-import");
        let Some(spec) = browsers::imported_spec(site_dir) else { return true };
        match youtube::readable_cookie_count(&spec, site_dir) {
            Some(n) if n > 0 => {
                println!("validated — {n} {} cookie(s) readable; dl runs on {} will use them", site.label, site.label);
                wal_note();
                true
            }
            Some(_) => {
                // Filtered rows but none decrypt → useless; drop it so future runs don't keep
                // re-attempting a doomed read (a locked Chromium keyring can even prompt).
                let _ = browsers::forget(site_dir);
                if browser == "firefox" {
                    eprintln!("dl: {matched} {} cookie(s) imported but none could be read — the profile DB may be unreadable; re-import", site.label);
                } else {
                    eprintln!("dl: {matched} {} cookie(s) imported but none decrypted — Chromium needs the desktop keyring unlocked; a Firefox store is more reliable. Import discarded.", site.label);
                }
                false
            }
            None => {
                println!("(couldn't verify readability — is yt-dlp installed? the {matched} imported cookie(s) will be tried on dl runs)");
                wal_note();
                true
            }
        }
    }

    /// Prompt for one of `stores` to import `site_label`'s cookies from (auto-selecting a lone
    /// candidate).
    fn _pick<'a>(stores: &'a [browsers::CookieStore], site_label: &str) -> Option<&'a browsers::CookieStore> {
        if let [only] = stores {
            println!("one cookie store found — importing {site_label} cookies from {}", only.label);
            return Some(only);
        }
        println!("{}", _header(&format!("Import {site_label} cookies from:")));
        for (i, store) in stores.iter().enumerate() {
            println!("  {}) {}", i + 1, store.label);
        }
        print!("choice [1-{}]: ", stores.len());
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok()?;
        match line.trim().parse::<usize>().ok().filter(|n| (1..=stores.len()).contains(n)) {
            Some(n) => Some(&stores[n - 1]),
            None => {
                eprintln!("dl: not a listed number");
                None
            }
        }
    }

    /// Download a video with the bundled yt-dlp. A YouTube URL (`youtube.com`, `youtu.be`, …)
    /// takes the full path — per-video subtitle selection, the playlist unplayable-report,
    /// channel tabs sorted into folders; any other site yt-dlp supports takes a generic path —
    /// one video downloaded flat into `--into`, the same quality knobs and failure ledger, minus
    /// the folder trees (a generic page gives no channel/playlist structure to build them from).
    /// `-c` lists what "any other site" tends to cover. The machinery lives in
    /// [`crate::drivers::youtube`]; this stays the thin argument shell.
    #[name("dl")]
    pub fn dl(args: DlArgs) {
        let DlArgs { url, into, single, cookies, no_cookies, audio, res, allow_ipv6, thumbnail, subtitles, taglist, cookie_import, compatibility_help, extra } = args;
        if compatibility_help {
            print!("{}", _compatibility_help());
            return;
        }
        if taglist {
            std::process::exit(youtube::taglist());
        }
        if let Some(target) = &cookie_import {
            let imported = _import_cookies(target);
            // `--cookie-import <target>` (no URL) is a setup step: import and stop. With a URL,
            // fall through and download, now able to use what was just imported.
            if url.is_none() {
                std::process::exit(i32::from(!imported));
            }
        }
        let url = url.unwrap_or_default(); // clap guarantees presence unless an action flag was given
        if let Err(err) = std::fs::create_dir_all(&into) {
            eprintln!("dl: cannot create {}: {err}", into.display());
            std::process::exit(1);
        }
        let ffmpeg = youtube::bundled_ffmpeg_dir();
        let deno = youtube::bundled_deno();
        // Auto-select the per-site store matching this URL's host — a prior `--cookie-import` for
        // this site is the standing default; an explicit `--cookies` file wins, and `--no-cookies`
        // opts out of stored cookies entirely (download anonymously).
        let site = browsers::resolve_target(&url);
        let site_dir = _cookie_store_dir().join(&site.key);
        let imported = (!no_cookies && cookies.is_none())
            .then(|| browsers::imported_spec(&site_dir))
            .flatten();
        if imported.is_some() {
            _warn_if_cookies_expired(&site_dir, &site);
        }
        let env = youtube::Env {
            ffmpeg_dir: ffmpeg.as_deref(),
            cookies: cookies.as_deref(),
            cookies_from_browser: imported.as_deref(),
            audio,
            res,
            allow_ipv6,
            thumbnail,
            subtitles,
            extra: &extra,
            js_runtime: deno.as_deref(),
        };
        let code = if _is_youtube(&url) {
            _youtube(&url, &into, env, single)
        } else {
            _video(&url, &into, env)
        };
        if code != 0 {
            std::process::exit(code);
        }
    }

    /// The YouTube path: classify the URL and hand off to the matching driver entry — a lone
    /// video, a playlist (with its unplayable report), or a whole channel (tabs → folders).
    fn _youtube(url: &str, into: &Path, env: youtube::Env, single: bool) -> i32 {
        match youtube::classify(url, single) {
            youtube::Link::Video => youtube::download_video(url, into, env),
            youtube::Link::Playlist { id } => youtube::download_playlist(url, &id, into, env),
            youtube::Link::Channel { root } => youtube::download_channel(&root, into, env),
        }
    }

    /// The generic path for every other site: one flat download into `into` — we can't tell a
    /// playlist from a channel from a lone page, so there's no folder tree — reusing the same
    /// quality knobs, archive, and failure ledger as the YouTube path.
    fn _video(url: &str, into: &Path, env: youtube::Env) -> i32 {
        youtube::download_generic(url, into, env)
    }

    /// Whether `url`'s host is YouTube — any subdomain of `youtube.com` / `youtube-nocookie.com`,
    /// or `youtu.be`. The gate that routes [`dl`] to the YouTube path rather than the generic one
    /// (a separate concern from cookie targeting, though both key off the host). Host extraction
    /// is [`browsers::host_of`], which already strips a leading `www.`.
    fn _is_youtube(url: &str) -> bool {
        let host = browsers::host_of(url);
        host == "youtu.be"
            || host == "youtube.com"
            || host.ends_with(".youtube.com")
            || host == "youtube-nocookie.com"
            || host.ends_with(".youtube-nocookie.com")
    }

    #[derive(Args)]
    pub struct DlArgs {
        /// A video URL — YouTube (video, playlist, or channel) or any other site yt-dlp supports (`-c` lists common ones)
        #[arg(required_unless_present_any = ["taglist", "cookie_import", "compatibility_help"])]
        pub url: Option<String>,
        /// Destination root — YouTube playlists/channels build folder trees here; other sites download into it directly
        #[arg(long, default_value = ".")]
        pub into: PathBuf,
        /// A YouTube video link that also names a playlist (`watch?v=…&list=…`) downloads the whole
        /// playlist by default; this takes just the video
        #[arg(long)]
        pub single: bool,
        /// A cookies file (Netscape format, as browser extensions export) — only needed for
        /// age-restricted content or networks that bot-wall; home connections rarely do
        #[arg(long)]
        pub cookies: Option<PathBuf>,
        /// Ignore cookies stored by a prior `--cookie-import` for this site and download
        /// anonymously — useful when a site bot-walls a signed-in session (e.g. TikTok) and
        /// sending your real cookies risks getting that session flagged
        #[arg(long, conflicts_with_all = ["cookies", "cookie_import"])]
        pub no_cookies: bool,
        /// Audio only: extract the best audio track (kept as-is, no re-encode); on the YouTube
        /// path subtitles arrive as metadata tags (`subtitles_en`, `subtitles_he_autogenerated`, …)
        #[arg(long)]
        pub audio: bool,
        /// Cap the video height (e.g. 1080) — takes the best formats at or under it
        #[arg(long, value_name = "HEIGHT")]
        pub res: Option<u32>,
        /// Let yt-dlp use IPv6. By default `dl` forces IPv4 — on a broken or slow IPv6 route,
        /// every request otherwise stalls ~5s on the fallback. Pass this on an IPv6-only network
        #[arg(long)]
        pub allow_ipv6: bool,
        /// Embed a cover-art thumbnail into each video (off by default — it costs extra requests).
        /// Handled as a late, idempotent pass: it skips videos that already have one and, ignoring
        /// the download archive, will patch previously-downloaded videos on a re-run
        #[arg(long)]
        pub thumbnail: bool,
        /// Force a subtitle scan of the target video(s) and embed any expected tracks that are
        /// missing. Subtitles already ride a normal download; this late, idempotent pass (ignoring
        /// the download archive) is for patching previously-downloaded videos on a re-run
        #[arg(long)]
        pub subtitles: bool,
        /// Import a browser's cookies for one site, so later dl runs on it authenticate. TARGET
        /// is a site keyword (youtube, tiktok, facebook, instagram, twitter, reddit, vimeo,
        /// twitch, niconico, bilibili, patreon, nebula, bbc), a domain (example.com), or a URL.
        /// Only that site's cookies are copied —
        /// never your whole cookie DB. Scans installed browsers (native/Flatpak/Snap/Nix) and
        /// copies from your pick, so a running browser can't lock it. Runs standalone, or before
        /// a download when a URL is also given
        #[arg(long, value_name = "TARGET", conflicts_with = "cookies")]
        pub cookie_import: Option<String>,
        /// Print the notable yt-dlp flags (usable after `--`), then yt-dlp's full option list
        #[arg(short = 't', long)]
        pub taglist: bool,
        /// Print the kinds of sites yt-dlp tends to support (like --help, but for site coverage)
        #[arg(short = 'c', long)]
        pub compatibility_help: bool,
        /// Anything after `--` is handed to yt-dlp verbatim, after our defaults — a repeated
        /// flag resolves in its favor; `-t` lists the ones worth knowing
        #[arg(last = true)]
        pub extra: Vec<String>,
    }

    /// The `dl -c` site-coverage listing — a Markdown doc embedded at compile time (long enough
    /// to earn its own file, and natural to author/read as Markdown). Rendered for the terminal
    /// by [`doc_render::render_doc`]; edit the `.md` to revise it.
    const COMPATIBILITY: &str = include_str!("templates/yt-dlp_compatibility.md");

    /// What `dl -c` prints: [`COMPATIBILITY`] rendered to ANSI-coloured text by
    /// [`doc_render::render_doc`] — markers stripped, headings/emphasis/code/lists/quotes coloured
    /// via the shared theme, pre-drawn tables and body passed through untouched.
    fn _compatibility_help() -> String {
        doc_render::render_doc(COMPATIBILITY)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn compatibility_help_renders_the_markdown_doc_with_colour_and_intact_lines() {
            let out = _compatibility_help();
            // The doc is rendered markdown → coloured (ANSI escapes present).
            assert!(out.contains('\x1b'), "expected colour from the markdown render");
            // Line-based render: never reflows, so it has exactly one output line per source
            // line — pre-drawn tables and indented blocks survive intact.
            assert_eq!(out.lines().count(), COMPATIBILITY.lines().count(), "line count must be preserved");
            // A representative box-drawing table row (if the doc has one) passes through verbatim,
            // borders and spacing untouched.
            if let Some(row) = COMPATIBILITY.lines().find(|l| l.contains('│')) {
                assert!(out.contains(row), "table row reflowed/mangled: {row:?}");
            }
        }

        #[test]
        fn youtube_hosts_route_to_the_youtube_path_others_do_not() {
            for yt in [
                "https://www.youtube.com/watch?v=x",
                "https://youtu.be/x",
                "https://music.youtube.com/watch?v=x",
                "https://m.youtube.com/watch?v=x",
                "http://youtube.com/playlist?list=y",
                "https://www.youtube-nocookie.com/embed/x",
            ] {
                assert!(_is_youtube(yt), "should be YouTube: {yt}");
            }
            for other in [
                "https://vimeo.com/12345",
                "https://twitter.com/u/status/1",
                "https://notyoutube.com/x",       // must not match by suffix-substring
                "https://youtube.com.evil.test/x", // host is evil.test, not youtube
                "https://example.com/youtube.com", // youtube.com only in the path
            ] {
                assert!(!_is_youtube(other), "should be generic: {other}");
            }
        }

        #[test]
        fn extensions_normalize_to_bare_lowercase() {
            assert_eq!(_normalize_ext(".PNG"), "png");
            assert_eq!(_normalize_ext("mp3"), "mp3");
        }

        #[test]
        fn one_pass_groups_links_by_type_case_insensitively_and_once() {
            let html = r#"<a href="a.png"><img src="B.PNG"><a href="a.png">
                          <a href="song.mp3">a sentence ending in .png"quoted prose .mp3""#;
            let groups =
                _links_by_type(html, &["png".to_string(), "mp3".to_string()]);
            assert_eq!(groups[0], ["a.png", "B.PNG"], "case-insensitive, duplicates dropped");
            assert_eq!(groups[1], ["song.mp3"]);
        }

        #[test]
        fn whitespace_bearing_tokens_are_prose_not_links() {
            let groups = _links_by_type(r#"text "not a link .png" "real.png""#, &["png".to_string()]);
            assert_eq!(groups[0], ["real.png"]);
        }

        #[test]
        fn links_resolve_the_way_a_browser_would() {
            let page = "https://ex.com/gallery/2024/index.html";
            assert_eq!(_resolve_link(page, "http://other.org/x.png"), "http://other.org/x.png");
            assert_eq!(_resolve_link(page, "//cdn.ex.com/x.png"), "https://cdn.ex.com/x.png");
            assert_eq!(_resolve_link(page, "/root.png"), "https://ex.com/root.png");
            assert_eq!(_resolve_link(page, "img/x.png"), "https://ex.com/gallery/2024/img/x.png");
            assert_eq!(_resolve_link(page, "../up.png"), "https://ex.com/gallery/up.png");
            assert_eq!(
                _resolve_link(page, "../../../../too-far.png"),
                "https://ex.com/too-far.png",
                "`..` must never climb above the host"
            );
        }

        #[test]
        fn scheme_less_pages_resolve_as_https_and_root_pages_use_the_host_as_dir() {
            assert_eq!(_resolve_link("ex.com/dir/p.html", "x.png"), "https://ex.com/dir/x.png");
            assert_eq!(_resolve_link("https://ex.com", "x.png"), "https://ex.com/x.png");
        }

        #[test]
        fn file_urls_keep_their_absolute_path() {
            // The e2e tests (and quick local experiments) scan file:// pages; the resolved
            // links must keep the leading slash that `file://` needs.
            assert_eq!(
                _resolve_link("file:///tmp/site/page.html", "sub/a.txt"),
                "file:///tmp/site/sub/a.txt"
            );
        }

    }
}
