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
    use crate::support::doc_style::_header;
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

    /// `~/.bashrs/user-data/browser_cookies` — where `--cookie-import` copies the chosen store
    /// and where every `dl` run looks for it.
    fn _cookie_store_dir() -> PathBuf {
        crate::conf::user_data_dir().join("browser_cookies")
    }

    /// The `--cookie-import` action: scan for browser cookie stores, let the user pick one, and
    /// copy it into bashrs's own data dir (which a running browser can't lock). Later runs use
    /// it automatically. Returns whether a store was imported.
    fn _import_cookies() -> bool {
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
        let Some(store) = _pick(&stores) else { return false };
        let dir = _cookie_store_dir();
        if let Err(err) = browsers::import(store, &dir) {
            eprintln!("dl: could not import the cookie store: {err}");
            return false;
        }
        println!("imported cookies from {}", store.label);
        _report_cookie_check(store, &dir)
    }

    /// Read the freshly imported store back through yt-dlp and tell the user what it actually
    /// yielded — turning "imported (hopefully)" into a concrete result, and catching a
    /// keyring-locked Chromium store or a not-signed-in profile *now* instead of silently on
    /// every later run. Returns whether a usable store remains imported. When the check can't run
    /// (yt-dlp not bundled yet), falls back to the speculative guidance rather than overclaiming.
    fn _report_cookie_check(store: &browsers::CookieStore, dir: &std::path::Path) -> bool {
        let Some(spec) = browsers::imported_spec(dir) else { return true };
        // The WAL caveat holds whenever cookies did come through: a copy captures only what the
        // browser already flushed to the DB, so a sign-in from moments ago may not be in it yet.
        let wal_note = || println!("note: if a fresh sign-in isn't recognized, fully quit the browser and re-import");
        match youtube::count_browser_cookies(&spec, dir) {
            Some(check) if check.youtube > 0 => {
                println!(
                    "validated — {} YouTube/Google cookies readable ({} total); future dl runs will use them",
                    check.youtube, check.total
                );
                wal_note();
                true
            }
            Some(check) if check.total > 0 => {
                // The store decrypts fine, it just has nothing for YouTube — a profile that was
                // never signed in. Keep it (it's valid), but say why it won't help yet.
                eprintln!(
                    "dl: read {} cookies from {}, but none for YouTube/Google — sign in to YouTube in that browser/profile, then re-import",
                    check.total, store.label
                );
                wal_note();
                true
            }
            Some(_) => {
                // Nothing decrypted at all. Rolling the import back keeps every future run from
                // re-attempting the same doomed read (a locked Chromium keyring can even prompt).
                let _ = browsers::forget(dir);
                if store.browser == "firefox" {
                    eprintln!("dl: no cookies could be read from {} — sign in to YouTube there first, then re-import", store.label);
                } else {
                    eprintln!("dl: no cookies could be decrypted from {} — Chromium cookies need the desktop keyring unlocked; a Firefox store is the most reliable. Import discarded.", store.label);
                }
                false
            }
            None => {
                // Couldn't run the read-back (yt-dlp not installed yet). The copy is in place;
                // fall back to the pre-validation guidance instead of claiming a count.
                println!("future dl runs will use them");
                wal_note();
                if store.browser != "firefox" {
                    println!("note: Chromium-family cookies are keyring-encrypted; if reads fail, a Firefox store is the most reliable");
                }
                true
            }
        }
    }

    /// Prompt for one of `stores` (auto-selecting a lone candidate).
    fn _pick(stores: &[browsers::CookieStore]) -> Option<&browsers::CookieStore> {
        if let [only] = stores {
            println!("one cookie store found — importing {}", only.label);
            return Some(only);
        }
        println!("{}", _header("Import YouTube cookies from:"));
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
        let DlArgs { url, into, single, cookies, audio, res, taglist, cookie_import, compatibility_help, extra } = args;
        if compatibility_help {
            print!("{}", _compatibility_help());
            return;
        }
        if taglist {
            std::process::exit(youtube::taglist());
        }
        if cookie_import {
            let imported = _import_cookies();
            // A bare `--cookie-import` (no URL) is a setup step: import and stop. With a URL, fall
            // through and download, now using what was just imported.
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
        // Imported browser cookies are the standing default; an explicit `--cookies` file wins.
        let imported = (cookies.is_none()).then(|| browsers::imported_spec(&_cookie_store_dir())).flatten();
        let env = youtube::Env {
            ffmpeg_dir: ffmpeg.as_deref(),
            cookies: cookies.as_deref(),
            cookies_from_browser: imported.as_deref(),
            audio,
            res,
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
    /// or `youtu.be`. The gate that routes [`dl`] to the YouTube path rather than the generic one.
    fn _is_youtube(url: &str) -> bool {
        let host = _url_host(url);
        let host = host.strip_prefix("www.").unwrap_or(&host);
        host == "youtu.be"
            || host == "youtube.com"
            || host.ends_with(".youtube.com")
            || host == "youtube-nocookie.com"
            || host.ends_with(".youtube-nocookie.com")
    }

    /// The lowercased host of a URL — scheme, userinfo, and port stripped. `""` for input with no
    /// host (a bare path), which never matches [`_is_youtube`] and so takes the generic path.
    fn _url_host(url: &str) -> String {
        let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
        let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
        let host = authority.rsplit('@').next().unwrap_or(authority); // drop any user:pass@
        host.split(':').next().unwrap_or(host).to_lowercase() // drop any :port
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
        /// Audio only: extract the best audio track (kept as-is, no re-encode); on the YouTube
        /// path subtitles arrive as metadata tags (`subtitles_en`, `subtitles_he_autogenerated`, …)
        #[arg(long)]
        pub audio: bool,
        /// Cap the video height (e.g. 1080) — takes the best formats at or under it
        #[arg(long, value_name = "HEIGHT")]
        pub res: Option<u32>,
        /// Scan for browser cookie stores (native, Flatpak, Snap, Nix) and import one into
        /// bashrs — copied, so a running browser can't lock it; later runs reuse it. For
        /// age-restricted or walled content. Runs standalone, or before a download when a URL
        /// is also given
        #[arg(long, conflicts_with = "cookies")]
        pub cookie_import: bool,
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

    /// The lead-in and closing lines of `dl -c` (plain text, framing the sections).
    const COMPATIBILITY_INTRO: &str =
        "yt-dlp backs `dl` and supports well over a thousand sites. What to expect:";
    const COMPATIBILITY_OUTRO: &str = "For anything gated (logins, paid, region- or age-locked), \
        --cookie-import or\n--cookies is usually what unlocks it.";

    /// The body of `dl -c`, as `(heading, detail)` sections. Kept as data so [`_compatibility_help`]
    /// can render each heading in the shared bold-blue header style (the one behind `becho` and
    /// `lll`'s column row) without ANSI escapes cluttering the copy. Details are flush-left in the
    /// source (no `\`-continuation, which would strip the first line's indent) so their leading
    /// spaces are exactly what prints.
    const COMPATIBILITY_SECTIONS: &[(&str, &str)] = &[
        ("Best-maintained / flagship",
"  • YouTube — the primary target, most robust (videos, playlists, channels,
    live, chapters, subtitles, SponsorBlock integration).
  • Vimeo, Dailymotion, SoundCloud, Bandcamp, Twitch — long-standing, reliable."),
        ("Social media",
"  Twitter/X, Instagram, TikTok, Facebook, Reddit, Tumblr, Bluesky, Snapchat,
  Pinterest. These break more often (frequent site changes) and increasingly
  need cookies for anything gated."),
        ("Broadcasters / catch-up TV (a large share of the extractor count)",
"  BBC iPlayer, ITV, Channel 4; ARD/ZDF and other German public broadcasters;
  France.tv/Arte; RAI; NHK; PBS; CBC; ABC (AU); Al Jazeera, etc."),
        ("Audio / music / podcasts",
"  SoundCloud, Bandcamp, Mixcloud, generic podcast RSS, many radio catch-up
  sites. (Note: not Spotify / Apple Music — DRM.)"),
        ("Learning platforms",
"  Coursera, Udemy, Khan Academy, LinkedIn Learning — the paid ones require
  --username/--password or --cookies."),
        ("Adult sites",
"  Many are supported (Pornhub, xHamster, etc.)."),
        ("Generic extractor",
"  For sites without a dedicated module, yt-dlp scrapes the page for <video>
  tags and HLS (.m3u8) / DASH (.mpd) manifests and reconstructs the stream —
  which is why it \"just works\" on lots of random embed pages."),
    ];

    /// What `dl -c` prints: a plain-language tour of yt-dlp's site coverage, headings styled
    /// bold-blue via [`_header`] (the same style `becho` and `lll`'s header use). Assembled into
    /// one buffer and returned, so the caller emits it in a single write rather than line by line.
    fn _compatibility_help() -> String {
        let mut out = format!("{COMPATIBILITY_INTRO}\n");
        for (heading, detail) in COMPATIBILITY_SECTIONS {
            out.push_str(&format!("\n{}\n{detail}\n", _header(heading)));
        }
        out.push_str(&format!("\n{COMPATIBILITY_OUTRO}\n"));
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn compatibility_help_styles_headings_via_the_shared_header_and_buffers_it_all() {
            let out = _compatibility_help();
            // Every section heading is rendered through the shared bold-blue `_header` (the style
            // behind `becho` / `lll`'s header) — tie the assertion to that function so a style
            // change can't silently un-blue these.
            for (heading, detail) in COMPATIBILITY_SECTIONS {
                assert!(out.contains(&_header(heading)), "heading not header-styled: {heading}");
                assert!(out.contains(detail), "detail missing/mangled under: {heading}");
            }
            // Intro and outro frame it as plain text (not styled).
            assert!(out.starts_with(COMPATIBILITY_INTRO));
            assert!(out.contains(COMPATIBILITY_OUTRO));
            // A detail's leading indent survives (the `\`-continuation bug would eat it).
            assert!(out.contains("\n  • YouTube "), "first bullet lost its indent");
            // One buffer, assembled once — the whole thing is a single owned String.
            assert!(out.lines().count() > COMPATIBILITY_SECTIONS.len());
        }

        #[test]
        fn url_host_strips_scheme_userinfo_port_and_path() {
            assert_eq!(_url_host("https://www.youtube.com/watch?v=x"), "www.youtube.com");
            assert_eq!(_url_host("http://user:pass@Host.EXAMPLE.com:8080/a/b"), "host.example.com");
            assert_eq!(_url_host("youtu.be/abc"), "youtu.be"); // scheme-less
            assert_eq!(_url_host("/local/path.mp4"), ""); // no host → generic path
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
