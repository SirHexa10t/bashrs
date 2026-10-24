//! End-to-end tests of assorted non-search commands, driving the real binary.


use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Run the real `bashrs` with `args` and extra environment overrides.
fn bashrs(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bashrs"));
    cmd.args(args);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("run bashrs")
}

/// Run the real `bashrs` with `args` from `cwd`.
fn bashrs_in(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bashrs")).args(args).current_dir(cwd).output().expect("run bashrs")
}

#[test]
fn brs_configure_creates_the_config_from_the_template_and_opens_it() {
    // A private HOME, so the test can never touch the user's real ~/.bashrs.
    let home = std::env::temp_dir().join(format!("bashrs_cfg_home_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();

    // First use: the file is created from the template, then opened via $EDITOR (cat prints it).
    let out = bashrs(&["brs_configure"], &[("HOME", home.to_str().unwrap()), ("EDITOR", "cat")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let shown = String::from_utf8_lossy(&out.stdout);
    // Fields must be present with SOME boolean value; the defaults themselves are the
    // template's business (the unit test checks legality by parsing the TOML).
    assert!(
        shown.contains("[tools]")
            && shown.contains("always_bundle_languages = ")
            && shown.contains("always_bundle_utilities = "),
        "{shown}"
    );

    let file = home.join(".bashrs").join("configrs.toml");
    assert!(file.exists(), "config file must be created under $HOME/.bashrs");

    // A current-shaped file with edited VALUES is opened as-is — user settings survive. The
    // edit flips whichever value the template currently ships, so this test never cares what
    // the default is.
    let template = std::fs::read_to_string(&file).unwrap();
    let flipped = if template.contains("always_bundle_utilities = true") {
        template.replace("always_bundle_utilities = true", "always_bundle_utilities = false")
    } else {
        template.replace("always_bundle_utilities = false", "always_bundle_utilities = true")
    };
    let edited = format!("# my edited config\n{flipped}");
    std::fs::write(&file, &edited).unwrap();
    let again = bashrs(&["brs_configure"], &[("HOME", home.to_str().unwrap()), ("EDITOR", "cat")]);
    assert!(String::from_utf8_lossy(&again.stdout).contains("# my edited config"),
        "a current-shaped config must never be overwritten");

    // A file with an OUTDATED shape (missing template keys) is archived as `.old` + rewritten.
    std::fs::write(&file, "[tools]\nalways_bundle = true\n").unwrap();
    let migrated = bashrs(&["brs_configure"], &[("HOME", home.to_str().unwrap()), ("EDITOR", "cat")]);
    assert!(String::from_utf8_lossy(&migrated.stderr).contains("archived"), "{}", String::from_utf8_lossy(&migrated.stderr));
    assert!(String::from_utf8_lossy(&migrated.stdout).contains("always_bundle_languages"), "fresh template opened");
    let olds: Vec<_> = std::fs::read_dir(home.join(".bashrs")).unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".old"))
        .collect();
    assert_eq!(olds.len(), 1, "the outdated file must be archived, once");
    assert!(
        olds[0].file_name().to_string_lossy().starts_with("configrs.toml."),
        "the archive keeps the full original name: {:?}", olds[0].file_name()
    );
    assert!(std::fs::read_to_string(olds[0].path()).unwrap().contains("always_bundle = true"),
        "the user's old settings must survive inside the archive");
    let _ = std::fs::remove_dir_all(&home);
}

/// A config the user symlinked (a dotfiles repo, say) survives the outdated-shape migration: the
/// fresh template goes into the file the link points at, the link is still a link, and the old
/// settings are archived beside it in `~/.bashrs` — where the shell's merge reminder looks.
/// Renaming used to move the link itself to the archive name, leaving a regular file behind.
#[test]
fn a_symlinked_config_is_migrated_through_the_link() {
    let home = std::env::temp_dir().join(format!("bashrs_cfg_link_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".bashrs")).unwrap();
    std::fs::create_dir_all(home.join("dotfiles")).unwrap();
    let dotfile = home.join("dotfiles/configrs.toml");
    std::fs::write(&dotfile, "[tools]\nalways_bundle = true\n").unwrap();
    let link = home.join(".bashrs/configrs.toml");
    std::os::unix::fs::symlink(&dotfile, &link).unwrap();

    let out = bashrs(&["brs_configure"], &[("HOME", home.to_str().unwrap()), ("EDITOR", "cat")]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("archived"), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(link.is_symlink(), "the link must survive the migration");
    assert!(std::fs::read_to_string(&dotfile).unwrap().contains("always_bundle_languages"),
        "the fresh template goes where the link points");
    let olds: Vec<_> = std::fs::read_dir(home.join(".bashrs")).unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".old"))
        .collect();
    assert_eq!(olds.len(), 1, "archived once, in ~/.bashrs");
    assert!(!olds[0].path().is_symlink(), "the archive is a copy, not the moved link");
    assert!(std::fs::read_to_string(olds[0].path()).unwrap().contains("always_bundle = true"),
        "and it holds the old settings");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn fs_usage_count_prints_a_bare_number_and_fails_on_missing_paths() {
    let dir = std::env::temp_dir().join(format!("bashrs_usage_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("nested")).unwrap();
    std::fs::write(dir.join("one.txt"), "x").unwrap();
    std::fs::write(dir.join("nested/two.txt"), "y").unwrap();
    let out = bashrs(&["fs_usage", "--count", dir.to_str().unwrap()], &[]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "2", "recursive, files only");
    let missing = bashrs(&["fs_usage", "--count", "/no/such/dir"], &[]);
    assert!(!missing.status.success(), "a missing path must exit non-zero");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dl_page_links_finds_resolves_and_downloads_from_a_local_page() {
    // Fully offline end-to-end: curl reads file:// pages and downloads file:// links, so the
    // whole scan → resolve → fetch pipeline runs against a local fixture site.
    let base = std::env::temp_dir().join(format!("bashrs_dl_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("site/assets")).unwrap();
    std::fs::create_dir_all(base.join("out")).unwrap();
    std::fs::write(base.join("site/assets/song.mp3"), "MP3DATA").unwrap();
    std::fs::write(base.join("site/cover.png"), "PNGDATA").unwrap();
    std::fs::write(
        base.join("site/page.html"),
        r#"<html><a href="assets/song.mp3">song</a><img src="cover.png"><a href="skip.txt">no</a></html>"#,
    )
    .unwrap();
    let url = format!("file://{}/site/page.html", base.display());
    let dl = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .args(args)
            .current_dir(base.join("out"))
            .output()
            .expect("run bashrs")
    };

    let listed = dl(&["dl_page_links", &url, "mp3", ".PNG", "--list"]);
    let lines = String::from_utf8_lossy(&listed.stdout).into_owned();
    assert!(lines.contains("assets/song.mp3") && lines.contains("cover.png"), "{lines}");
    assert!(!lines.contains("skip.txt"), "unrequested types must be ignored: {lines}");
    assert!(std::fs::read_dir(base.join("out")).unwrap().next().is_none(), "--list downloads nothing");

    let run = dl(&["dl_page_links", &url, "mp3", "png"]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(std::fs::read_to_string(base.join("out/song.mp3")).unwrap(), "MP3DATA");
    assert_eq!(std::fs::read_to_string(base.join("out/cover.png")).unwrap(), "PNGDATA");

    let none = dl(&["dl_page_links", &url, "zip"]);
    assert!(!none.status.success(), "no matches at all must exit non-zero");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn media_convert_refuses_to_convert_a_file_onto_itself() {
    // The guard fires on the resolved paths alone — before ffmpeg, before any filesystem access.
    let out = bashrs(&["media_convert", "clip.mp4", "mp4"], &[]);
    assert!(!out.status.success(), "self-conversion must fail");
    assert!(String::from_utf8_lossy(&out.stderr).contains("the output is the input itself"));
}

#[test]
fn media_commands_propagate_failure_as_a_nonzero_exit() {
    // Deterministic with or without ffmpeg installed: a missing input fails either way, and the
    // command must pass that on (ffmpeg keeps warnings out of its exit status, so a clean run
    // with ignorable warnings still exits 0).
    let convert = bashrs(&["media_convert", "/no/such/clip.mp4", "/tmp/bashrs_never_written.mkv"], &[]);
    assert!(!convert.status.success(), "a failed conversion must exit non-zero");
    let metadata = bashrs(&["media_metadata", "/no/such/clip.mp4"], &[]);
    assert!(!metadata.status.success(), "an unreadable file must exit non-zero");
}

/// Whether a real ffmpeg and ffprobe answer on PATH — where a bashrs shell puts the bundled copies
/// first (`~/.bashrs/tools/bin`), the ones the commands run. Without them the media tests have
/// nothing to drive, and say so as they skip.
fn media_tools_work(test: &str) -> bool {
    let works = ["ffmpeg", "ffprobe"]
        .iter()
        .all(|tool| Command::new(tool).arg("-version").output().is_ok_and(|out| out.status.success()));
    if !works {
        eprintln!("SKIPPED {test}: no ffmpeg/ffprobe on PATH");
    }
    works
}

/// `args` run through the real ffmpeg in `dir`, quietly; panics on failure, naming `what` it made.
fn ffmpeg_makes(dir: &Path, what: &str, args: &[&str]) {
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-y"])
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    assert!(made, "could not synthesize {what}");
}

/// `args` run through the real ffprobe on `file`, quietly: what it printed.
fn ffprobe(file: &Path, args: &[&str]) -> String {
    let out = Command::new("ffprobe").args(["-v", "error"]).args(args).arg(file).output().expect("ffprobe");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// What ffprobe reads of `file`: its container's duration, and each subtitle track's cues as
/// `(start, end)`, in seconds.
fn duration_and_cues(file: &Path) -> (f64, Vec<Vec<(f64, f64)>>) {
    let duration = ffprobe(file, &["-show_entries", "format=duration", "-of", "csv=p=0"]).trim().parse().expect("a duration");
    let tracks = ffprobe(file, &["-select_streams", "s", "-show_entries", "stream=index", "-of", "csv=p=0"]).lines().count();
    let cues = (0..tracks)
        .map(|track| {
            ffprobe(file, &["-select_streams", &format!("s:{track}"), "-show_entries", "packet=pts_time,duration_time", "-of", "csv=p=0"])
                .lines()
                .filter_map(|line| {
                    let (start, length) = line.trim_end_matches(',').split_once(',')?;
                    let start: f64 = start.parse().ok()?;
                    Some((start, start + length.parse::<f64>().ok()?))
                })
                .collect()
        })
        .collect();
    (duration, cues)
}

/// A cut fits the subtitle cues to itself at both ends, on every subtitle track: one on screen at
/// the start opens the cut from its first moment instead of being lost, one running past the end
/// stops there — and the file lasts as long as its picture, not as long as the overhanging cue.
/// Every way of giving the cut, re-encoded and copied.
#[test]
fn media_trim_fits_the_subtitle_cues_to_both_ends_of_the_cut() {
    if !media_tools_work("media_trim subtitle fitting") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bashrs_trim_cues_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Cues across 10 s and 20 s, and one between: the cut points every case below uses.
    std::fs::write(
        dir.join("cues.srt"),
        "1\n00:00:05,000 --> 00:00:12,000\nacross ten\n\n2\n00:00:14,000 --> 00:00:16,000\nbetween\n\n3\n00:00:18,000 --> 00:00:28,000\nacross twenty\n",
    )
    .unwrap();
    let source = ["-f", "lavfi", "-i", "testsrc2=d=30:s=160x120:r=25", "-f", "lavfi", "-i", "sine=d=30", "-i", "cues.srt"];
    let tracks = ["-map", "0", "-map", "1", "-map", "2", "-map", "2", "-metadata:s:s:0", "language=eng", "-metadata:s:s:1", "language=por"];
    ffmpeg_makes(&dir, "clip.mkv", &[&source[..], &tracks, &["-g", "25", "clip.mkv"]].concat());
    let trim = |args: &[&str], output: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .args(["media_trim", "clip.mkv"])
            .args(args)
            .args(["--output", output])
            .current_dir(&dir)
            .output()
            .expect("run bashrs");
        assert!(out.status.success(), "media_trim {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        duration_and_cues(&dir.join(output))
    };
    let near = |got: f64, want: f64, slack: f64| (got - want).abs() <= slack;

    // A copy starts at the keyframe before the cut, its pre-roll shifting everything along — cues
    // included, so the cut's own start is where the cue across it now begins. The copy's last
    // packets may also run a frame or two past the end, as copied video and audio do.
    for (copy, overrun) in [(&[][..], 0.05), (&["--copy"][..], 0.5)] {
        let (length, tracks) = trim(&[&["10", "20"], copy].concat(), &format!("both{}.mkv", copy.len()));
        assert_eq!(tracks.len(), 2, "{copy:?}: both subtitle tracks kept: {tracks:?}");
        for cues in &tracks {
            let (first, last) = (cues[0], cues[cues.len() - 1]);
            let start = first.0;
            if copy.is_empty() {
                assert!(start <= 0.05, "re-encoded, the cut starts the file: {cues:?}");
            }
            assert!(near(first.1 - start, 2.0, 0.05), "{copy:?}: the cue across 10 s opens the cut, its last 2 s left: {cues:?}");
            assert!(near(last.1 - start, 10.0, 0.05), "{copy:?}: the cue across 20 s ends with the cut: {cues:?}");
            assert!(length - start <= 10.0 + overrun, "{copy:?}: 10→20 lasts {length} from {start}, not ~10: {cues:?}");
        }
    }
    let (length, tracks) = trim(&["20"], "start.mkv");
    assert!(near(length, 10.0, 0.05), "20→end lasts {length}: {tracks:?}");
    for cues in &tracks {
        assert_eq!(cues.len(), 1, "only the cue across 20 s: {cues:?}");
        assert!(near(cues[0].0, 0.0, 0.05) && near(cues[0].1, 8.0, 0.05), "{cues:?}");
    }
    let (length, tracks) = trim(&["--end", "15"], "end.mkv");
    assert!(near(length, 15.0, 0.05), "0→15 lasts {length}: {tracks:?}");
    for cues in &tracks {
        assert!(near(cues[cues.len() - 1].1, 15.0, 0.05), "the cue across 15 s ends with the cut: {cues:?}");
    }
    // Into a container that holds no subtitles, the cut goes ffmpeg's own way — the sound alone.
    let (length, tracks) = trim(&["10", "20"], "sound.mp3");
    assert!(near(length, 10.0, 0.1) && tracks.is_empty(), "10→20 as sound: {length} {tracks:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A trim keeps every stream of a file in its own codec — the picture re-encoded as what it was,
/// at about the rate the source spent on the stretch kept; the sound, the subtitle tracks and the
/// attachment copied — not in the container's default codecs, and not at their default quality.
/// A `--copy` keeps the same streams.
#[test]
fn media_trim_keeps_every_stream_in_its_own_codec_at_its_own_rate() {
    if !media_tools_work("media_trim in kind") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bashrs_trim_kind_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cues.srt"), "1\n00:00:01,000 --> 00:00:19,000\nall along\n").unwrap();
    std::fs::write(dir.join("font.ttf"), "not really a font").unwrap();
    // None of these is Matroska's default: it would re-encode them as h264, vorbis and ass. And
    // the picture's rate is far from mpeg4's own default (about a third of it, here), so a re-encode
    // that missed the source's rate would show.
    ffmpeg_makes(&dir, "clip.mkv", &[
        "-f", "lavfi", "-i", "testsrc2=d=20:s=640x480:r=25", "-f", "lavfi", "-i", "sine=d=20", "-i", "cues.srt",
        "-map", "0", "-map", "1", "-map", "2", "-map", "2",
        "-c:v", "mpeg4", "-b:v", "1500k", "-c:a", "ac3", "-c:s", "srt",
        "-attach", "font.ttf", "-metadata:s:t", "mimetype=application/x-truetype-font", "clip.mkv",
    ]);
    let streams = |file: &str| ffprobe(&dir.join(file), &["-show_entries", "stream=codec_type,codec_name", "-of", "csv=p=0"]);
    let video_rate = |file: &str, from: f64, until: f64| {
        let packets = ffprobe(&dir.join(file), &["-select_streams", "v:0", "-show_entries", "packet=pts_time,size", "-of", "csv=p=0"]);
        let bytes: f64 = packets
            .lines()
            .filter_map(|line| line.split_once(','))
            .filter_map(|(pts, size)| Some((pts.parse::<f64>().ok()?, size.parse::<f64>().ok()?)))
            .filter(|(pts, _)| (from..until).contains(pts))
            .map(|(_, size)| size)
            .sum();
        bytes * 8.0 / (until - from)
    };
    let source = streams("clip.mkv");
    assert_eq!(source.lines().count(), 5, "five streams to keep: {source}");
    for (args, output) in [(&["5", "15"][..], "kind.mkv"), (&["5", "15", "--copy"], "copy.mkv")] {
        let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .args(["media_trim", "clip.mkv"])
            .args(args)
            .args(["--output", output])
            .current_dir(&dir)
            .output()
            .expect("run bashrs");
        assert!(out.status.success(), "media_trim {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(streams(output), source, "{args:?}: every stream, each in its own codec");
    }
    let (kept, spent) = (video_rate("kind.mkv", 0.0, 10.0), video_rate("clip.mkv", 5.0, 15.0));
    assert!((kept / spent - 1.0).abs() < 0.25, "the picture at the source's rate: {kept:.0} for {spent:.0} bit/s");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A trim keeps a thumbnail a thumbnail: a Matroska cover comes out attached again, under its own
/// name and type — not the plain video track a remux makes of it, which players list as one more
/// video and thumbnailers ignore — beside the other attachments; a cover an earlier remux demoted
/// so comes out a cover again; an mp4's cover art stays cover art. Re-encoded, copied, or cut at
/// the end only — and the covers taken out for it are gone afterwards, whether the trim saved or
/// was refused.
#[test]
fn media_trim_keeps_a_thumbnail_a_thumbnail() {
    if !media_tools_work("media_trim thumbnail") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bashrs_trim_thumb_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("font.ttf"), "not really a font").unwrap();
    ffmpeg_makes(&dir, "cover.jpg", &["-f", "lavfi", "-i", "color=c=red:s=64x64", "-frames:v", "1", "-update", "1", "cover.jpg"]);
    let clip = ["-f", "lavfi", "-i", "testsrc2=d=10:s=160x120:r=25", "-f", "lavfi", "-i", "sine=d=10"];
    ffmpeg_makes(&dir, "covered.mkv", &[&clip[..], &[
        "-attach", "cover.jpg", "-metadata:s:t:0", "mimetype=image/jpeg", "-metadata:s:t:0", "filename=cover.jpg",
        "-attach", "font.ttf", "-metadata:s:t:1", "mimetype=application/x-truetype-font", "-metadata:s:t:1", "filename=font.ttf",
        "covered.mkv",
    ]].concat());
    // What a `-map 0 -c copy` remux does to a Matroska cover: a plain one-frame video track.
    ffmpeg_makes(&dir, "demoted.mkv", &["-i", "covered.mkv", "-map", "0", "-c", "copy", "demoted.mkv"]);
    ffmpeg_makes(&dir, "covered.mp4", &[&clip[..], &["-i", "cover.jpg", "-map", "0", "-map", "1", "-map", "2", "-c:v:1", "copy", "-disposition:v:1", "attached_pic", "covered.mp4"]].concat());
    // Each stream as `kind,cover[,filename,mimetype]`.
    let streams = |file: &str| -> Vec<String> {
        ffprobe(&dir.join(file), &["-show_entries", "stream=codec_type:stream_disposition=attached_pic:stream_tags=filename,mimetype", "-of", "csv=p=0"])
            .lines()
            .map(str::to_string)
            .collect()
    };
    assert!(streams("demoted.mkv").contains(&"video,0,cover.jpg,image/jpeg".to_string()), "the remux demoted the cover: {:?}", streams("demoted.mkv"));
    let trim = |input: &str, args: &[&str], output: &str, overwrite: bool| {
        let child = Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .args(["media_trim", input])
            .args(args)
            .args(["--output", output])
            .args(overwrite.then_some("-y"))
            .current_dir(&dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run bashrs");
        let covers = std::env::temp_dir().join(format!("bashrs_trim_covers_{}", child.id()));
        let out = child.wait_with_output().expect("bashrs ran");
        assert!(!covers.exists(), "{input} {args:?}: the covers taken out are left behind in {}", covers.display());
        out
    };
    for (input, args, output) in [
        ("covered.mkv", &["2", "8"][..], "cut.mkv"),
        ("covered.mkv", &["2", "8", "--copy"], "copied.mkv"),
        ("covered.mkv", &["--end", "6"], "ended.mkv"),
        ("demoted.mkv", &["2", "8"], "repaired.mkv"),
    ] {
        let out = trim(input, args, output, true);
        assert!(out.status.success(), "{input} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        let kept = streams(output);
        assert_eq!(kept.iter().filter(|stream| stream.starts_with("video,0")).count(), 1, "{input} {args:?}: one picture, no cover as a track: {kept:?}");
        assert!(kept.contains(&"video,1,cover.jpg,image/jpeg".to_string()), "{input} {args:?}: the cover, attached: {kept:?}");
        assert!(kept.contains(&"attachment,0,font.ttf,application/x-truetype-font".to_string()), "{input} {args:?}: the font beside it: {kept:?}");
    }
    let out = trim("covered.mp4", &["2", "8"], "cut.mp4", true);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(streams("cut.mp4").iter().filter(|stream| stream.starts_with("video,1")).count(), 1, "the mp4's cover art: {:?}", streams("cut.mp4"));
    // Refused — the output is there, and no -y — the covers taken out go all the same.
    let refused = trim("covered.mkv", &["2", "8"], "cut.mkv", false);
    assert!(!refused.status.success(), "an existing output without -y is refused");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The correction makes a picture riding as a video track the file's thumbnail, in place: a cover
/// a remux demoted is restored; a copy of a thumbnail the file already has is removed; a different
/// picture is kept as a second one, the thumbnail shown staying the one it was; an mp4's picture
/// track is marked as cover art. A file with nothing to correct is left as it is, and said so; one
/// whose container holds no thumbnail, or that is no media at all, fails — untouched. The
/// corrected files keep their permissions, and nothing is left beside them.
#[test]
fn media_correction_makes_a_picture_track_the_thumbnail() {
    if !media_tools_work("media_correction") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bashrs_correction_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    ffmpeg_makes(&dir, "cover.jpg", &["-f", "lavfi", "-i", "color=c=red:s=64x64", "-frames:v", "1", "-update", "1", "cover.jpg"]);
    ffmpeg_makes(&dir, "other.jpg", &["-f", "lavfi", "-i", "color=c=green:s=64x64", "-frames:v", "1", "-update", "1", "other.jpg"]);
    let clip = ["-f", "lavfi", "-i", "testsrc2=d=4:s=160x120:r=25", "-f", "lavfi", "-i", "sine=d=4"];
    let attach = |picture: &'static str| ["-attach", picture, "-metadata:s:t:0", "mimetype=image/jpeg", "-metadata:s:t:0", "filename=cover.jpg"];
    ffmpeg_makes(&dir, "proper.mkv", &[&clip[..], &attach("cover.jpg"), &["proper.mkv"]].concat());
    ffmpeg_makes(&dir, "plain.mkv", &[&clip[..], &["plain.mkv"]].concat());
    // What a `-map 0 -c copy` remux does to a cover — and what a fetch beside it then adds.
    ffmpeg_makes(&dir, "demoted.mkv", &["-i", "proper.mkv", "-map", "0", "-c", "copy", "demoted.mkv"]);
    ffmpeg_makes(&dir, "same.mkv", &[&["-i", "demoted.mkv", "-map", "0", "-c", "copy"][..], &attach("cover.jpg"), &["same.mkv"]].concat());
    ffmpeg_makes(&dir, "different.mkv", &[&["-i", "demoted.mkv", "-map", "0", "-c", "copy"][..], &attach("other.jpg"), &["different.mkv"]].concat());
    ffmpeg_makes(&dir, "track.mp4", &[&clip[..], &["-i", "cover.jpg", "-map", "0", "-map", "1", "-map", "2", "-c:v:1", "copy", "track.mp4"]].concat());
    let picture = &clip[..4]; // the moving picture alone, no sound
    ffmpeg_makes(&dir, "track.avi", &[picture, &["-i", "cover.jpg", "-map", "0", "-map", "1", "-c:v:0", "mpeg4", "-c:v:1", "copy", "track.avi"]].concat());
    std::fs::write(dir.join("notes.txt"), "no media at all").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.join("demoted.mkv"), std::fs::Permissions::from_mode(0o640)).unwrap();
    let rows = |file: &str| -> Vec<String> {
        ffprobe(&dir.join(file), &["-show_entries", "stream=codec_type:stream_disposition=attached_pic:stream_tags=filename,mimetype", "-of", "csv=p=0"])
            .lines()
            .map(str::to_string)
            .collect()
    };
    let before: Vec<(&str, Vec<String>, Vec<u8>)> = ["proper.mkv", "plain.mkv", "track.avi", "notes.txt"]
        .into_iter()
        .map(|file| (file, rows(file), std::fs::read(dir.join(file)).unwrap()))
        .collect();

    let files = ["demoted.mkv", "same.mkv", "different.mkv", "track.mp4", "proper.mkv", "plain.mkv", "track.avi", "notes.txt"];
    let child = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .arg("media_correction")
        .args(files)
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run bashrs");
    let covers = std::env::temp_dir().join(format!("bashrs_correction_covers_{}", child.id()));
    let out = child.wait_with_output().expect("bashrs ran");
    assert!(!covers.exists(), "the pictures taken out are left behind in {}", covers.display());
    let said = String::from_utf8_lossy(&out.stdout);
    let complained = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "two files could not be corrected: {said}{complained}");
    for line in [
        "demoted.mkv: its picture track is its thumbnail now (cover.jpg)",
        "same.mkv: a copy of its thumbnail riding as a track removed",
        "different.mkv: its picture track is its thumbnail now (cover-2.jpg)",
        "track.mp4: its picture track is its thumbnail now (cover.jpg)",
        "proper.mkv: nothing to correct — its thumbnail is in place",
        "plain.mkv: nothing to correct",
    ] {
        assert!(said.lines().any(|said| said == line), "{line:?} in: {said}");
    }
    assert!(complained.contains("track.avi: a picture rides in it as a video track, but a .avi holds no thumbnail"), "{complained}");
    assert!(complained.contains("notes.txt: ffprobe could not read it"), "{complained}");

    let cover = "video,1,cover.jpg,image/jpeg".to_string();
    let plain = |file: &str| rows(file).iter().filter(|row| row.starts_with("video,0")).count();
    for file in ["demoted.mkv", "same.mkv"] {
        assert_eq!((plain(file), rows(file).iter().filter(|row| **row == cover).count()), (1, 1), "{file}: one picture, one cover: {:?}", rows(file));
    }
    let different = rows("different.mkv");
    assert!(different.contains(&cover) && different.contains(&"video,1,cover-2.jpg,image/jpeg".to_string()), "{different:?}");
    assert_eq!(plain("different.mkv"), 1, "{different:?}");
    let shown = ffprobe(&dir.join("different.mkv"), &["-select_streams", "v", "-show_entries", "stream=index:stream_tags=filename", "-of", "csv=p=0"]);
    let first_cover = shown.lines().find(|row| row.ends_with(",cover.jpg")).and_then(|row| row.split(',').next()).expect("a cover.jpg");
    let kept = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(dir.join("different.mkv")).args(["-map", &format!("0:{first_cover}"), "-c", "copy", "-f", "image2pipe", "-"]).output().unwrap().stdout;
    assert_eq!(kept, std::fs::read(dir.join("other.jpg")).unwrap(), "the thumbnail shown is the one the file had");
    assert_eq!(rows("track.mp4").iter().filter(|row| row.starts_with("video,1")).count(), 1, "{:?}", rows("track.mp4"));
    for (file, rows_before, bytes) in &before {
        assert_eq!(&rows(file), rows_before, "{file} unchanged");
        assert_eq!(&std::fs::read(dir.join(file)).unwrap(), bytes, "{file} untouched");
    }
    let mode = std::fs::metadata(dir.join("demoted.mkv")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640, "the corrected file keeps its permissions");
    let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, ["cover.jpg", "demoted.mkv", "different.mkv", "notes.txt", "other.jpg", "plain.mkv", "proper.mkv", "same.mkv", "track.avi", "track.mp4"], "nothing beside them");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A rewrite that did not do what the correction planned is never put in the original's place: here
/// an ffmpeg stand-in whose rewrite only copies the file, the picture still a track. The original
/// is left as it was, the failure said, and nothing is left beside it.
#[test]
fn media_correction_puts_no_unchecked_rewrite_in_place() {
    if !media_tools_work("media_correction check") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bashrs_correction_check_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();
    ffmpeg_makes(&dir, "cover.jpg", &["-f", "lavfi", "-i", "color=c=red:s=64x64", "-frames:v", "1", "-update", "1", "cover.jpg"]);
    ffmpeg_makes(&dir, "proper.mkv", &[
        "-f", "lavfi", "-i", "testsrc2=d=2:s=160x120:r=25",
        "-attach", "cover.jpg", "-metadata:s:t:0", "mimetype=image/jpeg", "-metadata:s:t:0", "filename=cover.jpg", "proper.mkv",
    ]);
    ffmpeg_makes(&dir, "demoted.mkv", &["-i", "proper.mkv", "-map", "0", "-c", "copy", "demoted.mkv"]);
    let real = Command::new("sh").args(["-c", "command -v ffmpeg"]).output().expect("sh");
    let real = String::from_utf8_lossy(&real.stdout).trim().to_string();
    let stand_in = dir.join("bin/ffmpeg");
    std::fs::write(
        &stand_in,
        format!(
            "#!/bin/sh\nfor last in \"$@\"; do :; done\ncase \"$last\" in\n  *.correcting.*)\n    while [ \"$#\" -gt 0 ] && [ \"$1\" != \"-i\" ]; do shift; done\n    cp \"$2\" \"$last\"; exit 0 ;;\nesac\nexec '{real}' \"$@\"\n"
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&stand_in, std::fs::Permissions::from_mode(0o755)).unwrap();
    let before = std::fs::read(dir.join("demoted.mkv")).unwrap();
    let path = format!("{}:{}", dir.join("bin").display(), std::env::var("PATH").unwrap_or_default());
    // A HOME of its own: no bundled ffmpeg there to outrank the stand-in on PATH.
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["media_correction", "demoted.mkv"])
        .env("PATH", &path)
        .env("HOME", dir.join("home"))
        .current_dir(&dir)
        .output()
        .expect("run bashrs");
    let complained = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{complained}");
    assert!(complained.contains("demoted.mkv: the corrected file did not check out — left as it was"), "{complained}");
    assert_eq!(std::fs::read(dir.join("demoted.mkv")).unwrap(), before, "the original is left as it was");
    assert!(!dir.join("demoted.correcting.mkv").exists(), "the unchecked rewrite is not left beside it");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An AV1 trim says nothing of SVT-AV1's own: its banner and configuration, which it prints
/// whatever ffmpeg's `-v` says, are kept off the terminal. Only where the ffmpeg on PATH has it.
#[test]
fn media_trim_keeps_svt_av1_quiet() {
    if !media_tools_work("media_trim quiet AV1") {
        return;
    }
    let encoders = Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).output().expect("ffmpeg");
    if !String::from_utf8_lossy(&encoders.stdout).contains("libsvtav1") {
        eprintln!("SKIPPED media_trim quiet AV1: this ffmpeg has no libsvtav1");
        return;
    }
    let dir = std::env::temp_dir().join(format!("bashrs_trim_svt_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=d=3:s=160x120:r=25", "-c:v", "libsvtav1", "clip.mkv"])
        .env("SVT_LOG", "1")
        .current_dir(&dir)
        .status()
        .is_ok_and(|status| status.success());
    assert!(made, "could not synthesize clip.mkv");
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["media_trim", "clip.mkv", "1", "2", "--output", "cut.mkv"])
        .env_remove("SVT_LOG")
        .current_dir(&dir)
        .output()
        .expect("run bashrs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(!said.contains("Svt[info]"), "SVT-AV1's banner reached the terminal: {said}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The quality conversion copies what already fits the target container, and says so — but with
/// `--h264`, a picture not already H.264 is re-encoded, however well it would fit.
#[test]
fn media_convert_quality_copies_only_what_already_fits() {
    if !media_tools_work("media_convert_quality remux") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bashrs_quality_remux_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source = ["-f", "lavfi", "-i", "testsrc2=d=1:s=160x120:r=25", "-f", "lavfi", "-i", "sine=d=1", "-c:a", "aac"];
    ffmpeg_makes(&dir, "h264.mp4", &[&source[..], &["-c:v", "libx264", "h264.mp4"]].concat());
    ffmpeg_makes(&dir, "mpeg4.mp4", &[&source[..], &["-c:v", "mpeg4", "mpeg4.mp4"]].concat());
    let convert = |input: &str, output: &str, h264: bool| {
        let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .args(["media_convert_quality", input, output])
            .args(h264.then_some("--h264"))
            .current_dir(&dir)
            .output()
            .expect("run bashrs");
        assert!(out.status.success(), "{input} → {output}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stderr).contains("copying them bit-identically")
    };
    assert!(convert("h264.mp4", "h264.mov", true), "h264 and aac fit a .mov as they are");
    assert!(convert("mpeg4.mp4", "mpeg4.mov", false), "so do mpeg4 and aac");
    assert!(!convert("mpeg4.mp4", "mpeg4_h264.mov", true), "but --h264 re-encodes a picture that isn't h264");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Several files print side by side: the facts they share level with each other — the file names
/// first, a shared tag on one line — and a tag only one file has below them, in its own column;
/// every gap one ` | `, at the same column on every line. A fifth file is a usage error.
#[test]
fn media_metadata_lines_up_what_several_files_share() {
    if !media_tools_work("media_metadata comparison") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bashrs_metadata_cmp_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    ffmpeg_makes(&dir, "first.mka", &["-f", "lavfi", "-i", "sine=d=1", "-metadata", "title=First", "-metadata", "comment=only here", "first.mka"]);
    ffmpeg_makes(&dir, "second.mka", &["-f", "lavfi", "-i", "sine=d=2", "-metadata", "title=Second", "second.mka"]);
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["media_metadata", "first.mka", "second.mka"])
        .env("COLUMNS", "300")
        .current_dir(&dir)
        .output()
        .expect("run bashrs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let shown = console::strip_ansi_codes(&String::from_utf8_lossy(&out.stdout)).into_owned();
    let lines: Vec<&str> = shown.lines().collect();
    // Content first, whatever the padding: each line as its cells — split at the bar, which no
    // value here holds, as a row whose last cell is empty ends on a bare ` |`.
    let cells: Vec<Vec<&str>> = lines.iter().map(|line| line.split('|').map(str::trim).collect()).collect();
    assert_eq!(cells[0], ["file: first.mka", "file: second.mka"], "{shown}");
    let title = cells.iter().position(|row| row == &["title: First", "title: Second"]).unwrap_or_else(|| panic!("titles paired: {shown}"));
    let comment = cells.iter().position(|row| row == &["comment: only here", ""]).unwrap_or_else(|| panic!("the lone comment: {shown}"));
    assert!(comment > title, "the lone tag after the shared ones: {shown}");
    // Then the layout: one gap per line, in one column.
    let gaps: std::collections::BTreeSet<Option<usize>> = lines.iter().map(|line| line.find(" |")).collect();
    assert_eq!(gaps.len(), 1, "every ` | ` in one column: {shown}");
    assert!(lines.iter().all(|line| line.matches(" | ").count() + usize::from(line.ends_with(" |")) == 1), "{shown}");

    // One file is the plain report, never a table: not wrapped even where the terminal is
    // narrower than its lines.
    let one = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["media_metadata", "first.mka"])
        .env("COLUMNS", "20")
        .current_dir(&dir)
        .output()
        .expect("run bashrs");
    let alone = console::strip_ansi_codes(&String::from_utf8_lossy(&one.stdout)).into_owned();
    assert!(alone.starts_with("file: first.mka\n") && alone.contains("\ntitle: First\n"), "one file, the plain report: {alone}");
    assert!(alone.contains("\ncontainer: matroska,webm\n"), "a line wider than the terminal, whole: {alone}");
    assert!(!alone.contains(" |"), "and no columns: {alone}");

    let five = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["media_metadata", "a", "b", "c", "d", "e"])
        .output()
        .expect("run bashrs");
    assert_eq!(five.status.code(), Some(2), "a fifth file is a usage error, before ffprobe runs");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every media command that writes a file, as `(command, its arguments, the file they write)`,
/// each run from one directory of synthesized inputs — with `-y` appended, or without.
const MEDIA_WRITERS: &[(&str, &[&str], &str)] = &[
    ("media_convert", &["tone.wav", "convert.wav"], "convert.wav"),
    ("media_convert_quality", &["tone.wav", "quality.flac"], "quality.flac"),
    ("media_convert_compact", &["tone.wav", "compact.flac"], "compact.flac"),
    ("media_trim", &["tone.wav", "0.2", "--output", "trim.wav"], "trim.wav"),
    ("media_remove_vocals", &["tone.wav", "devocal.wav"], "devocal.wav"),
    ("media_hmerge_imgs", &["--output", "hmerge.png", "red.png", "blue.png"], "hmerge.png"),
    ("media_vmerge_imgs", &["--output", "vmerge.png", "red.png", "blue.png"], "vmerge.png"),
];

#[test]
fn every_command_offering_overwrite_has_a_media_writers_row() {
    // The CLI's own definition says which commands take `-y`, so none can be added without the
    // test below exercising it — and no row outlives its command.
    use clap::CommandFactory;
    let offering: std::collections::BTreeSet<String> = bashrs::cli::Cli::command()
        .get_subcommands()
        .filter(|command| command.get_arguments().any(|arg| arg.get_long() == Some("overwrite")))
        .map(|command| command.get_name().to_string())
        .collect();
    let rows = MEDIA_WRITERS.iter().map(|(command, ..)| command.to_string()).collect();
    assert_eq!(offering, rows);
}

#[test]
fn media_commands_leave_an_existing_output_alone_unless_told_to_overwrite() {
    // Each MEDIA_WRITERS row runs twice against a stale output: refused and untouched without
    // `-y`, replaced with it.
    if !media_tools_work("media overwrite") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bashrs_media_overwrite_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (source, shape, file) in [
        ("sine=d=1", &["-ac", "2"][..], "tone.wav"), // stereo, as vocal removal requires
        ("color=c=red:s=8x8", &["-frames:v", "1", "-update", "1"], "red.png"),
        ("color=c=blue:s=8x8", &["-frames:v", "1", "-update", "1"], "blue.png"),
    ] {
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", source])
            .args(shape)
            .arg(file)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        assert!(made, "could not synthesize {file}");
    }
    let run = |command: &str, args: &[&str], overwrite: bool| {
        Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .arg(command)
            .args(args)
            .args(overwrite.then_some("-y"))
            .current_dir(&dir)
            .output()
            .expect("run bashrs")
    };

    for &(command, args, output) in MEDIA_WRITERS {
        let output = dir.join(output);
        std::fs::write(&output, "stale").unwrap();
        let refused = run(command, args, false);
        assert!(!refused.status.success(), "{command} without -y must refuse");
        assert_eq!(std::fs::read(&output).unwrap(), b"stale", "{command} without -y must leave the file alone");
        let replaced = run(command, args, true);
        assert!(replaced.status.success(), "{command} -y: {}", String::from_utf8_lossy(&replaced.stderr));
        assert!(std::fs::read(&output).unwrap() != b"stale", "{command} -y must replace the file");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pro_run_forwards_every_arg_to_the_program_except_its_own_help() {
    // A minimal crate whose program echoes its argv and cwd — whatever pro_run forwards must
    // come out the other side, one argv entry each (the spaced value proves the quoting-proof
    // "$@" ride; the spaced PROJECT path proves the {dir} substitution's quoting).
    let dir = std::env::temp_dir().join(format!("bashrs pro_run {}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"argecho\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.rs"),
        "fn main() { println!(\"ARGS={:?} CWD={:?}\", std::env::args().skip(1).collect::<Vec<_>>(), std::env::current_dir().unwrap()); }",
    )
    .unwrap();
    let run_in = |cwd: &Path, args: &[&str]| -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("run bashrs");
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), all)
    };
    let run = |args: &[&str]| run_in(&dir, args);

    // Everything — flags included — lands in the program's argv…
    let (ok, out) = run(&["pro_run", "-x", "--flag", "value with space"]);
    assert!(ok, "{out}");
    assert!(out.contains(r#"ARGS=["-x", "--flag", "value with space"]"#), "{out}");
    // …except -h/--help, which stay pro_run's own…
    let (ok, help) = run(&["pro_run", "-h"]);
    assert!(ok, "{help}");
    assert!(help.contains("Usage:") && !help.contains("ARGS="), "-h is pro_run's help: {help}");
    // …unless escaped past clap with `--`, which forwards even those.
    let (ok, out) = run(&["pro_run", "--", "-h"]);
    assert!(ok, "{out}");
    assert!(out.contains(r#"ARGS=["-h"]"#), "{out}");
    // `--h2` is the shorthand for that: it forwards a leading `-h` so the PROGRAM's help prints,
    // not pro_run's. It's pro_run's own flag, so it never reaches the program's argv itself.
    let (ok, out) = run(&["pro_run", "--h2"]);
    assert!(ok, "{out}");
    assert!(out.contains(r#"ARGS=["-h"]"#), "--h2 forwards -h to the program: {out}");

    // A leading --pdir picks the project from anywhere; the rest still forwards — and the
    // PROGRAM runs in the caller's own directory, not the project's (the regression that bit:
    // a cwd-sensitive program must behave exactly as if its binary were invoked directly).
    let neutral = std::env::temp_dir().canonicalize().unwrap();
    let (ok, out) = run_in(&neutral, &["pro_run", "--pdir", dir.to_str().unwrap(), "-x"]);
    assert!(ok, "{out}");
    assert!(out.contains(r#"ARGS=["-x"]"#), "ran via --pdir from elsewhere: {out}");
    assert!(
        out.contains(&format!("CWD={:?}", neutral)),
        "the program keeps the caller's cwd, not the project's: {out}"
    );
    // After the first forwarded token, even a `--pdir` belongs to the program.
    let (ok, out) = run(&["pro_run", "foo", "--pdir", "zzz"]);
    assert!(ok, "{out}");
    assert!(out.contains(r#"ARGS=["foo", "--pdir", "zzz"]"#), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn table_fancy_is_the_framed_terminal_width_table_preset() {
    // `table_fancy` must stay exactly `table -j " | " --split-lines --space-rows '-' --emit-frame`.
    // Both are run over the same input at a pinned $COLUMNS (the width --split-lines resolves to),
    // so the comparison is deterministic and pins the preset to the invocation it stands for.
    let input = "name  role  note\nada  pioneer  wrote the first algorithm for a machine\nbob  builder  yes";
    let width = ("COLUMNS", "60");
    let fancy = bashrs(&["table_fancy", input], &[width]);
    let spelled_out = bashrs(
        &["table", input, "-j", " | ", "--split-lines", "--space-rows", "-", "--emit-frame"],
        &[width],
    );
    assert!(fancy.status.success(), "{}", String::from_utf8_lossy(&fancy.stderr));
    let shown = String::from_utf8_lossy(&fancy.stdout);
    assert_eq!(shown, String::from_utf8_lossy(&spelled_out.stdout), "preset drifted from its flags");
    // …and that shape is: framed rows, none wider than the window, wrapped where needed.
    for line in shown.lines() {
        assert!(line.starts_with('|') || line.starts_with('-'), "framed or a rule: {line:?}");
        assert!(line.chars().count() <= 60, "wider than $COLUMNS: {line:?}");
    }
    assert!(shown.lines().count() > 3, "the long row wrapped: {shown}");
}

#[test]
fn pro_test_survives_a_relative_dir_across_its_two_steps() {
    // CMake is the build-then-test toolchain: pro_test cd's per step (process-global), and a
    // RELATIVE dir once resolved the second cd against the first's result (myproj/myproj) —
    // the build ran, then "cannot enter" killed ctest. Reproduced and fixed; this pins it.
    let works = Command::new("cmake").arg("--version").output().is_ok_and(|o| o.status.success())
        && Command::new("ctest").arg("--version").output().is_ok_and(|o| o.status.success());
    if !works {
        eprintln!("SKIPPED pro_test relative-dir: no cmake/ctest on this machine");
        return;
    }
    let parent = std::env::temp_dir().join(format!("bashrs_pro_test_{}", std::process::id()));
    let project = parent.join("cmproj");
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&project).unwrap();
    // Compiler-free on purpose (LANGUAGES NONE): the double-cd defect is about directories,
    // not compilation, and this keeps the test runnable on machines without a C toolchain.
    std::fs::write(
        project.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.16)\nproject(smoke LANGUAGES NONE)\nenable_testing()\nadd_test(NAME smoke COMMAND /bin/true)\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["pro_test", "cmproj"])
        .current_dir(&parent)
        .output()
        .expect("run bashrs");
    let all =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!all.contains("cannot enter"), "the second step must re-enter the SAME dir: {all}");
    assert!(all.contains("100% tests passed"), "ctest actually ran: {all}");
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn offline_provisioning_hands_its_problems_to_the_file_compile_sh_prints_last() {
    // COMPILE.sh's contract with install-stainless, run for real with nowhere to connect to.
    let home = std::env::temp_dir().join(format!("bashrs_offline_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let problems = home.join("problems");
    std::fs::write(&problems, "").unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let (ok, said) = install_stainless_offline(&home, &path, &[OsStr::new("--problems-file"), problems.as_os_str()]);

    let written = std::fs::read_to_string(&problems).unwrap();
    assert!(ok, "offline is no failure of the compile: {said}");
    assert!(written.contains("Provisioning problems"), "{written}");
    assert!(
        written.lines().any(|line| line.contains("uv") && line.contains("could not reach GitHub")),
        "a tool lookup, with its reason: {written}"
    );
    assert!(written.contains("tools: could not create the python environment"), "{written}");
    assert!(written.lines().any(|line| line.contains("stainless/") && line.contains("git failed")), "{written}");
    // Each problem was also printed where it happened; the summary itself went to the file.
    assert!(!said.contains("Provisioning problems"), "{said}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn a_tool_is_looked_up_until_the_system_has_its_own_and_then_its_bundle_goes() {
    // Every `always_bundle_*` setting off, with an earlier compile's bundles still in place, shims
    // and all. While its shim is the only ffmpeg on PATH, that is no system copy: the bundle stays
    // and is looked up, as from any shell. Once the system has its own (a stand-in), no lookup is
    // made for it, and the bundle goes; so does uv's, a language's, once the system has uv. Two
    // bundles stay whatever the settings say: the python environment, which holds the user's
    // packages, and git-lfs, an extra the system lacks — the only copy there is.
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::temp_dir().join(format!("bashrs_system_has_it_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let tools = home.join(".bashrs/tools");
    let template =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/conf/templates/configrs.toml")).unwrap();
    let config = template
        .replace("always_bundle_utilities = true", "always_bundle_utilities = false")
        .replace("always_bundle_languages = true", "always_bundle_languages = false");
    assert_eq!(config.matches("= false").count(), template.matches("= false").count() + 2, "both settings still exist to turn off");
    let os_bin = home.join("os/bin");
    for dir in ["ffmpeg/bin", "python/bin", "uv", "git-lfs", "bin"].map(|dir| tools.join(dir)).into_iter().chain([os_bin.clone()]) {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(home.join(".bashrs/configrs.toml"), config).unwrap();
    std::fs::write(tools.join("python/pyvenv.cfg"), "home = /somewhere\n").unwrap();
    for (program, body) in [
        (tools.join("ffmpeg/bin/ffmpeg"), "#!/bin/sh\n"),
        (tools.join("ffmpeg/bin/ffprobe"), "#!/bin/sh\n"),
        (tools.join("uv/uv"), "#!/bin/sh\n"),
        (tools.join("uv/uvx"), "#!/bin/sh\n"),
        (tools.join("git-lfs/git-lfs"), "#!/bin/sh\n"),
        (tools.join("python/bin/python3"), "#!/bin/sh\necho 'Python 3.14.6'\n"),
        (os_bin.join("python3"), "#!/bin/sh\necho 'Python 3.14.6'\n"),
    ] {
        std::fs::write(&program, body).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for (name, bundled) in
        [("ffmpeg", "ffmpeg/bin/ffmpeg"), ("ffprobe", "ffmpeg/bin/ffprobe"), ("uv", "uv/uv"), ("uvx", "uv/uvx"), ("git-lfs", "git-lfs/git-lfs")]
    {
        std::os::unix::fs::symlink(Path::new("..").join(bundled), tools.join("bin").join(name)).unwrap();
    }
    std::fs::write(
        tools.join("ffmpeg/.source_url"),
        "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n9.0-latest-linux64-gpl-9.0.tar.xz",
    )
    .unwrap();
    // The inherited PATH minus every directory with an ffmpeg or a git-lfs of its own — a real
    // system copy, or the real ~/.bashrs's shim — so the only ones left are this home's bundles,
    // or the stand-ins.
    let inherited: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|dir| !dir.join("ffmpeg").exists() && !dir.join("git-lfs").exists())
        .collect();
    let path_with = |first: &[PathBuf]| std::env::join_paths(first.iter().chain(&inherited)).unwrap();
    let looked_up = |said: &str| said.lines().any(|line| line.contains("recorded ffmpeg pin") || line.contains("newer ffmpeg build"));

    let (ok, said) = install_stainless_offline(&home, &path_with(&[os_bin.clone(), tools.join("bin")]), &[]);
    assert!(ok && looked_up(&said), "the shim is the bundle, not the system's — so it is kept current: {said}");
    assert!(tools.join("ffmpeg/bin/ffmpeg").exists(), "and kept: {said}");

    for program in ["ffmpeg", "uv"] {
        std::fs::write(os_bin.join(program), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(os_bin.join(program), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (ok, said) = install_stainless_offline(&home, &path_with(&[tools.join("bin"), os_bin.clone()]), &[]);
    assert!(ok, "{said}");
    let os_ffmpeg = os_bin.join("ffmpeg");
    assert!(said.contains(&format!("tools: ffmpeg is already on this system ({}) — not bundling", os_ffmpeg.display())), "{said}");
    assert!(!looked_up(&said), "no lookup for what the system has: {said}");
    for (bundle, shim) in [("ffmpeg", "ffmpeg"), ("uv", "uv"), ("uv", "uvx")] {
        assert!(tools.join(bundle).symlink_metadata().is_err(), "the old {bundle} bundle is gone: {said}");
        assert!(tools.join("bin").join(shim).symlink_metadata().is_err(), "and its {shim} shim with it");
    }
    assert!(os_ffmpeg.exists() && os_bin.join("uv").exists(), "the system's own are never touched");
    assert!(tools.join("python/bin/python3").exists(), "the python environment stays — the user's packages: {said}");
    assert!(tools.join("git-lfs/git-lfs").exists(), "the system lacks git-lfs, so its bundle stays: {said}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn the_install_moves_the_old_user_data_to_the_new_homes() {
    // The real install-shell, over the layout from before the split, in a private HOME with no rc
    // files to wire and fish never run: what the apps keep ends up in user-appdata, the user's
    // binds profiles in user-custom, and the install says what it moved.
    let home = std::env::temp_dir().join(format!("bashrs_user_dirs_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let old = home.join(".bashrs/user-data");
    std::fs::create_dir_all(old.join("sequencer")).unwrap();
    std::fs::create_dir_all(old.join("browser_cookies/youtube")).unwrap();
    std::fs::write(old.join("sequencer/binds.toml"), "[binds]").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .arg("install-shell")
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("run bashrs");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{said}");
    let bashrs = home.join(".bashrs");
    assert!(bashrs.join("user-appdata/browser_cookies/youtube").is_dir(), "{said}");
    assert_eq!(std::fs::read_to_string(bashrs.join("user-custom/sequencer/binds.toml")).unwrap(), "[binds]", "{said}");
    assert!(old.symlink_metadata().is_err(), "the old name is gone: {said}");
    assert!(said.contains("Moved ") && said.contains("user-custom/sequencer"), "it says so: {said}");
    let _ = std::fs::remove_dir_all(&home);
}

/// `install-stainless --use-stable-carstay`, run for real in the private `home` with `path` as its
/// `PATH` and nowhere to connect to: every proxy variable aimed at a closed port, and nothing
/// routing around it — NO_PROXY goes, and so does the system git config, which could name a proxy
/// of its own. Stable mode is the one that never rewrites the project's Carstay.toml; were it ever
/// to, the file is put back before the assertion says so. Returns whether the run succeeded, and
/// everything it printed.
fn install_stainless_offline(home: &Path, path: &OsStr, extra: &[&OsStr]) -> (bool, String) {
    let carstay = Path::new(env!("CARGO_MANIFEST_DIR")).join("Carstay.toml");
    let recorded = std::fs::read(&carstay).ok();
    let dead = "http://127.0.0.1:9";
    let out = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["install-stainless", "--use-stable-carstay"])
        .args(extra)
        .env("HOME", home)
        .env("PATH", path)
        .envs(["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY", "ALL_PROXY"].map(|var| (var, dead)))
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("UV_HTTP_RETRIES", "0") // uv's retries would stretch one refused download into seconds
        .output()
        .expect("run bashrs");
    let now = std::fs::read(&carstay).ok();
    if let (Some(bytes), true) = (&recorded, now != recorded) {
        let _ = std::fs::write(&carstay, bytes);
    }
    assert_eq!(now, recorded, "the project's Carstay.toml must be left exactly as it was");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// A fresh project directory with an empty sibling to run from: whatever a `pro_*` command then
/// finds can only have come from the project named on its command line. Returns
/// `(parent, project, elsewhere)`; the test removes `parent` when it is done.
fn project_beside_an_empty_dir(test: &str) -> (PathBuf, PathBuf, PathBuf) {
    let parent = std::env::temp_dir().join(format!("bashrs_{test}_{}", std::process::id()));
    let (project, elsewhere) = (parent.join("proj"), parent.join("elsewhere"));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    (parent, project, elsewhere)
}

#[test]
fn pro_todo_reads_the_project_it_is_given_not_the_directory_it_runs_in() {
    let (parent, project, elsewhere) = project_beside_an_empty_dir("pro_todo");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("TODO.md"), "buy milk\n").unwrap();
    std::fs::write(project.join("src/main.rs"), "// TODO: wire it up\n").unwrap();
    let out = bashrs_in(&elsewhere, &["pro_todo", project.to_str().unwrap()]);
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(shown.contains("buy milk"), "the project's TODO file, printed whole: {shown}");
    assert!(shown.contains("wire it up"), "the project's TODO mention: {shown}");
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn pro_readme_shows_the_project_it_is_given_not_the_directory_it_runs_in() {
    let (parent, project, elsewhere) = project_beside_an_empty_dir("pro_readme");
    std::fs::write(project.join("README.md"), "# proj\nDoes one thing well.\n").unwrap();
    let out = bashrs_in(&elsewhere, &["pro_readme", project.to_str().unwrap()]);
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(shown.contains("Does one thing well."), "the project's README: {shown}");
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn pro_compile_builds_the_project_it_is_given_not_the_directory_it_runs_in() {
    // Make leaves a mark without any compiler: the recipe touches a file, which only a build
    // run in the named project can put there.
    if !Command::new("make").arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("SKIPPED pro_compile project dir: no make on this machine");
        return;
    }
    let (parent, project, elsewhere) = project_beside_an_empty_dir("pro_compile");
    std::fs::write(project.join("Makefile"), "all:\n\ttouch built\n").unwrap();
    let out = bashrs_in(&elsewhere, &["pro_compile", project.to_str().unwrap()]);
    let all =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(project.join("built").exists(), "the build ran in the named project: {all}");
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn pro_update_packages_acts_on_the_project_it_is_given_not_the_directory_it_runs_in() {
    // A Makefile project's update step is a note, not a package manager, so nothing is run or
    // installed — and its toolchain is detected only if the named directory is the one read.
    let (parent, project, elsewhere) = project_beside_an_empty_dir("pro_update_packages");
    std::fs::write(project.join("Makefile"), "all:\n").unwrap();
    let out = bashrs_in(&elsewhere, &["pro_update_packages", project.to_str().unwrap()]);
    let all =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(all.contains("Detected C/C++ (Make)"), "the named project's toolchain: {all}");
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn diff_prints_gits_own_diff_of_two_files_and_exits_as_git_does() {
    let dir = std::env::temp_dir().join(format!("bashrs_diff_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (old, new) = (dir.join("old.txt"), dir.join("new.txt"));
    std::fs::write(&old, "same\nlet x = 1;\n").unwrap();
    std::fs::write(&new, "same\nlet x = 2;\n").unwrap();
    let path = |file: &Path| file.to_str().unwrap().to_owned();

    // Coloured even into a pipe — and under the colour, byte for byte what git prints, with
    // git's exit code: 1, "they differ".
    let out = bashrs(&["diff", &path(&old), &path(&new)], &[]);
    let git = Command::new("git")
        .args(["diff", "--no-index", "--no-ext-diff", "--color=never", "--"])
        .args([&old, &new])
        .output()
        .expect("run git");
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(shown.contains("\x1b["), "colour is on, terminal or not: {shown}");
    let plain = console::strip_ansi_codes(&shown);
    assert!(plain.contains("\n-let x = 1;\n+let x = 2;\n"), "a real diff: {plain}");
    assert_eq!(plain.as_bytes(), git.stdout);

    // `-n` puts each line's number in both files before it: `same` is line 1 of both, the removed
    // line is 2 of the old file only, its replacement 2 of the new file only.
    let numbered = bashrs(&["diff", "-n", &path(&old), &path(&new)], &[]);
    let numbered = console::strip_ansi_codes(&String::from_utf8_lossy(&numbered.stdout)).into_owned();
    assert!(numbered.contains("\n1 1 │ same\n2   │-let x = 1;\n  2 │+let x = 2;\n"), "{numbered}");

    let same = bashrs(&["diff", &path(&old), &path(&old)], &[]);
    assert_eq!((same.status.code(), same.stdout.as_slice()), (Some(0), &b""[..]), "a match is silent");

    // A side that does not exist is git's to report — and never a match.
    let missing = bashrs(&["diff", &path(&old), &path(&dir.join("absent"))], &[]);
    let complaint = String::from_utf8_lossy(&missing.stderr);
    assert_ne!(missing.status.code(), Some(0), "{complaint}");
    assert!(complaint.contains("absent"), "git names the missing side: {complaint}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diff_reads_a_dash_side_from_stdin() {
    let dir = std::env::temp_dir().join(format!("bashrs_diff_stdin_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let new = dir.join("new.txt");
    std::fs::write(&new, "same\nlet x = 2;\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_bashrs"))
        .args(["diff", "-", new.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run bashrs");
    // Written, then dropped: the pipe closes and git reads to its end.
    child.stdin.take().unwrap().write_all(b"same\nlet x = 1;\n").unwrap();
    let out = child.wait_with_output().unwrap();
    let shown = console::strip_ansi_codes(&String::from_utf8_lossy(&out.stdout)).into_owned();
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(shown.contains("\n-let x = 1;\n+let x = 2;\n"), "stdin reached git as the old side: {shown}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diff_compares_two_directories_one_level_deep_unless_recursive() {
    let dir = std::env::temp_dir().join(format!("bashrs_diff_dirs_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (old, new) = (dir.join("a"), dir.join("b"));
    for (file, text) in [
        (old.join("f.txt"), "let x = 1;\n"), (new.join("f.txt"), "let x = 2;\n"),
        (old.join("sub/deep.txt"), "old\n"), (new.join("sub/deep.txt"), "new\n"),
        (new.join("only.txt"), "new\n"),
    ] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let path = |side: &Path| side.to_str().unwrap().to_owned();
    let shown = |out: &Output| console::strip_ansi_codes(&String::from_utf8_lossy(&out.stdout)).into_owned();

    // One level, GNU's way: git's diff for the pair of files, GNU's lines for the rest.
    let shallow = bashrs(&["diff", &path(&old), &path(&new)], &[]);
    let text = shown(&shallow);
    assert_eq!(shallow.status.code(), Some(1), "{}", String::from_utf8_lossy(&shallow.stderr));
    assert!(text.contains("\n-let x = 1;\n+let x = 2;\n"), "{text}");
    assert!(text.contains(&format!("Only in {}: only.txt\n", path(&new))), "{text}");
    assert!(text.contains(&format!("Common subdirectories: {}/sub and {}/sub\n", path(&old), path(&new))), "{text}");
    assert!(!text.contains("deep.txt"), "not walked without -r: {text}");

    // `-r`: the subdirectory is compared where it was listed.
    let deep = bashrs(&["diff", "-r", &path(&old), &path(&new)], &[]);
    let text = shown(&deep);
    assert_eq!(deep.status.code(), Some(1), "{}", String::from_utf8_lossy(&deep.stderr));
    assert!(text.contains("\n-old\n+new\n") && !text.contains("Common subdirectories"), "{text}");

    let same = bashrs(&["diff", "-r", &path(&old), &path(&old)], &[]);
    assert_eq!((same.status.code(), same.stdout.as_slice()), (Some(0), &b""[..]), "a match is silent");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `lll` and the `cd` listing read a device as the real `ls` writes it: `/dev/null`, character
/// device 1, 3 on every Linux — its numbers its size, then its date, then its name. (Taken for
/// one word, the size once pushed the date into the name's place.) In a `/dev` whose minors vary
/// in width, `ls` pads between the numbers too, which must not split them either.
#[test]
fn lll_and_the_cd_listing_read_a_device_as_ls_writes_it() {
    let plain = |out: &Output| console::strip_ansi_codes(&String::from_utf8_lossy(&out.stdout)).into_owned();
    let listing = plain(&bashrs(&["lll", "/dev"], &[]));
    let row = listing.lines().find(|row| row.ends_with("  null")).unwrap_or_else(|| panic!("no `null` row:\n{listing}"));
    let cells: Vec<&str> = row.split("  ").map(str::trim).filter(|cell| !cell.is_empty()).collect();
    let date = |cell: &str| cell.len() == 19 && cell.as_bytes()[4] == b'-' && cell.as_bytes()[10] == b'_';
    assert!(cells[5] == "1, 3" && date(cells[6]) && cells[7] == "null", "size, date, name: {row}");
    let landing = Command::new(env!("CARGO_BIN_EXE_bashrs")).arg("cd-listing").current_dir("/dev").output().expect("cd-listing");
    assert!(plain(&landing).lines().any(|name| name == "null"), "{}", plain(&landing));
}

/// `bashrs <args>` as the opacity commands see a desktop: of the variables they read, only `vars`
/// are set, and `fakes` — `(name, script)` stand-ins for the terminals' and window managers' own
/// tools, each logging the words it was run with before answering as its script says — come first
/// on PATH. Returns the run, and the log: one line per call, the tool's name and its words.
fn opacity_run(tag: &str, args: &[&str], vars: &[(&str, &str)], fakes: &[(&str, &str)]) -> (Output, String) {
    use std::os::unix::fs::PermissionsExt;
    const READ: &[&str] = &[
        "KITTY_WINDOW_ID", "ALACRITTY_WINDOW_ID", "ALACRITTY_SOCKET", "XDG_CURRENT_DESKTOP", "SWAYSOCK",
        "HYPRLAND_INSTANCE_SIGNATURE", "XDG_SESSION_TYPE", "DISPLAY", "WAYLAND_DISPLAY", "TMUX", "STY", "ZELLIJ",
        "HERDR_PANE_ID",
    ];
    let dir = std::env::temp_dir().join(format!("bashrs_opacity_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, script) in fakes {
        let tool = dir.join(name);
        std::fs::write(&tool, format!("#!/bin/sh\nprintf '%s\\n' \"${{0##*/}} $*\" >> \"$CALLS\"\n{script}\n")).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let calls = dir.join("calls");
    let path = std::env::join_paths(std::iter::once(dir.clone()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())))
        .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_bashrs"));
    command.args(args).env("PATH", path).env("CALLS", &calls);
    for var in READ {
        command.env_remove(var);
    }
    let out = command.envs(vars.iter().copied()).output().expect("run bashrs");
    let log = std::fs::read_to_string(&calls).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    (out, log)
}

/// kitty, as its remote control answers: `ls --self` with the OS window's opacity, and
/// `set-background-opacity` either taking or refusing — in kitty's own words.
const KITTY_AT: &str = r#"case "$2" in ls) echo '[{"id": 1, "background_opacity": 0.8, "tabs": []}]' ;; esac"#;
const KITTY_REFUSING: &str = r#"case "$2" in
  ls) echo '[{"id": 1, "background_opacity": 0.8, "tabs": []}]' ;;
  set-background-opacity) echo "Error: You must turn on the dynamic_background_opacity option in kitty.conf" >&2; exit 1 ;;
esac"#;

#[test]
fn the_opacity_steps_in_kittys_background_when_run_directly_in_it() {
    let (out, log) = opacity_run("kitty", &["term_opacity_up"], &[("KITTY_WINDOW_ID", "7")], &[("kitten", KITTY_AT)]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "kitty: background opacity 85%\n");
    assert_eq!(log, "kitten @ ls --self\nkitten @ set-background-opacity --match id:7 0.85\n", "read back, then set this window's");
}

#[test]
fn a_refusing_terminal_hands_the_step_to_the_window_manager_and_says_why() {
    let (out, log) = opacity_run(
        "refused",
        &["term_opacity_up"],
        &[("KITTY_WINDOW_ID", "7"), ("XDG_CURRENT_DESKTOP", "KDE")],
        &[("kitten", KITTY_REFUSING), ("busctl", "")],
    );
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{said}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "KWin: window opacity +5%\n");
    assert!(said.contains("kitty couldn't — You must turn on the dynamic_background_opacity option in kitty.conf"), "{said}");
    assert!(log.ends_with(
        "busctl --user call org.kde.kglobalaccel /component/kwin org.kde.kglobalaccel.Component invokeShortcut s Increase Opacity\n"
    ), "{log}");
}

#[test]
fn inside_a_multiplexer_the_window_manager_steps_and_the_terminal_is_left_alone() {
    let (out, log) = opacity_run(
        "tmux",
        &["term_opacity_down"],
        &[("KITTY_WINDOW_ID", "7"), ("TMUX", "/tmp/tmux-1000/default,1,0"), ("XDG_CURRENT_DESKTOP", "KDE")],
        &[("kitten", KITTY_AT), ("busctl", "")],
    );
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{said}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "KWin: window opacity -5%\n");
    assert!(!log.contains("kitten"), "kitty's possibly stale window is never addressed: {log}");
    assert!(said.contains("inside tmux, kitty's own controls are out of reach"), "{said}");
}

#[test]
fn alacritty_steps_its_window_from_the_opacity_it_reads_back() {
    let alacritty = r#"case "$2" in get-config) echo '{"window": {"opacity": 0.9, "decorations": "Full"}}' ;; esac"#;
    let (out, log) = opacity_run("alacritty", &["term_opacity_down"], &[("ALACRITTY_WINDOW_ID", "3")], &[("alacritty", alacritty)]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Alacritty: background opacity 85%\n");
    assert_eq!(log, "alacritty msg get-config\nalacritty msg config window.opacity=0.85\n");
}

#[test]
fn the_window_managers_step_the_focused_window_in_their_own_terms() {
    let (out, log) = opacity_run("sway", &["term_opacity_down"], &[("SWAYSOCK", "/run/user/1000/sway-ipc.sock")], &[("swaymsg", "")]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "sway: window opacity -5%\n", "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(log, "swaymsg opacity minus 0.05\n");

    let hyprctl = r#"case "$1" in getprop) echo 0.95 ;; dispatch) echo ok ;; esac"#;
    let (out, log) = opacity_run("hyprland", &["term_opacity_up", "10"], &[("HYPRLAND_INSTANCE_SIGNATURE", "v0.56")], &[("hyprctl", hyprctl)]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Hyprland: window opacity 100%\n", "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(log, "hyprctl getprop activewindow opacity\nhyprctl dispatch setprop activewindow opacity 1.00\n", "never past opaque");

    let (out, log) = opacity_run("kwin", &["term_opacity_down", "20"], &[("XDG_CURRENT_DESKTOP", "KDE")], &[("busctl", "")]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "KWin: window opacity -20%\n", "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(log.matches("invokeShortcut s Decrease Opacity").count(), 4, "four of KWin's 5% presses: {log}");

    // Hyprland can say no while exiting 0: only its `ok` counts as done.
    let refusing = r#"case "$1" in getprop) echo 0.9 ;; dispatch) echo "Invalid dispatcher" ;; esac"#;
    let (out, _) = opacity_run("hyprland_no", &["term_opacity_up"], &[("HYPRLAND_INSTANCE_SIGNATURE", "v0.56")], &[("hyprctl", refusing)]);
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && said.contains("Hyprland: Hyprland answered: Invalid dispatcher"), "{said}");
}

/// The floors differ by what fades: a background may go fully clear, as the text still shows; a
/// whole window stops at 5%, where KWin's own opacity actions stop, being all but lost below it.
#[test]
fn a_background_may_clear_completely_but_a_window_stops_at_five_percent() {
    let nearly_clear = r#"case "$2" in ls) echo '[{"id": 1, "background_opacity": 0.03, "tabs": []}]' ;; esac"#;
    let (out, log) = opacity_run("floor_kitty", &["term_opacity_down"], &[("KITTY_WINDOW_ID", "7")], &[("kitten", nearly_clear)]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "kitty: background opacity 0%\n", "{}", String::from_utf8_lossy(&out.stderr));
    assert!(log.ends_with("set-background-opacity --match id:7 0.00\n"), "{log}");

    let faint = r#"case "$1" in getprop) echo 0.07 ;; dispatch) echo ok ;; esac"#;
    let (out, log) = opacity_run("floor_hyprland", &["term_opacity_down"], &[("HYPRLAND_INSTANCE_SIGNATURE", "v0.56")], &[("hyprctl", faint)]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Hyprland: window opacity 5%\n", "{}", String::from_utf8_lossy(&out.stderr));
    assert!(log.ends_with("setprop activewindow opacity 0.05\n"), "{log}");
}

#[test]
fn where_nothing_can_change_the_opacity_it_fails_and_names_what_could() {
    let (out, log) = opacity_run("nothing", &["term_opacity_up"], &[], &[]);
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("nothing here can change the terminal's opacity"), "{said}");
    assert!(said.contains("kitty, Alacritty") && said.contains("KWin, sway, Hyprland"), "{said}");
    assert!(log.is_empty(), "no tool was even tried: {log}");
}

/// Compile-time guard that the fixture path stays valid if this file grows path-dependent tests.
#[allow(dead_code)]
fn _manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}
