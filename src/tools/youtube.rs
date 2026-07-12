//! Driving the bundled `yt-dlp`: URL classification, per-video subtitle resolution, download
//! argv assembly, the playlist unplayable-report, and the flag menu. The `dl_yt` command
//! ([`crate::categories::download`]) stays a thin shell over this, the same way
//! [`super::python`] backs the `py_*` commands. It lives in `tools` rather than `support`
//! because it resolves and runs the bundled binaries — a layer `support` sits below.
//!
//! Downloads are driven **one video at a time** (playlists and channel tabs are scanned first,
//! then each entry gets its own yt-dlp invocation). That costs one extra metadata request per
//! video, and buys the thing a single batch invocation cannot express: per-video subtitle
//! selection. YouTube's auto-caption table is a ~157-language translate matrix, so a static
//! `--sub-langs` is either too narrow (missing the uploader's own languages) or downloads the
//! whole matrix; [`sub_langs_for`] instead computes each video's exact list.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::support::doc_style;
use crate::support::exec::{capture_output, capture_stdout, run_reporting_code};
use crate::support::preferences;

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

/// What every yt-dlp invocation of one `dl_yt` run shares.
#[derive(Clone, Copy, Default)]
pub(crate) struct Env<'a> {
    /// The bundled ffmpeg's directory, when it exists — powers the subtitle embedding.
    pub(crate) ffmpeg_dir: Option<&'a Path>,
    /// The `--cookies` file, when the user supplied one.
    pub(crate) cookies: Option<&'a Path>,
    /// Audio-only extraction (`-x`).
    pub(crate) audio: bool,
    /// Height cap, as a format-sort preference (`-S res:N`).
    pub(crate) res: Option<u32>,
    /// Raw passthrough args, appended after every default so they win any flag repeated.
    pub(crate) extra: &'a [String],
    /// The bundled deno, when it exists — yt-dlp's YouTube extractor wants a JS runtime (EJS)
    /// or some formats go missing.
    pub(crate) js_runtime: Option<&'a Path>,
}

/// The bundled ffmpeg's directory, when the bundle exists ([`super::resolve`] falls back to
/// the bare name otherwise, which means "let yt-dlp find a PATH one").
pub(crate) fn bundled_ffmpeg_dir() -> Option<PathBuf> {
    let resolved = super::resolve("ffmpeg");
    if resolved == "ffmpeg" {
        return None;
    }
    PathBuf::from(resolved).parent().map(Path::to_path_buf)
}

/// The bundled deno binary, when the bundle exists (else yt-dlp looks for a PATH deno itself).
pub(crate) fn bundled_deno() -> Option<PathBuf> {
    let resolved = super::resolve("deno");
    (resolved != "deno").then(|| PathBuf::from(resolved))
}

/// The `--js-runtimes deno:<path>` pair.
fn js_runtime_flag(deno: &Path) -> [OsString; 2] {
    let mut runtime = OsString::from("deno:");
    runtime.push(deno.as_os_str());
    ["--js-runtimes".into(), runtime]
}

/// Starter argv for the metadata-side invocations (probes, scans): even those run the YouTube
/// extractor, which warns — and may miss formats — without a JS runtime, and warns again when
/// it can't find an ffmpeg. Hand it both bundles when they exist.
fn seeded() -> Vec<OsString> {
    let mut argv =
        bundled_deno().map(|deno| js_runtime_flag(&deno).to_vec()).unwrap_or_default();
    if let Some(dir) = bundled_ffmpeg_dir() {
        argv.push("--ffmpeg-location".into());
        argv.push(dir.into_os_string());
    }
    argv
}

/// A lone video: probe its subtitle situation, then download with the exact list.
pub(crate) fn download_video(url: &str, into: &Path, env: Env) -> i32 {
    let langs = video_langs(url, None);
    run(video_argv(url, into, env, &langs))
}

/// Playlist mode. The flat scan comes first — it still *names* entries nothing can play
/// anymore, which downloads would only surface as opaque errors — the traceability report and
/// the download archive live inside the playlist's own folder, and every playable, not-yet-
/// archived entry is downloaded individually with its own subtitle list.
pub(crate) fn download_playlist(url: &str, id: &str, into: &Path, env: Env) -> i32 {
    let Some(scan) = scan_playlist(url, "%(title)S[%(id)S]") else {
        eprintln!(
            "dl_yt: could not scan the playlist — downloading in one pass, EN-only subtitles, \
             no unplayable report"
        );
        return run(playlist_argv(url, into, into, env, &default_langs(), None));
    };
    // The playlist's folder, spelled exactly as yt-dlp's template expansion will spell it.
    let dir = scan.dirname.clone().unwrap_or_else(|| format!("[{id}]"));
    let home = into.join(&dir);
    if let Err(err) = std::fs::create_dir_all(&home) {
        eprintln!("dl_yt: cannot create {}: {err}", home.display());
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
            Err(err) => eprintln!("dl_yt: could not write {}: {err}", report.display()),
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
    if pending.is_empty() {
        return 0;
    }
    download_pending(url, &pending, into, &home, env, |url, into, home, env, langs, items| {
        playlist_argv(url, into, home, env, langs, Some(items))
    })
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
    let probed = batch_probe(url, &indexes);
    let planned: Vec<(String, Vec<String>)> = pending
        .iter()
        .map(|entry| {
            let langs = probed
                .iter()
                .find(|(index, _)| *index == entry.index)
                .map(|(_, langs)| langs.clone())
                .unwrap_or_else(default_langs);
            (entry.index.clone(), langs)
        })
        .collect();
    let mut worst = 0;
    for (items, langs) in group_by_langs(&planned) {
        let subs = if langs.is_empty() { "none".to_string() } else { langs.join(",") };
        println!(
            "=== downloading {} entr{} (subs: {subs}) ===",
            items.split(',').count(),
            if items.contains(',') { "ies" } else { "y" },
        );
        let code = run(argv(url, into, home, env, &langs, &items));
        if code != 0 && worst == 0 {
            worst = code;
        }
    }
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
        let scan = match scan_tab(&tab_url, "%(uploader)S[%(channel_id)S]") {
            TabScan::Found(scan) => scan,
            TabScan::Missing => {
                println!("the channel has no `{tab}` tab");
                continue;
            }
            TabScan::Failed => {
                eprintln!("dl_yt: could not read the `{tab}` tab — moving on");
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
        if pending.is_empty() {
            succeeded += 1;
            continue;
        }
        let tab_code =
            download_pending(&tab_url, &pending, into, &home, env, |url, into, home, env, langs, items| {
                channel_tab_argv(url, tab, into, home, env, langs, items)
            });
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
    let green = doc_style::_wrap(["", "", "g"]);
    for (flag, blurb) in NOTABLE_FLAGS {
        let pad = " ".repeat(width - flag.len());
        println!("  {}{pad}  {blurb}", doc_style::_scoped(&green, flag));
    }
    println!("\n{}", doc_style::_header("Everything yt-dlp accepts:"));
    run_reporting_code(super::resolve("yt-dlp"), ["--help"])
}

/// The flags worth knowing about, shown by [`taglist`]: `(flag as you'd type it, what it does)`.
pub(crate) const NOTABLE_FLAGS: &[(&str, &str)] = &[
    ("--sponsorblock-remove sponsor", "cut sponsored segments out of the video"),
    ("--write-description", "save the description as a sidecar file"),
    ("--write-info-json", "save every scrap of metadata as JSON"),
    ("--playlist-items 3,5-7", "download only a slice of a playlist"),
    ("--limit-rate 2M", "cap the download speed"),
    ("--merge-output-format webm/mkv", "prefer webm (forfeits embedded cover art)"),
    ("--sub-langs all", "every real subtitle language, not just EN"),
    ("--proxy URL", "route through a proxy"),
];

// --- subtitle resolution ---------------------------------------------------------

/// The subtitle languages to embed for one video, given what exists. The policy: every language
/// the uploader published; the video's native language as auto-captions when the uploader
/// published none in it (the plain auto track, else the raw `-orig` one); and EN — real, else
/// auto-translated — unless a real or native EN already covers it. Nothing is ever doubled.
fn sub_langs_for(reals: &[String], autos: &[String], orig: Option<&str>) -> Vec<String> {
    let base = |lang: &str| lang.split('-').next().unwrap_or(lang).to_lowercase();
    let mut langs: Vec<String> =
        reals.iter().filter(|lang| *lang != "live_chat").cloned().collect();
    let native = orig.filter(|orig| !orig.is_empty()).map(&base);
    if let Some(native) = &native {
        if !langs.iter().any(|lang| base(lang) == *native) {
            // Embedded track titles come from YouTube's own names, and only some variants
            // announce their machine origins: `x-orig` is titled "X (Original)" where plain
            // `x` reads like an uploader's track — prefer the honest label.
            let labeled = format!("{native}-orig");
            if autos.contains(&labeled) {
                langs.push(labeled);
            } else if autos.contains(native) {
                langs.push(native.clone());
            }
        }
    }
    if !langs.iter().any(|lang| base(lang) == "en") {
        // Same labeling preference for the translated fallback ("English from X") — though
        // YouTube doesn't always publish the pair key, and plain `en` is the plan B.
        let labeled = native.map(|native| format!("en-{native}"));
        if let Some(labeled) = labeled.filter(|labeled| autos.contains(labeled)) {
            langs.push(labeled);
        } else if autos.iter().any(|auto| auto == "en") {
            langs.push("en".to_string());
        }
    }
    langs
}

/// The pre-probe fallback (and the no-probe path): today's EN-family behavior.
fn default_langs() -> Vec<String> {
    ["en", "en-US", "en-GB"].map(str::to_string).to_vec()
}

/// One video's subtitle list, via a metadata probe (`item` picks an entry when `url` is a
/// playlist or tab). An unprobeable video falls back to the EN default — resilience over
/// completeness.
fn video_langs(url: &str, item: Option<&str>) -> Vec<String> {
    probe_subs(url, item)
        .map(|(reals, autos, orig)| sub_langs_for(&reals, &autos, orig.as_deref()))
        .unwrap_or_else(default_langs)
}

/// Per-entry print format of the batch subtitle probe.
const PROBE_FORMAT: &str = "%(playlist_index)s\t%(language)s\t%(subtitles)j\t%(automatic_captions)j";

/// Probe the subtitle situation of many entries in ONE yt-dlp invocation (process startup and
/// player work are the expensive parts — per-entry probes multiply them). Entries that fail to
/// extract are simply absent; callers fall back to [`default_langs`] for those.
fn batch_probe(url: &str, indexes: &[String]) -> Vec<(String, Vec<String>)> {
    let mut argv = seeded();
    argv.extend([
        OsString::from("--ignore-errors"),
        "--playlist-items".into(), indexes.join(",").into(),
        "--print".into(), PROBE_FORMAT.into(),
        url.into(),
    ]);
    capture_stdout(super::resolve("yt-dlp"), argv).map(|out| parse_batch(&out)).unwrap_or_default()
}

/// Parse [`batch_probe`]'s lines into `(entry index, its subtitle list)`.
fn parse_batch(out: &str) -> Vec<(String, Vec<String>)> {
    out.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(4, '\t');
            let (index, language, subs, autos) =
                (fields.next()?, fields.next()?, fields.next()?, fields.next()?);
            let orig = (!language.is_empty() && language != "NA").then_some(language);
            Some((
                index.to_string(),
                sub_langs_for(&json_top_keys(subs), &json_top_keys(autos), orig),
            ))
        })
        .collect()
}

/// Group entries by their computed subtitle list, so each distinct list becomes ONE download
/// invocation (`--playlist-items` takes a comma list) instead of one per entry.
fn group_by_langs(planned: &[(String, Vec<String>)]) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    for (index, langs) in planned {
        match groups.iter_mut().find(|(_, group_langs)| group_langs == langs) {
            Some((indexes, _)) => indexes.push(index.clone()),
            None => groups.push((vec![index.clone()], langs.clone())),
        }
    }
    groups.into_iter().map(|(indexes, langs)| (indexes.join(","), langs)).collect()
}

/// Ask yt-dlp what subtitles a video has: `(real languages, auto-caption keys, native language)`.
fn probe_subs(url: &str, item: Option<&str>) -> Option<(Vec<String>, Vec<String>, Option<String>)> {
    let mut argv: Vec<OsString> = seeded();
    if let Some(index) = item {
        argv.extend([OsString::from("--playlist-items"), index.into()]);
    }
    argv.extend(
        ["--print", "%(language)s", "--print", "%(subtitles)j", "--print", "%(automatic_captions)j"]
            .map(OsString::from),
    );
    argv.push(url.into());
    let out = capture_stdout(super::resolve("yt-dlp"), argv)?;
    let mut lines = out.lines();
    let orig = lines.next()?.trim();
    let orig = (!orig.is_empty() && orig != "NA").then(|| orig.to_string());
    let reals = json_top_keys(lines.next()?);
    let autos = json_top_keys(lines.next()?);
    Some((reals, autos, orig))
}

/// The top-level keys of a JSON object, dependency-free: walk the text tracking brace/bracket
/// depth and string state; a string that closes at depth 1 and is followed by `:` is a key.
/// Enough for yt-dlp's `%(subtitles)j` dumps (string keys, array values).
fn json_top_keys(json: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut depth = 0u32;
    let mut in_string: Option<String> = None;
    let mut escaped = false;
    let mut pending: Option<String> = None; // a string closed at depth 1, awaiting its `:`
    for ch in json.chars() {
        if let Some(buffer) = in_string.as_mut() {
            if escaped {
                buffer.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                let text = in_string.take().unwrap();
                if depth == 1 {
                    pending = Some(text);
                }
            } else {
                buffer.push(ch);
            }
            continue;
        }
        match ch {
            '"' => in_string = Some(String::new()),
            ':' => {
                if let Some(key) = pending.take() {
                    keys.push(key);
                }
            }
            ' ' | '\t' | '\n' | '\r' => {}
            '{' | '[' => {
                depth += 1;
                pending = None;
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                pending = None;
            }
            _ => pending = None,
        }
    }
    keys
}

// --- argv assembly ---------------------------------------------------------------

/// The file name every mode shares: sortable upload date, title, and the video id that keeps
/// any file traceable back to its source (ideas kept from the old dl_youtube.py).
const YT_NAME: &str = "%(upload_date)s__%(title)s__%(id)s.%(ext)s";

/// The download-archive's file name, dropped inside whatever folder owns the collection.
const ARCHIVE_NAME: &str = ".yt_archive.txt";

/// The flags every yt-dlp run shares: keep going past broken entries, parallel fragments, the
/// video's requested subtitles fetched-and-embedded (sidecars cleaned up), its title/uploader/
/// date/description and thumbnail embedded as tags and cover art (thumbnail converted to jpg —
/// the one format the mkv cover-art convention reliably recognizes), mkv output, and a
/// download-archive (at `archive_dir`) so interrupted or repeated runs resume instead of
/// redoing. yt-dlp marks the archive only after post-processing finishes — verified by racing
/// the archive file against the output directory.
///
/// mkv beats webm as the merge container even though both box the same codecs at the same
/// quality: mkv accepts any codec yt-dlp picks (h264 fallbacks included) and embeds the
/// thumbnail, where webm refuses attachments and leaves the art as loose image files.
fn common(archive_dir: &Path, env: Env, langs: &[String]) -> Vec<OsString> {
    let mut argv: Vec<OsString> = ["--ignore-errors", "--concurrent-fragments", "4"]
        .into_iter()
        .map(OsString::from)
        .collect();
    if !env.audio && !langs.is_empty() {
        // Subtitles ride video files: they can't embed into an extracted audio track, and
        // requesting them there would only strand `.vtt` sidecars beside the audio.
        argv.extend(["--write-subs", "--write-auto-subs", "--sub-langs"].map(OsString::from));
        argv.push(langs.join(",").into());
        argv.extend(["--embed-subs", "--compat-options", "no-keep-subs"].map(OsString::from));
    }
    argv.extend(
        [
            "--embed-metadata", "--embed-chapters",
            "--embed-thumbnail", "--convert-thumbnails", "jpg",
            "--merge-output-format", "mkv",
            "--download-archive",
        ]
        .map(OsString::from),
    );
    argv.push(archive_dir.join(ARCHIVE_NAME).into_os_string());
    if let Some(dir) = env.ffmpeg_dir {
        // yt-dlp only finds PATH ffmpegs on its own — point it at the bundled build.
        argv.push("--ffmpeg-location".into());
        argv.push(dir.as_os_str().to_owned());
    }
    if let Some(deno) = env.js_runtime {
        // Same story for the JS runtime: yt-dlp only searches PATH for deno.
        argv.extend(js_runtime_flag(deno));
    }
    if let Some(file) = env.cookies {
        argv.push("--cookies".into());
        argv.push(file.as_os_str().to_owned());
    }
    if env.audio {
        argv.push("-x".into());
    }
    if let Some(height) = env.res {
        argv.push("-S".into());
        argv.push(format!("res:{height}").into());
    }
    argv
}

fn run(argv: Vec<OsString>) -> i32 {
    run_reporting_code(super::resolve("yt-dlp"), argv)
}

/// A lone video, named [`YT_NAME`] directly under the destination (which also holds the
/// archive). `--no-playlist` pins the meaning even when the link happens to carry a `list=`
/// (the `--single` case).
fn video_argv(url: &str, into: &Path, env: Env, langs: &[String]) -> Vec<OsString> {
    let mut argv = common(into, env, langs);
    argv.extend(["--no-playlist", "--output"].map(OsString::from));
    argv.push(into.join(YT_NAME).into_os_string());
    argv.extend(env.extra.iter().map(OsString::from));
    argv.push(url.into());
    argv
}

/// A playlist entry (or, `item`-less, a whole playlist in one pass — the scanless fallback):
/// its own `title[id]` folder under `into`, entries ordered by playlist position, the archive
/// at `archive_dir`.
fn playlist_argv(
    url: &str,
    into: &Path,
    archive_dir: &Path,
    env: Env,
    langs: &[String],
    item: Option<&str>,
) -> Vec<OsString> {
    let mut argv = common(archive_dir, env, langs);
    if let Some(index) = item {
        argv.extend([OsString::from("--playlist-items"), index.into()]);
    }
    argv.push("--output".into());
    argv.push(
        into.join(format!("%(playlist_title)s[%(playlist_id)s]/%(playlist_index)03d__{YT_NAME}"))
            .into_os_string(),
    );
    argv.extend(env.extra.iter().map(OsString::from));
    argv.push(url.into());
    argv
}

/// The channel tabs, each downloaded into its own folder under `uploader[channel_id]/`; the
/// playlists tab is handled by recursion instead (see [`download_channel`]).
const CHANNEL_TABS: &[&str] = &["videos", "shorts", "streams", "playlists"];

fn channel_tab_argv(
    tab_url: &str,
    tab: &str,
    into: &Path,
    archive_dir: &Path,
    env: Env,
    langs: &[String],
    item: &str,
) -> Vec<OsString> {
    let mut argv = common(archive_dir, env, langs);
    argv.extend([OsString::from("--playlist-items"), item.into()]);
    argv.push("--output".into());
    argv.push(
        into.join(format!("%(uploader)s[%(channel_id)s]/{tab}/{YT_NAME}")).into_os_string(),
    );
    argv.extend(env.extra.iter().map(OsString::from));
    argv.push(tab_url.into());
    argv
}

// --- collection scanning ----------------------------------------------------------

/// One playlist entry as the flat scan sees it.
struct ScanEntry {
    index: String,
    id: String,
    title: String,
}

/// A scanned playlist: its title, its folder name (as yt-dlp will spell it), and every entry,
/// dead or alive.
struct PlaylistScan {
    title: String,
    dirname: Option<String>,
    entries: Vec<ScanEntry>,
}

/// Per-entry print format of the flat scan (playlist title rides along on every line).
const SCAN_FORMAT: &str = "%(playlist_index)s\t%(id)s\t%(title)s\t%(playlist_title)s";

/// A channel tab's scan outcome: tabs a channel simply doesn't have are normal, not errors.
enum TabScan {
    Found(PlaylistScan),
    Missing,
    Failed,
}

/// Scan a channel tab with stderr captured, so "this channel has no such tab" — yt-dlp's error,
/// but an everyday reality — turns into a calm message instead of an ERROR dump; anything else
/// on stderr is passed through as the real failure it is.
fn scan_tab(url: &str, dir_template: &str) -> TabScan {
    let mut args = seeded();
    args.extend([
        OsString::from("--flat-playlist"),
        "--print".into(), SCAN_FORMAT.into(),
        "--print".into(), format!("playlist:{dir_template}").into(),
        url.into(),
    ]);
    let Some((ok, stdout, stderr)) = capture_output(super::resolve("yt-dlp"), args) else {
        return TabScan::Failed;
    };
    if ok {
        if let Some(scan) = parse_scan(&stdout) {
            return TabScan::Found(scan);
        }
        return TabScan::Missing; // reachable but empty: nothing to download either way
    }
    if tab_absence(&stderr) {
        return TabScan::Missing;
    }
    eprint!("{stderr}");
    TabScan::Failed
}

/// Whether yt-dlp's stderr says the tab doesn't exist (its stable phrasing:
/// `ERROR: [youtube:tab] @handle: This channel does not have a streams tab`).
fn tab_absence(stderr: &str) -> bool {
    stderr.contains("does not have a")
}

/// One flat invocation lists the entries AND prints the collection's folder name (playlist
/// scope, sanitized by the `S` conversion in `dir_template` — a channel tab's flat entries
/// often lack `uploader`/`channel_id`, the `NA[NA]` trap, while the tab's own fields don't).
fn scan_playlist(url: &str, dir_template: &str) -> Option<PlaylistScan> {
    let mut argv = seeded();
    argv.extend([
        OsString::from("--flat-playlist"),
        "--print".into(), SCAN_FORMAT.into(),
        "--print".into(), format!("playlist:{dir_template}").into(),
        url.into(),
    ]);
    let out = capture_stdout(super::resolve("yt-dlp"), argv)?;
    parse_scan(&out)
}

/// Parse the flat scan's tab-separated lines (malformed ones are skipped; `None` only when
/// nothing parsed at all).
fn parse_scan(out: &str) -> Option<PlaylistScan> {
    let mut title = String::new();
    let mut dirname = None;
    let mut entries = Vec::new();
    for line in out.lines() {
        if !line.contains('\t') {
            // The playlist-scope print: a lone tabless line (it arrives after the entries;
            // the last one wins). An `NA` in it means the fields weren't there — no folder.
            let line = line.trim();
            if !line.is_empty() && !line.contains("NA") {
                dirname = Some(line.to_string());
            }
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(index), Some(id), Some(entry_title)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if let Some(playlist_title) = fields.next() {
            title = playlist_title.to_string();
        }
        entries.push(ScanEntry {
            index: index.to_string(),
            id: id.to_string(),
            title: entry_title.to_string(),
        });
    }
    (!entries.is_empty()).then_some(PlaylistScan { title, dirname, entries })
}

/// The video ids a download-archive has recorded (its lines are `<extractor> <id>` — the
/// extractor prefix is yt-dlp's own on-disk format, kept so it can read the file back).
fn archived_ids(path: &Path) -> HashSet<String> {
    std::fs::read_to_string(path)
        .map(|content| {
            content
                .lines()
                .filter_map(|line| line.split_whitespace().nth(1).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The titles YouTube substitutes once an entry can't be played by anyone anymore.
const TOMBSTONES: &[&str] = &["[Private video]", "[Deleted video]", "[Unavailable video]"];

fn is_tombstone(title: &str) -> bool {
    TOMBSTONES.contains(&title)
}

/// The unplayable-entries report: everything a future search needs to trace what each entry
/// was — its id (the strongest key), its position, and lookup links for the WaybackMachine
/// and filmot (which indexes titles of deleted videos).
fn unplayable_report(playlist_title: &str, playlist_id: &str, dead: &[&ScanEntry]) -> String {
    let mut report = format!(
        "Unplayable entries of playlist: {playlist_title} [{playlist_id}]\n\
         recorded {} — video ids stay valid keys for archive services even after deletion\n\n",
        preferences::datehour_stamp()
    );
    for entry in dead {
        report += &format!(
            "#{index}  {title}\n  \
             was:      https://www.youtube.com/watch?v={id}\n  \
             wayback:  https://web.archive.org/web/*/https://www.youtube.com/watch?v={id}\n  \
             filmot:   https://filmot.com/video/{id}\n\n",
            index = entry.index,
            title = entry.title,
            id = entry.id,
        );
    }
    report
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
    fn the_single_flag_pins_a_video_carrying_a_playlist_to_just_the_video() {
        let url = "https://www.youtube.com/watch?v=7jrKjkrX3Gw&list=PLxyz";
        assert_eq!(classify(url, true), Link::Video);
        let argv = video_argv(url, Path::new("."), Env::default(), &default_langs());
        assert!(argv.contains(&OsString::from("--no-playlist")), "{argv:?}");
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
    fn subtitle_policy_covers_uploader_native_and_english_without_doubles() {
        let s = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // The PlayStation case: uploader published en+ko — both taken, no auto anything.
        assert_eq!(
            sub_langs_for(&s(&["en", "ko"]), &s(&["ko", "ko-orig", "en", "fr"]), Some("ko")),
            s(&["en", "ko"])
        );
        // No uploader subs on a Korean video: native auto + translated EN — the variants whose
        // YouTube names announce the machine origin win ("Korean (Original)", "English from
        // Korean"), so nobody mistakes them for the uploader's script.
        assert_eq!(
            sub_langs_for(&[], &s(&["ko", "ko-orig", "en", "en-ko"]), Some("ko")),
            s(&["ko-orig", "en-ko"])
        );
        assert_eq!(
            sub_langs_for(&[], &s(&["ko", "en"]), Some("ko")),
            s(&["ko", "en"]),
            "the unlabeled keys are the plan B when no labeled variant is published"
        );
        // The Hebrew case, legacy `iw` code and all (lang=iw, keys iw/iw-orig/en).
        assert_eq!(
            sub_langs_for(&[], &s(&["iw", "iw-orig", "en"]), Some("iw")),
            s(&["iw-orig", "en"])
        );
        // English-native video with no uploader subs: EN once, not twice — labeled.
        assert_eq!(sub_langs_for(&[], &s(&["en", "en-orig"]), Some("en")), s(&["en-orig"]));
        // Real Korean only: EN still arrives as the auto translation.
        assert_eq!(sub_langs_for(&s(&["ko"]), &s(&["en", "ko-orig"]), Some("ko")), s(&["ko", "en"]));
        // live_chat is not a subtitle; unknown native language degrades gracefully.
        assert_eq!(sub_langs_for(&s(&["live_chat", "en"]), &[], None), s(&["en"]));
        assert!(sub_langs_for(&[], &[], None).is_empty(), "nothing available, nothing requested");
    }

    #[test]
    fn json_top_keys_reads_only_the_outer_object() {
        assert_eq!(
            json_top_keys(r#"{"en": [{"url": "x", "name": "English"}], "ko": []}"#),
            ["en", "ko"]
        );
        assert_eq!(json_top_keys(r#"{"a-b": [], "c": {"nested": [1, 2]}}"#), ["a-b", "c"]);
        assert_eq!(json_top_keys(r#"{"esc\"aped": []}"#), [r#"esc"aped"#]);
        assert!(json_top_keys("{}").is_empty());
        assert!(json_top_keys("null").is_empty());
    }

    #[test]
    fn every_yt_run_embeds_metadata_resumes_and_targets_mkv() {
        let argv = common(
            Path::new("/dl"),
            Env {
                ffmpeg_dir: Some(Path::new("/ff/bin")),
                cookies: Some(Path::new("/c.txt")),
                js_runtime: Some(Path::new("/dn/deno")),
                ..Default::default()
            },
            &default_langs(),
        );
        let text: Vec<String> = argv.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
        for expected in [
            "--write-subs", "--write-auto-subs", "--embed-subs",
            "--sub-langs", "en,en-US,en-GB",
            "--embed-metadata", "--embed-chapters",
            "--embed-thumbnail", "--convert-thumbnails", "jpg",
            "--merge-output-format", "mkv",
            "--ignore-errors",
            "--download-archive", "/dl/.yt_archive.txt",
            "--ffmpeg-location", "/ff/bin",
            "--cookies", "/c.txt",
            "--js-runtimes", "deno:/dn/deno",
        ] {
            assert!(text.iter().any(|arg| arg == expected), "missing {expected}: {text:?}");
        }
        // With no bundled ffmpeg and no cookies, neither flag appears — nor the knobs. And a
        // video with no subtitles anywhere requests none.
        let bare = common(Path::new("/dl"), Env::default(), &[]);
        for absent in
            ["--ffmpeg-location", "--cookies", "--js-runtimes", "-x", "-S", "--write-subs", "--sub-langs"]
        {
            assert!(!bare.iter().any(|arg| arg == absent), "{absent} leaked in");
        }
    }

    #[test]
    fn the_optional_knobs_shape_the_run_and_extras_land_last() {
        let extra = vec!["--merge-output-format".to_string(), "webm/mkv".to_string()];
        let env = Env { audio: true, res: Some(1080), extra: &extra, ..Default::default() };
        let argv = video_argv("https://u", Path::new("/dl"), env, &default_langs());
        assert!(argv.contains(&OsString::from("-x")), "audio-only: {argv:?}");
        assert!(
            !argv.contains(&OsString::from("--embed-subs")),
            "audio can't hold subs — requesting them would strand sidecars: {argv:?}"
        );
        let sort = argv.iter().position(|arg| arg == "-S").expect("-S");
        assert_eq!(argv[sort + 1], OsString::from("res:1080"));
        // Extras sit after every default (so a repeated flag resolves their way) and only
        // the URL follows them.
        let output = argv.iter().position(|arg| arg == "--output").unwrap();
        let webm = argv.iter().position(|arg| arg == "webm/mkv").unwrap();
        assert!(output < webm, "extras must come after the defaults they override");
        assert_eq!(argv.last().unwrap(), &OsString::from("https://u"));
        assert_eq!(argv[argv.len() - 2], OsString::from("webm/mkv"));
    }

    #[test]
    fn each_mode_shapes_its_own_output_tree_and_archive_home() {
        let last = |argv: &[OsString]| argv.last().unwrap().to_string_lossy().into_owned();
        let template = |argv: &[OsString]| {
            let at = argv.iter().position(|a| a == "--output").expect("--output");
            argv[at + 1].to_string_lossy().into_owned()
        };
        let archive = |argv: &[OsString]| {
            let at = argv.iter().position(|a| a == "--download-archive").expect("archive flag");
            argv[at + 1].to_string_lossy().into_owned()
        };
        let langs = default_langs();

        let video = video_argv("https://youtu.be/x", Path::new("/dl"), Env::default(), &langs);
        assert_eq!(template(&video), format!("/dl/{YT_NAME}"));
        assert_eq!(last(&video), "https://youtu.be/x", "the url comes last");

        // A playlist entry: files under the playlist folder, the archive inside it, and the
        // specific entry selected.
        let entry = playlist_argv(
            "https://l",
            Path::new("/dl"),
            Path::new("/dl/My List[PL1]"),
            Env::default(),
            &langs,
            Some("4"),
        );
        assert!(template(&entry).starts_with("/dl/%(playlist_title)s[%(playlist_id)s]/"));
        assert!(template(&entry).contains("%(playlist_index)03d__"));
        assert_eq!(archive(&entry), "/dl/My List[PL1]/.yt_archive.txt");
        let items = entry.iter().position(|a| a == "--playlist-items").expect("item selection");
        assert_eq!(entry[items + 1], OsString::from("4"));

        let tab = channel_tab_argv(
            "https://c/videos",
            "videos",
            Path::new("/dl"),
            Path::new("/dl/Chan[UC1]"),
            Env::default(),
            &langs,
            "7",
        );
        assert!(template(&tab).starts_with("/dl/%(uploader)s[%(channel_id)s]/videos/"));
        assert_eq!(archive(&tab), "/dl/Chan[UC1]/.yt_archive.txt");
        assert_eq!(last(&tab), "https://c/videos", "the tab url comes last");
    }

    #[test]
    fn the_flat_scan_parses_spots_tombstones_and_takes_the_folder_line() {
        let out = "1\tabc123def45\tA fine video\tMy Playlist\n\
                   2\tdead0000001\t[Private video]\tMy Playlist\n\
                   3\tdead0000002\t[Deleted video]\tMy Playlist\n\
                   My Playlist[PLxyz]\n";
        let scan = parse_scan(out).expect("parses");
        assert_eq!(scan.title, "My Playlist");
        assert_eq!(scan.entries.len(), 3);
        assert_eq!(scan.dirname.as_deref(), Some("My Playlist[PLxyz]"));
        let dead: Vec<&ScanEntry> =
            scan.entries.iter().filter(|e| is_tombstone(&e.title)).collect();
        assert_eq!(dead.len(), 2);
        // A folder line carrying NA means the fields weren't there — no folder is better
        // than an `NA[NA]` one.
        let na = parse_scan("1\tid234567890\tT\tP\nNA[NA]\n").expect("parses");
        assert_eq!(na.dirname, None);
        assert!(parse_scan("").is_none(), "an empty scan is a failed scan");
    }

    #[test]
    fn the_batch_probe_parses_per_entry_and_groups_by_subtitle_list() {
        // Entry 1: real en+ko. Entry 2: nothing anywhere. Entry 3: auto-only Korean video.
        let out = "1\tko\t{\"en\": [], \"ko\": []}\t{}\n\
                   2\tNA\t{}\t{}\n\
                   3\tko\t{}\t{\"ko\": [], \"en\": []}\n";
        let plan = parse_batch(out);
        assert_eq!(plan[0], ("1".to_string(), vec!["en".to_string(), "ko".to_string()]));
        assert_eq!(plan[1], ("2".to_string(), vec![]));
        assert_eq!(plan[2], ("3".to_string(), vec!["ko".to_string(), "en".to_string()]));
        let groups = group_by_langs(&plan);
        assert_eq!(groups.len(), 3, "three distinct lists here");
        // Same list → one invocation covering both entries.
        let twin = vec![plan[0].clone(), ("9".to_string(), plan[0].1.clone())];
        let merged = group_by_langs(&twin);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].0, "1,9", "indexes join into one --playlist-items value");
    }

    #[test]
    fn archived_ids_read_the_second_column() {
        let dir = std::env::temp_dir().join(format!("bashrs_arch_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(ARCHIVE_NAME);
        std::fs::write(&file, "youtube abc123\nyoutube def456\nmalformed\n").unwrap();
        let ids = archived_ids(&file);
        assert!(ids.contains("abc123") && ids.contains("def456"));
        assert_eq!(ids.len(), 2);
        assert!(archived_ids(Path::new("/no/such/archive")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_unplayable_report_traces_each_entry_by_id() {
        let dead = [ScanEntry {
            index: "2".into(),
            id: "dead0000001".into(),
            title: "[Private video]".into(),
        }];
        let refs: Vec<&ScanEntry> = dead.iter().collect();
        let report = unplayable_report("My Playlist", "PLxyz", &refs);
        assert!(report.contains("My Playlist [PLxyz]"));
        assert!(report.contains("#2  [Private video]"));
        assert!(report.contains("https://www.youtube.com/watch?v=dead0000001"));
        assert!(report
            .contains("https://web.archive.org/web/*/https://www.youtube.com/watch?v=dead0000001"));
        assert!(report.contains("https://filmot.com/video/dead0000001"));
    }

    #[test]
    fn a_missing_tab_is_recognized_by_ytdlps_phrasing() {
        assert!(tab_absence("ERROR: [youtube:tab] @x: This channel does not have a streams tab"));
        assert!(!tab_absence("ERROR: [youtube:tab] @x: Unable to download webpage"));
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
