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
    let resolved = crate::tools::resolve("ffmpeg");
    if resolved == "ffmpeg" {
        return None;
    }
    PathBuf::from(resolved).parent().map(Path::to_path_buf)
}

/// The bundled deno binary, when the bundle exists (else yt-dlp looks for a PATH deno itself).
pub(crate) fn bundled_deno() -> Option<PathBuf> {
    let resolved = crate::tools::resolve("deno");
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
    let (id, picks) = video_picks(url);
    let keys: Vec<String> = picks.iter().map(|pick| pick.key.clone()).collect();
    let code = run(video_argv(url, into, env, &keys));
    if code == 0 {
        if let Some(id) = id {
            mark_auto_titles(into, &id, &picks, env);
        }
    }
    code
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
        let keys: Vec<String> = default_picks().iter().map(|pick| pick.key.clone()).collect();
        return run(playlist_argv(url, into, into, env, &keys, None));
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
            mark_auto_titles(home, &plan.id, &plan.picks, env);
        }
    }
    worst
}

/// Rename the embedded titles of auto-generated subtitle tracks. YouTube's own names read like
/// authentic tracks — "Spanish (Original)" sounds MORE official than the uploader's, and a
/// translated track is often titled plain "English" — so every auto pick's track becomes
/// "<name> (auto-generated)", with the misleading " (Original)" dropped. A local stream-copy
/// remux (no re-encode, no network); any failure leaves the downloaded file as-is.
fn mark_auto_titles(root: &Path, id: &str, picks: &[Pick], env: Env) {
    let autos: Vec<&Pick> = picks.iter().filter(|pick| pick.auto).collect();
    if autos.is_empty() || env.audio {
        return;
    }
    let Some(file) = find_by_id(root, id) else { return };
    let ffprobe = env
        .ffmpeg_dir
        .map(|dir| dir.join("ffprobe").into_os_string())
        .unwrap_or_else(|| "ffprobe".into());
    let Some(listing) = capture_stdout(
        &ffprobe,
        [
            OsString::from("-v"), "error".into(),
            "-select_streams".into(), "s".into(),
            "-show_entries".into(), "stream_tags=title".into(),
            "-of".into(), "csv=p=0".into(),
            file.as_os_str().to_owned(),
        ],
    ) else {
        return;
    };
    let mut retitle: Vec<(usize, String)> = Vec::new(); // (subtitle-stream order, new title)
    for (order, title) in listing.lines().enumerate() {
        if let Some(pick) = autos.iter().find(|pick| pick.name == title.trim()) {
            retitle.push((order, stamped_title(&pick.name)));
        }
    }
    if retitle.is_empty() {
        return;
    }
    let ffmpeg = env
        .ffmpeg_dir
        .map(|dir| dir.join("ffmpeg").into_os_string())
        .unwrap_or_else(|| "ffmpeg".into());
    let stamped = file.with_extension("stamping.mkv");
    let mut argv: Vec<OsString> = ["-v", "error", "-y", "-i"].map(OsString::from).to_vec();
    argv.push(file.as_os_str().to_owned());
    argv.extend(["-map", "0", "-c", "copy"].map(OsString::from));
    for (order, title) in &retitle {
        argv.push(format!("-metadata:s:s:{order}").into());
        argv.push(format!("title={title}").into());
    }
    argv.push(stamped.as_os_str().to_owned());
    let renamed = matches!(
        std::process::Command::new(&ffmpeg).args(&argv).status(),
        Ok(status) if status.success()
    ) && std::fs::rename(&stamped, &file).is_ok();
    if renamed {
        for (_, title) in &retitle {
            println!("stamped subtitle track: {title}");
        }
    } else {
        let _ = std::fs::remove_file(&stamped);
        eprintln!("dl_yt: could not stamp auto-subtitle titles in {}", file.display());
    }
}

/// An auto track's honest title: YouTube's name with the actively-misleading " (Original)"
/// dropped and the machine origin stated.
fn stamped_title(name: &str) -> String {
    format!("{} (auto-generated)", name.replace(" (Original)", ""))
}

/// The downloaded file whose name carries `__<id>.` — the id rides in every output template
/// precisely so files stay findable. A shallow recursive walk under `root`.
fn find_by_id(root: &Path, id: &str) -> Option<PathBuf> {
    let marker = format!("__{id}.");
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.contains(&marker) && name.ends_with(".mkv")
            }) {
                return Some(path);
            }
        }
    }
    None
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
    let (program, args) = ytdlp_invocation(vec![OsString::from("--help")]);
    run_reporting_code(program, args)
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

/// One subtitle track the plan requests: its yt-dlp language key, YouTube's display name for
/// it (which becomes the embedded track title — what the post-pass matches on), and whether
/// it's auto-generated rather than the uploader's.
#[derive(Clone, Debug, PartialEq)]
struct Pick {
    key: String,
    name: String,
    auto: bool,
}

/// The subtitle tracks to embed for one video, given what exists. The policy: every language
/// the uploader published; the video's native language as auto-captions when the uploader
/// published none in it (the raw `-orig` track, else the plain one); and EN — real, else
/// auto-translated (the `en-<lang>` pair when published, plain `en` otherwise) — unless a real
/// or native EN already covers it. Nothing is ever doubled, and every auto pick is flagged so
/// [`mark_auto_titles`] can stamp its embedded title.
///
/// Every pick is a MUST-TRY, never a must-have — EN above all: YouTube lists auto-translations
/// for every video but its translation endpoint is aggressively rate-limited (429s, sometimes
/// permanently on shared/VPN IPs), so a listed track may never be served. `--ignore-errors`
/// rides every download exactly so a refused subtitle downgrades to a warning and the video
/// completes without it — don't "fix" a sub-only failure into a failed exit.
fn sub_picks_for(
    reals: &[(String, String)],
    autos: &[(String, String)],
    orig: Option<&str>,
) -> Vec<Pick> {
    let base = |lang: &str| lang.split('-').next().unwrap_or(lang).to_lowercase();
    let auto_pick = |key: &str| {
        autos.iter().find(|(auto_key, _)| auto_key == key).map(|(auto_key, name)| Pick {
            key: auto_key.clone(),
            name: if name.is_empty() { auto_key.clone() } else { name.clone() },
            auto: true,
        })
    };
    let mut picks: Vec<Pick> = reals
        .iter()
        .filter(|(key, _)| key != "live_chat")
        .map(|(key, name)| Pick { key: key.clone(), name: name.clone(), auto: false })
        .collect();
    let native = orig.filter(|orig| !orig.is_empty()).map(&base);
    if let Some(native) = &native {
        if !picks.iter().any(|pick| base(&pick.key) == *native) {
            if let Some(pick) = auto_pick(&format!("{native}-orig")).or_else(|| auto_pick(native))
            {
                picks.push(pick);
            }
        }
    }
    if !picks.iter().any(|pick| base(&pick.key) == "en") {
        let translated = native.as_ref().and_then(|native| auto_pick(&format!("en-{native}")));
        if let Some(pick) = translated.or_else(|| auto_pick("en")) {
            picks.push(pick);
        }
    }
    picks
}

/// The pre-probe fallback (and the no-probe path): the EN-family behavior — not flagged auto,
/// since without a probe nobody knows, and a wrong "(auto-generated)" stamp is worse than none.
fn default_picks() -> Vec<Pick> {
    ["en", "en-US", "en-GB"]
        .map(|key| Pick { key: key.to_string(), name: String::new(), auto: false })
        .to_vec()
}

/// One video's subtitle plan plus its id (the post-pass finds the file by it). An unprobeable
/// video falls back to the EN default — resilience over completeness.
fn video_picks(url: &str) -> (Option<String>, Vec<Pick>) {
    let mut argv = seeded();
    argv.extend(
        [
            "--print", "%(id)s",
            "--print", "%(language)s",
            "--print", "%(subtitles)j",
            "--print", "%(automatic_captions)j",
        ]
        .map(OsString::from),
    );
    argv.push(url.into());
    let (program, args) = ytdlp_invocation(argv);
    let Some(out) = capture_stdout(program, args) else {
        return (None, default_picks());
    };
    let mut lines = out.lines();
    let id = lines.next().map(str::trim).filter(|id| !id.is_empty() && *id != "NA");
    let (Some(language), Some(subs), Some(autos)) = (lines.next(), lines.next(), lines.next())
    else {
        return (id.map(str::to_string), default_picks());
    };
    let orig = Some(language.trim()).filter(|lang| !lang.is_empty() && *lang != "NA");
    let picks = sub_picks_for(&json_lang_names(subs), &json_lang_names(autos), orig);
    (id.map(str::to_string), picks)
}

/// Per-entry print format of the batch subtitle probe.
const PROBE_FORMAT: &str =
    "%(playlist_index)s\t%(id)s\t%(language)s\t%(subtitles)j\t%(automatic_captions)j";

/// One pending entry with its computed subtitle plan.
struct Planned {
    index: String,
    id: String,
    picks: Vec<Pick>,
}

/// Probe the subtitle situation of many entries in ONE yt-dlp invocation (process startup and
/// player work are the expensive parts — per-entry probes multiply them). Entries that fail to
/// extract are simply absent; callers fall back to [`default_picks`] for those.
fn batch_probe(url: &str, indexes: &[String]) -> Vec<Planned> {
    let mut argv = seeded();
    argv.extend([
        OsString::from("--ignore-errors"),
        "--playlist-items".into(), indexes.join(",").into(),
        "--print".into(), PROBE_FORMAT.into(),
        url.into(),
    ]);
    let (program, args) = ytdlp_invocation(argv);
    capture_stdout(program, args).map(|out| parse_batch(&out)).unwrap_or_default()
}

/// Parse [`batch_probe`]'s lines into per-entry plans.
fn parse_batch(out: &str) -> Vec<Planned> {
    out.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\t');
            let (index, id, language, subs, autos) =
                (fields.next()?, fields.next()?, fields.next()?, fields.next()?, fields.next()?);
            let orig = (!language.is_empty() && language != "NA").then_some(language);
            Some(Planned {
                index: index.to_string(),
                id: id.to_string(),
                picks: sub_picks_for(&json_lang_names(subs), &json_lang_names(autos), orig),
            })
        })
        .collect()
}

/// Group planned entries by their subtitle KEY list, so each distinct list becomes ONE download
/// invocation (`--playlist-items` takes a comma list) instead of one per entry. Auto flags may
/// differ within a group (the same `en` can be real for one entry, translated for another) —
/// that's per-entry data the post-pass reads from each [`Planned`].
fn group_by_langs(planned: &[Planned]) -> Vec<Vec<&Planned>> {
    let keys = |plan: &Planned| plan.picks.iter().map(|pick| pick.key.clone()).collect::<Vec<_>>();
    let mut groups: Vec<(Vec<String>, Vec<&Planned>)> = Vec::new();
    for plan in planned {
        let plan_keys = keys(plan);
        match groups.iter_mut().find(|(group_keys, _)| *group_keys == plan_keys) {
            Some((_, members)) => members.push(plan),
            None => groups.push((plan_keys, vec![plan])),
        }
    }
    groups.into_iter().map(|(_, members)| members).collect()
}

/// The top-level keys of a JSON object paired with the first `"name"` string inside each
/// key's value — dependency-free, same walking technique as [`json_top_keys`]. Fits yt-dlp's
/// `%(subtitles)j` shape: `{"en": [{"url": …, "name": "English"}, …], …}`; a key whose value
/// carries no name gets `""`.
fn json_lang_names(json: &str) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut depth = 0u32;
    let mut in_string: Option<String> = None;
    let mut escaped = false;
    let mut pending: Option<(u32, String)> = None; // a closed string, with its depth
    let mut await_name_value = false; // the last string was a `"name"` key inside a value
    for ch in json.chars() {
        if let Some(buffer) = in_string.as_mut() {
            if escaped {
                buffer.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                let text = in_string.take().unwrap();
                if await_name_value {
                    await_name_value = false;
                    if let Some((_, name)) = entries.last_mut() {
                        if name.is_empty() {
                            *name = text;
                        }
                    }
                } else {
                    pending = Some((depth, text));
                }
            } else {
                buffer.push(ch);
            }
            continue;
        }
        match ch {
            '"' => in_string = Some(String::new()),
            ':' => match pending.take() {
                Some((1, key)) => entries.push((key, String::new())),
                Some((_, key)) if key == "name" => await_name_value = true,
                _ => {}
            },
            ' ' | '\t' | '\n' | '\r' => {}
            '{' | '[' => {
                depth += 1;
                pending = None;
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                pending = None;
            }
            _ => {
                pending = None;
                await_name_value = false;
            }
        }
    }
    entries
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
    // `--ignore-errors` also keeps subtitle picks must-try: a 429'd track warns and is skipped
    // without failing the video (see `sub_picks_for`).
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
        // YouTube rate-limits its subtitle endpoint (429s, particularly on auto-translations);
        // a short pause per subtitle fetch stays under its radar and only taxes subbed videos.
        argv.extend(["--sleep-subtitles", "2"].map(OsString::from));
    }
    argv.extend(
        [
            "--sleep-subtitles", "2",
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

/// How to invoke yt-dlp: the bundled zipapp is run through the bundled python *explicitly* —
/// its `env python3` shebang would otherwise pick whatever python the caller's PATH offers,
/// and the curl_cffi impersonation support lives in ours. A system yt-dlp runs as itself.
fn ytdlp_invocation(args: Vec<OsString>) -> (OsString, Vec<OsString>) {
    let ytdlp = crate::tools::resolve("yt-dlp");
    if ytdlp == "yt-dlp" {
        return (ytdlp, args);
    }
    let mut full = vec![ytdlp];
    full.extend(args);
    (crate::tools::resolve("python3"), full)
}

fn run(argv: Vec<OsString>) -> i32 {
    let (program, args) = ytdlp_invocation(argv);
    let code = run_reporting_code(program, args);
    if code != 0 {
        // The interpreter's name in the line above obscures the real actor; and the most common
        // hard failure deserves its diagnosis spelled out.
        eprintln!(
            "dl_yt: yt-dlp failed (exit {code}). A `403 Forbidden` on video data usually means \
             YouTube has blocklisted this network's IP (VPN exits often are) — switching the \
             node/network tends to fix it, and --cookies is the other lever."
        );
    }
    code
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
    let (program, args) = ytdlp_invocation(args);
    let Some((ok, stdout, stderr)) = capture_output(program, args) else {
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
    let (program, args) = ytdlp_invocation(argv);
    let out = capture_stdout(program, args)?;
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

    /// The EN-family keys, as tests pass them to the argv builders.
    fn en_keys() -> Vec<String> {
        ["en", "en-US", "en-GB"].map(str::to_string).to_vec()
    }

    #[test]
    fn the_single_flag_pins_a_video_carrying_a_playlist_to_just_the_video() {
        let url = "https://www.youtube.com/watch?v=7jrKjkrX3Gw&list=PLxyz";
        assert_eq!(classify(url, true), Link::Video);
        let argv = video_argv(url, Path::new("."), Env::default(), &en_keys());
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

    /// `(key, name)` pairs the way the probe parser hands them over.
    fn named(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(key, name)| (key.to_string(), name.to_string())).collect()
    }

    fn keys_of(picks: &[Pick]) -> Vec<&str> {
        picks.iter().map(|pick| pick.key.as_str()).collect()
    }

    #[test]
    fn subtitle_policy_covers_uploader_native_and_english_without_doubles() {
        // The PlayStation case: uploader published en+ko — both taken, no auto anything.
        let real = sub_picks_for(
            &named(&[("en", "English"), ("ko", "Korean")]),
            &named(&[("ko", "Korean"), ("ko-orig", "Korean (Original)"), ("en", "English")]),
            Some("ko"),
        );
        assert_eq!(keys_of(&real), ["en", "ko"]);
        assert!(real.iter().all(|pick| !pick.auto), "uploader subs must never be stamped");

        // No uploader subs on a Korean video: native auto + translated EN, both flagged auto,
        // preferring the pair/orig keys when published.
        let auto = sub_picks_for(
            &[],
            &named(&[
                ("ko", "Korean"),
                ("ko-orig", "Korean (Original)"),
                ("en", "English"),
                ("en-ko", "English from Korean"),
            ]),
            Some("ko"),
        );
        assert_eq!(keys_of(&auto), ["ko-orig", "en-ko"]);
        assert!(auto.iter().all(|pick| pick.auto));
        assert_eq!(auto[0].name, "Korean (Original)", "the name is what the post-pass matches");

        // The Hebrew case, legacy `iw` code and all — no pair key published, plain `en` plan B.
        let hebrew = sub_picks_for(
            &[],
            &named(&[("iw", "Hebrew"), ("iw-orig", "Hebrew (Original)"), ("en", "English")]),
            Some("iw"),
        );
        assert_eq!(keys_of(&hebrew), ["iw-orig", "en"]);
        assert!(hebrew.iter().all(|pick| pick.auto), "plain `en` here is still a translation");

        // English-native video with no uploader subs: EN once, not twice.
        assert_eq!(
            keys_of(&sub_picks_for(
                &[],
                &named(&[("en", "English"), ("en-orig", "English (Original)")]),
                Some("en")
            )),
            ["en-orig"]
        );
        // Real Korean only: EN still arrives as the auto translation, flagged as such.
        let mixed = sub_picks_for(
            &named(&[("ko", "Korean")]),
            &named(&[("en", "English"), ("ko-orig", "Korean (Original)")]),
            Some("ko"),
        );
        assert_eq!(keys_of(&mixed), ["ko", "en"]);
        assert_eq!(mixed.iter().map(|pick| pick.auto).collect::<Vec<_>>(), [false, true]);
        // live_chat is not a subtitle; unknown native language degrades gracefully.
        assert_eq!(
            keys_of(&sub_picks_for(&named(&[("live_chat", ""), ("en", "English")]), &[], None)),
            ["en"]
        );
        assert!(sub_picks_for(&[], &[], None).is_empty(), "nothing available, nothing requested");
    }

    #[test]
    fn auto_tracks_get_stamped_titles() {
        assert_eq!(stamped_title("Hebrew (Original)"), "Hebrew (auto-generated)");
        assert_eq!(stamped_title("English"), "English (auto-generated)");
        assert_eq!(stamped_title("English from Korean"), "English from Korean (auto-generated)");
    }

    #[test]
    fn json_lang_names_pair_each_key_with_its_display_name() {
        let json = r#"{"en": [{"url": "u", "name": "English"}], "iw-orig": [{"ext": "vtt", "name": "Hebrew (Original)"}], "bare": []}"#;
        assert_eq!(
            json_lang_names(json),
            [
                ("en".to_string(), "English".to_string()),
                ("iw-orig".to_string(), "Hebrew (Original)".to_string()),
                ("bare".to_string(), String::new()),
            ]
        );
        assert!(json_lang_names("NA").is_empty());
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
            &en_keys(),
        );
        let text: Vec<String> = argv.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
        for expected in [
            "--write-subs", "--write-auto-subs", "--embed-subs",
            "--sub-langs", "en,en-US,en-GB",
            "--sleep-subtitles", "2",
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
        let argv = video_argv("https://u", Path::new("/dl"), env, &en_keys());
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
        let langs = en_keys();

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
    fn the_batch_probe_parses_per_entry_and_groups_by_subtitle_keys() {
        // Entry 1: real en+ko. Entry 2: nothing anywhere. Entry 3: auto-only Korean video.
        let out = "1\tidA00000001\tko\t{\"en\": [{\"name\": \"English\"}], \"ko\": [{\"name\": \"Korean\"}]}\t{}\n\
                   2\tidB00000002\tNA\t{}\t{}\n\
                   3\tidC00000003\tko\t{}\t{\"ko\": [{\"name\": \"Korean\"}], \"en\": [{\"name\": \"English\"}]}\n";
        let plan = parse_batch(out);
        assert_eq!(plan[0].id, "idA00000001");
        assert_eq!(keys_of(&plan[0].picks), ["en", "ko"]);
        assert!(plan[1].picks.is_empty());
        assert_eq!(keys_of(&plan[2].picks), ["ko", "en"]);
        assert!(plan[2].picks.iter().all(|pick| pick.auto));
        let groups = group_by_langs(&plan);
        assert_eq!(groups.len(), 3, "three distinct key lists here");
        // Same keys → one group, even when the auto flags differ per entry (real en for one,
        // translated en for another) — the post-pass reads each entry's own picks.
        let twin = [
            Planned { index: "1".into(), id: "a".into(), picks: plan[0].picks.clone() },
            Planned {
                index: "9".into(),
                id: "b".into(),
                picks: plan[0]
                    .picks
                    .iter()
                    .map(|pick| Pick { auto: !pick.auto, ..pick.clone() })
                    .collect(),
            },
        ];
        let merged = group_by_langs(&twin);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].iter().map(|plan| plan.index.as_str()).collect::<Vec<_>>(), ["1", "9"]);
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
