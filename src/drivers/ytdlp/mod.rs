//! Driving the bundled `yt-dlp` — the machinery behind `dl`
//! ([`crate::categories::download`] stays a thin argument shell over this, the same way
//! [`super::python`] backs the `py_*` commands). This module is the *conductor*: classify what a
//! URL points at ([`Link`]/[`classify`]) and run the download of it, one video at a time. Each
//! concern the conductor drives is its own child:
//!
//! - [`invocation`] — argv assembly and process launch, for every yt-dlp run.
//! - [`subtitles`] — each video's exact subtitle list, from YouTube's caption matrix.
//! - [`scan`] — playlist/channel-tab enumeration and the unplayable-report.
//! - [`postprocess`] — thumbnail / subtitle-track / audio-tag patching of downloaded files.
//! - [`failures`] — failure diagnosis, advice lines, and the failed-downloads ledger.
//! - [`cookie_store`] — the cookie-DB filtering behind `--cookie-import`.
//!
//! Named for the tool, not the site: the subtitle matrix and channel tabs are YouTube-shaped, but
//! [`download_generic`] serves any other site yt-dlp supports (`dl` routes there when the host
//! isn't YouTube), reusing everything site-agnostic — the argv base, the failure diagnosis + geo
//! rescue, the ledger, and the download archive.
//!
//! Downloads are driven **one video at a time** (playlists and channel tabs are scanned first,
//! then each entry gets its own yt-dlp invocation). That costs one extra metadata request per
//! video, and buys the thing a single batch invocation cannot express: per-video subtitle
//! selection (see [`subtitles`]).

mod cookie_store;
mod failures;
mod invocation;
mod postprocess;
mod scan;
mod subtitles;

// The surface `dl` (categories::download) drives, re-exported so the children stay private.
pub(crate) use cookie_store::{cookies_expired, filter_cookie_db, readable_cookie_count};
pub(crate) use invocation::{bundled_deno, bundled_ffmpeg_dir, Env};

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::support::doc_style;
use crate::support::exec::run_reporting_code;
use crate::support::theme_code;
use failures::{GeoRescue, diagnose_failure, scrub_ledger, write_ledger};
use invocation::{ARCHIVE_NAME, CHANNEL_TABS, channel_tab_argv, generic_argv, playlist_argv, run, video_argv, ytdlp_invocation};
use postprocess::{embed_subtitles, embed_thumbnails, find_by_id, finish_media, patch_collection_subtitles};
use scan::{ScanEntry, TabScan, archived_ids, is_tombstone, scan_playlist, scan_tab, unplayable_report};
use subtitles::{Pick, Planned, batch_probe, default_picks, group_by_langs, video_picks};

/// What a YouTube URL points at — each kind downloads differently.
#[derive(Debug, PartialEq)]
pub(crate) enum Link {
    Video,
    Playlist { id: String },
    Channel { root: String },
}

/// Classify `url`. Channel forms (`/@handle`, `/channel/…`, `/c/…`, `/user/…`) normalize to
/// the channel root — any tab suffix is dropped, since tabs are enumerated at download time.
/// A `list=` parameter (or an explicit `/playlist`) means playlist, unless `single` opts
/// out. Everything else — non-YouTube URLs included, yt-dlp knows hundreds of sites — is
/// treated as a lone video.
pub(crate) fn classify(url: &str, single: bool) -> Link {
    for marker in ["/@", "/channel/", "/c/", "/user/"] {
        if let Some(start) = url.find(marker) {
            let name = start + marker.len();
            let end = url[name..].find(['/', '?', '#']).map(|i| name + i).unwrap_or(url.len());
            return Link::Channel { root: url[..end].to_string() };
        }
    }
    if !single {
        if let Some((_, rest)) = url.split_once("list=") {
            let id = rest.split(['&', '#']).next().unwrap_or_default();
            if !id.is_empty() {
                return Link::Playlist { id: id.to_string() };
            }
        }
    }
    Link::Video
}

/// The 11-char video id in a lone YouTube URL — `watch?v=ID`, `youtu.be/ID`, `/shorts/ID`,
/// `/embed/ID`. `None` for a shape we don't recognize (the caller falls back to a probe). Lets
/// [`download_video`] tell "already on disk" without a yt-dlp call, so it can skip the subtitle
/// probe + download for a video it already has.
fn id_from_url(url: &str) -> Option<String> {
    let rest = ["v=", "youtu.be/", "/shorts/", "/embed/", "/live/"]
        .iter()
        .find_map(|marker| url.split_once(marker).map(|(_, rest)| rest))?;
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    (id.len() == 11).then_some(id)
}

/// A lone video: probe its subtitle situation, then download with the exact list.
pub(crate) fn download_video(url: &str, into: &Path, env: Env) -> i32 {
    // The subtitle probe and the download are both for a video we don't have yet. If its file is
    // already on disk (id parsed straight from the URL — no yt-dlp call), skip both: subtitle
    // handling rides the download, so there's nothing to probe for. A URL we can't pull an id from
    // falls back to the probe. The opt-in `--thumbnail` pass below still runs, patching a re-run.
    let url_id = id_from_url(url);
    let on_disk = url_id.as_deref().is_some_and(|id| find_by_id(into, id).is_some());
    let mut code = 0;
    // `picks` feed the opt-in `--subtitles` pass. A fresh download always probes (it needs the
    // list to embed inline); an already-downloaded video probes only when `--subtitles` forces a
    // scan; a plain re-run on an existing video probes nothing.
    let mut picks: Vec<Pick> = Vec::new();
    let id = if !on_disk {
        // Announced because the probe runs silently for a few seconds (its output is captured).
        println!("probing subtitles…");
        let (probe_id, probed) = video_picks(url, env);
        picks = probed;
        let keys: Vec<String> = picks.iter().map(|pick| pick.key.clone()).collect();
        code = run(video_argv(url, into, env, &keys));
        if code != 0 {
            // The bracketed id is the ledger's stable key ([`scrub_ledger`]); a URL label still
            // carries the id for YouTube links via its `v=` parameter.
            let label =
                probe_id.as_ref().map(|id| format!("[{id}]")).unwrap_or_else(|| url.to_string());
            match diagnose_failure(video_argv(url, into, env, &keys), &label, GeoRescue::IpEnforced)
            {
                None => code = 0, // the plain retry came through
                Some(line) => write_ledger(into, &[line]),
            }
        }
        if code == 0 {
            if let Some(id) = &probe_id {
                finish_media(into, id, &picks, env);
            }
            // The explicit happy ending — yt-dlp's own output stops at its last postprocessor
            // ("[Metadata] …"), which reads unfinished.
            let done = probe_id.as_deref().or(url_id.as_deref()).unwrap_or(url);
            println!("{}", doc_style::approved(&format!("{done}: downloaded")));
        }
        probe_id.or(url_id)
    } else if env.subtitles {
        println!("{}: already downloaded — scanning subtitles", url_id.as_deref().unwrap_or(url));
        let (probe_id, probed) = video_picks(url, env);
        picks = probed;
        probe_id.or(url_id)
    } else {
        println!("{}: already downloaded — skipping", url_id.as_deref().unwrap_or(url));
        url_id
    };
    if code == 0 {
        if let Some(id) = &id {
            if env.thumbnail {
                embed_thumbnails(into, std::slice::from_ref(id), env);
            }
            if env.subtitles {
                println!("subtitles: scanning 1 video…");
                embed_subtitles(into, id, url, &picks, env);
            }
        }
    }
    scrub_ledger(into);
    code
}

/// The sites the late `--thumbnail`/`--subtitles` patch passes support — YouTube only, today.
/// Kept as a list so the generic path's notice names exactly what IS covered as it grows.
const PATCHABLE_PLATFORMS: &[&str] = &["youtube"];

/// The generic single-video path for non-YouTube sites (`dl` routes here when the host isn't
/// YouTube). One download, flat into `into` — a generic page gives no playlist/channel structure
/// to build folders from — reusing this module's shared argv base ([`common`]), failure
/// diagnosis + rescue, ledger, and download archive. No subtitle probing: that's a
/// YouTube-caption-matrix affair, so a generic site just gets the media, metadata, thumbnail, and
/// chapters. The URL is the ledger label — the only stable key without a metadata probe (and
/// enough for [`scrub_ledger`] when it embeds the id).
pub(crate) fn download_generic(url: &str, into: &Path, env: Env) -> i32 {
    // The patch passes need a per-video id in the filename and a known thumbnail/caption source —
    // YouTube-machinery, today. Name what IS supported (the list will grow) instead of silently
    // ignoring the flag.
    if env.thumbnail || env.subtitles {
        let flags = match (env.thumbnail, env.subtitles) {
            (true, true) => "--thumbnail/--subtitles",
            (true, false) => "--thumbnail",
            _ => "--subtitles",
        };
        eprintln!(
            "dl: {flags} supported platforms: {} — skipped for this site",
            PATCHABLE_PLATFORMS.join(", ")
        );
    }
    let mut code = run(generic_argv(url, into, env));
    if code != 0 {
        match diagnose_failure(generic_argv(url, into, env), url, GeoRescue::XffSweep) {
            None => code = 0, // the plain retry or a spoofed region came through
            Some(line) => write_ledger(into, &[line]),
        }
    }
    scrub_ledger(into);
    code
}

/// Playlist mode. The flat scan comes first — it still *names* entries nothing can play
/// anymore, which downloads would only surface as opaque errors — the traceability report and
/// the download archive live inside the playlist's own folder, and every playable, not-yet-
/// archived entry is downloaded individually with its own subtitle list.
pub(crate) fn download_playlist(url: &str, id: &str, into: &Path, env: Env) -> i32 {
    let Some(scan) = scan_playlist(url, "%(title)S[%(id)S]", env) else {
        eprintln!(
            "dl: could not scan the playlist — downloading in one pass, EN-only subtitles, \
             no unplayable report"
        );
        let keys: Vec<String> = default_picks().iter().map(|pick| pick.key.clone()).collect();
        return run(playlist_argv(url, into, into, env, &keys, None));
    };
    // The playlist's folder, spelled exactly as yt-dlp's template expansion will spell it.
    let dir = scan.dirname.clone().unwrap_or_else(|| format!("[{id}]"));
    let home = into.join(&dir);
    if let Err(err) = std::fs::create_dir_all(&home) {
        eprintln!("dl: cannot create {}: {err}", home.display());
        return 1;
    }

    let dead: Vec<&ScanEntry> =
        scan.entries.iter().filter(|entry| is_tombstone(&entry.title)).collect();
    if dead.is_empty() {
        println!("all {} playlist entries are playable", scan.entries.len());
    } else {
        let report = home.join(format!("unplayable__{id}.txt"));
        match std::fs::write(&report, unplayable_report(&scan.title, id, &dead)) {
            Ok(()) => println!(
                "{} of {} entries are unplayable — traces written to {}",
                dead.len(),
                scan.entries.len(),
                report.display()
            ),
            Err(err) => eprintln!("dl: could not write {}: {err}", report.display()),
        }
    }

    let archived = archived_ids(&home.join(ARCHIVE_NAME));
    let pending: Vec<&ScanEntry> = scan
        .entries
        .iter()
        .filter(|entry| !is_tombstone(&entry.title) && !archived.contains(&entry.id))
        .collect();
    let skipped = scan.entries.len() - pending.len();
    if skipped > 0 {
        println!("{skipped} entries already archived (or unplayable) — skipped");
    }
    let code = if pending.is_empty() {
        0
    } else {
        download_pending(url, &pending, into, &home, env, |url, into, home, env, langs, items| {
            playlist_argv(url, into, home, env, langs, Some(items))
        })
    };
    // Late thumbnail pass (opt-in) over every playable entry — including archived ones, so a
    // re-run with `--thumbnail` patches previously-downloaded videos.
    if env.thumbnail {
        let ids: Vec<String> = scan
            .entries
            .iter()
            .filter(|entry| !is_tombstone(&entry.title))
            .map(|entry| entry.id.clone())
            .collect();
        embed_thumbnails(&home, &ids, env);
    }
    if env.subtitles {
        let entries: Vec<&ScanEntry> =
            scan.entries.iter().filter(|entry| !is_tombstone(&entry.title)).collect();
        patch_collection_subtitles(url, &entries, &home, env);
    }
    code
}

/// The shared batched tail of playlist and channel-tab downloads: ONE probe invocation covers
/// every pending entry's subtitles, entries are grouped by their computed list, and each group
/// downloads in one yt-dlp invocation — process startup and player work are the expensive
/// parts, so the invocation count is what matters. Probe-failed entries fall back to the EN
/// default rather than being dropped.
fn download_pending(
    url: &str,
    pending: &[&ScanEntry],
    into: &Path,
    home: &Path,
    env: Env,
    argv: impl Fn(&str, &Path, &Path, Env, &[String], &str) -> Vec<OsString>,
) -> i32 {
    println!("probing subtitles of {} entries…", pending.len());
    let indexes: Vec<String> = pending.iter().map(|entry| entry.index.clone()).collect();
    let probed = batch_probe(url, &indexes, env);
    let planned: Vec<Planned> = pending
        .iter()
        .map(|entry| {
            probed
                .iter()
                .find(|plan| plan.index == entry.index)
                .map(|plan| Planned {
                    index: plan.index.clone(),
                    id: plan.id.clone(),
                    picks: plan.picks.clone(),
                })
                .unwrap_or_else(|| Planned {
                    index: entry.index.clone(),
                    id: entry.id.clone(),
                    picks: default_picks(),
                })
        })
        .collect();
    let mut worst = 0;
    for group in group_by_langs(&planned) {
        let keys: Vec<String> =
            group[0].picks.iter().map(|pick| pick.key.clone()).collect();
        let items =
            group.iter().map(|plan| plan.index.as_str()).collect::<Vec<_>>().join(",");
        let subs = if keys.is_empty() { "none".to_string() } else { keys.join(",") };
        println!(
            "=== downloading {} entr{} (subs: {subs}) ===",
            group.len(),
            if group.len() > 1 { "ies" } else { "y" },
        );
        let code = run(argv(url, into, home, env, &keys, &items));
        if code != 0 && worst == 0 {
            worst = code;
        }
        for plan in &group {
            finish_media(home, &plan.id, &plan.picks, env);
        }
    }
    // Post-mortem: whatever is still unarchived failed inside a group, where `--ignore-errors`
    // kept the rest flowing but discarded the why. Each gets a captured re-run to diagnose,
    // geo-blocks get the region rescue, and the stubborn ones go to the ledger — unarchived on
    // purpose, so future runs keep retrying them (and the scrub below clears their ledger lines
    // the moment a retry lands).
    let survivors = archived_ids(&home.join(ARCHIVE_NAME));
    let mut ledger = Vec::new();
    for plan in planned.iter().filter(|plan| !survivors.contains(&plan.id)) {
        let title = pending
            .iter()
            .find(|entry| entry.index == plan.index)
            .map(|entry| entry.title.as_str())
            .unwrap_or(&plan.id);
        // The bracketed id is the ledger's stable key — index and title both drift as the
        // playlist changes, so scrub_ledger matches on the id alone.
        let label = format!("#{} {title} [{}]", plan.index, plan.id);
        println!("--- diagnosing {label} ---");
        let keys: Vec<String> = plan.picks.iter().map(|pick| pick.key.clone()).collect();
        match diagnose_failure(argv(url, into, home, env, &keys, &plan.index), &label, GeoRescue::IpEnforced) {
            // A plain-retry success downloaded the file just now, after the group's finish pass
            // already ran — so post-process it here, or it would keep yt-dlp's default subtitle
            // titles and (in audio mode) miss its metadata tags.
            None => finish_media(home, &plan.id, &plan.picks, env),
            Some(line) => ledger.push(line),
        }
    }
    scrub_ledger(home);
    write_ledger(home, &ledger);
    worst
}

/// Channel mode: every tab in turn. The archive lives inside the channel's own folder (shared
/// by its tabs); the video-bearing tabs download entry by entry like a playlist, and the
/// playlists tab recurses into [`download_playlist`] per playlist — each with its own folder,
/// archive, and unplayable report, nested under `<channel>/playlists/`. A failing tab (most
/// channels lack a few) doesn't stop the rest; the run counts as success if any came through.
pub(crate) fn download_channel(root: &str, into: &Path, env: Env) -> i32 {
    let mut home: Option<PathBuf> = None; // settled by the first readable tab's scan
    let mut succeeded = 0;
    for tab in CHANNEL_TABS {
        let tab_url = format!("{root}/{tab}");
        println!("=== {tab_url} ===");
        let scan = match scan_tab(&tab_url, "%(uploader)S[%(channel_id)S]", env) {
            TabScan::Found(scan) => scan,
            TabScan::Missing => {
                println!("the channel has no `{tab}` tab");
                continue;
            }
            TabScan::Failed => {
                eprintln!("dl: could not read the `{tab}` tab — moving on");
                continue;
            }
        };
        let home = home
            .get_or_insert_with(|| {
                // The channel's own folder holds the archive all tabs share; an unprobeable
                // name degrades to a shared root archive.
                let dir = match &scan.dirname {
                    Some(dir) => into.join(dir),
                    None => into.to_path_buf(),
                };
                let _ = std::fs::create_dir_all(&dir);
                dir
            })
            .clone();
        if *tab == "playlists" {
            // Entries here are playlists, not videos — recurse, nesting under the channel.
            let nest = home.join("playlists");
            for entry in &scan.entries {
                println!("--- playlist: {} ---", entry.title);
                let url = format!("https://www.youtube.com/playlist?list={}", entry.id);
                if download_playlist(&url, &entry.id, &nest, env) == 0 {
                    succeeded += 1;
                }
            }
            continue;
        }
        let archived = archived_ids(&home.join(ARCHIVE_NAME));
        let pending: Vec<&ScanEntry> =
            scan.entries.iter().filter(|entry| !archived.contains(&entry.id)).collect();
        let skipped = scan.entries.len() - pending.len();
        if skipped > 0 {
            println!("{skipped} entries already archived — skipped");
        }
        let tab_code = if pending.is_empty() {
            0
        } else {
            download_pending(&tab_url, &pending, into, &home, env, |url, into, home, env, langs, items| {
                channel_tab_argv(url, tab, into, home, env, langs, items)
            })
        };
        // Late thumbnail pass (opt-in) over the whole tab — archived entries included, so a re-run
        // with `--thumbnail` patches previously-downloaded videos.
        if env.thumbnail {
            let ids: Vec<String> = scan.entries.iter().map(|entry| entry.id.clone()).collect();
            embed_thumbnails(&home, &ids, env);
        }
        if env.subtitles {
            let entries: Vec<&ScanEntry> = scan.entries.iter().collect();
            patch_collection_subtitles(&tab_url, &entries, &home, env);
        }
        if tab_code == 0 {
            succeeded += 1;
        }
    }
    i32::from(succeeded == 0)
}

/// The `-t/--taglist` listing: the notable yt-dlp flags first, styled for a quick scan, then
/// yt-dlp's own full option list (the repetition is fine — this is the index, that's the book).
pub(crate) fn taglist() -> i32 {
    println!("{}", doc_style::_header("Notable yt-dlp flags — pass them after `--`:"));
    let width = NOTABLE_FLAGS.iter().map(|(flag, _)| flag.len()).max().unwrap_or(0);
    for (flag, blurb) in NOTABLE_FLAGS {
        let pad = " ".repeat(width - flag.len());
        println!("  {}{pad}  {blurb}", theme_code::argname(flag));
    }
    println!("\n{}", doc_style::_header("Everything yt-dlp accepts:"));
    let (program, args) = ytdlp_invocation(vec![OsString::from("--help")]);
    run_reporting_code(program, args)
}

/// The flags worth knowing about, shown by [`taglist`]: `(flag as you'd type it, what it does)`.
const NOTABLE_FLAGS: &[(&str, &str)] = &[
    ("--sponsorblock-remove sponsor", "cut sponsored segments out of the video"),
    ("--write-description", "save the description as a sidecar file"),
    ("--write-info-json", "save every scrap of metadata as JSON"),
    ("--playlist-items 3,5-7", "download only a slice of a playlist"),
    ("--limit-rate 2M", "cap the download speed"),
    ("--merge-output-format webm/mkv", "prefer webm (forfeits embedded cover art)"),
    ("--sub-langs all", "every real subtitle language, not just EN"),
    ("--proxy URL", "route through a proxy"),
];

/// Test fixtures shared by the sibling modules' suites — the scratch dir, the
/// ffmpeg availability gate, and the language-key shorthand.
#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    /// The EN-family keys, as tests pass them to the argv builders.
    pub(crate) fn en_keys() -> Vec<String> {
        ["en", "en-US", "en-GB"].map(str::to_string).to_vec()
    }

    /// A fresh scratch directory under the system temp dir.
    pub(crate) fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bashrs_yt_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Skip-with-notice: `true` when ffmpeg+ffprobe are runnable (bundled or PATH) — the same
    /// resolution the code under test uses via `Env::ffmpeg_dir`.
    pub(crate) fn ffmpeg_or_skip(test: &str) -> bool {
        let dir = bundled_ffmpeg_dir();
        let works = ["ffmpeg", "ffprobe"].iter().all(|name| {
            let bin = dir
                .as_ref()
                .map(|dir| dir.join(name).into_os_string())
                .unwrap_or_else(|| (*name).into());
            std::process::Command::new(bin)
                .arg("-version")
                .output()
                .is_ok_and(|out| out.status.success())
        });
        if !works {
            eprintln!("SKIPPED {test}: no usable ffmpeg/ffprobe available");
        }
        works
    }

    /// The resolved name of an ffmpeg-family binary, mirroring the code under test.
    pub(crate) fn ff_bin(name: &str) -> std::ffi::OsString {
        bundled_ffmpeg_dir()
            .map(|dir| dir.join(name).into_os_string())
            .unwrap_or_else(|| name.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_classify_by_what_they_point_at() {
        let video = |url: &str| assert_eq!(classify(url, false), Link::Video, "{url}");
        video("https://www.youtube.com/watch?v=MFT4OgFxfes");
        video("http://www.youtube.com/watch?v=MFT4OgFxfes");
        video("https://youtu.be/pv21e6iEZUw?si=sl5UGl0DI00f-0_h");
        video("https://youtu.be/IYnsfV5N2n8?si=6xSF90BnXIJcdy04&t=39"); // timestamped share
        video("https://vimeo.com/12345"); // not ours to refuse: yt-dlp knows other sites

        let playlist = classify(
            "https://www.youtube.com/watch?v=7jrKjkrX3Gw&list=PLvR1Vs9Qj4fk-VnGR2xUtNLvLcwBwGB6V",
            false,
        );
        assert_eq!(playlist, Link::Playlist { id: "PLvR1Vs9Qj4fk-VnGR2xUtNLvLcwBwGB6V".into() });
        assert_eq!(
            classify("https://www.youtube.com/playlist?list=PLxyz&feature=share", false),
            Link::Playlist { id: "PLxyz".into() }
        );
    }

    #[test]
    fn channel_urls_normalize_to_their_root_dropping_any_tab() {
        for url in [
            "https://www.youtube.com/@MontemayorChannel",
            "https://www.youtube.com/@MontemayorChannel/videos",
            "https://www.youtube.com/@MontemayorChannel/streams?view=0",
        ] {
            assert_eq!(
                classify(url, false),
                Link::Channel { root: "https://www.youtube.com/@MontemayorChannel".into() },
                "{url}"
            );
        }
        assert_eq!(
            classify("https://www.youtube.com/channel/UCabc123/playlists", false),
            Link::Channel { root: "https://www.youtube.com/channel/UCabc123".into() }
        );
        assert_eq!(
            classify("https://www.youtube.com/user/OldName", false),
            Link::Channel { root: "https://www.youtube.com/user/OldName".into() }
        );
    }

    #[test]
    fn id_from_url_pulls_the_11_char_video_id_or_gives_up() {
        assert_eq!(id_from_url("https://www.youtube.com/watch?v=MFT4OgFxfes").as_deref(), Some("MFT4OgFxfes"));
        assert_eq!(id_from_url("https://youtu.be/pv21e6iEZUw?si=x").as_deref(), Some("pv21e6iEZUw"));
        assert_eq!(
            id_from_url("https://www.youtube.com/watch?v=7jrKjkrX3Gw&list=PLxyz").as_deref(),
            Some("7jrKjkrX3Gw")
        );
        assert_eq!(id_from_url("https://www.youtube.com/shorts/abcDEF12345").as_deref(), Some("abcDEF12345"));
        assert_eq!(id_from_url("https://www.youtube.com/live/abcDEF12345?feature=share").as_deref(), Some("abcDEF12345"));
        assert_eq!(id_from_url("https://example.com/video/123"), None); // no recognizable id
    }

    #[test]
    fn the_notable_flag_menu_is_well_formed() {
        assert!(!NOTABLE_FLAGS.is_empty());
        for (flag, blurb) in NOTABLE_FLAGS {
            assert!(flag.starts_with("--"), "{flag}");
            assert!(!blurb.is_empty(), "{flag} needs a blurb");
        }
    }
}
