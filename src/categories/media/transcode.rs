//! Turn one media file into another: transcode with per-container quality/compact profiles
//! (`media_convert*`), or keep one stretch of the timeline (`media_trim`). Both write a new file
//! through the shared `_run_writing` tail.

#[bashrs_macros::category(command = MediaTranscodeCommand, prefix = "media_")]
mod commands {
    use crate::support::exec::capture_stdout;
    use crate::tools;
    use clap::Args;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use crate::categories::media::streams::{_attach_argv, _attaches_covers, _cover, _streams, _take_out, Cover, Stream};
    use crate::categories::media::{_run_writing, _run_writing_then, OverwriteArgs};
    // --- media_convert (+ quality / compact profiles) ---------------------------

    /// Convert a media file to another format
    pub fn convert(args: ConvertArgs) {
        let ConvertArgs { base: ConvertBase { input, output, overwrite }, bitrate, remux } = args;
        let output = _convert_output(&input, &output);
        let mut tuning: Vec<&str> = Vec::new();
        if remux {
            tuning.extend(["-c", "copy"]);
        }
        if let Some(bitrate) = bitrate.as_deref() {
            tuning.extend(["-b:v", bitrate]);
        }
        _run_convert("media_convert", &input, &output, overwrite.yes, &tuning);
    }

    /// What every `media_convert*` command accepts — the file in, the file out, and `-y` — each
    /// adding its own tuning on top: `--bitrate`/`--remux` for `media_convert` itself, `--h264`
    /// for the profiled two.
    #[derive(Args)]
    pub struct ConvertBase {
        /// Input file
        pub input: PathBuf,
        /// Output path — or just a format (`mp4`, `.ogg`, …), converting beside the input
        pub output: PathBuf,
        #[command(flatten)]
        pub overwrite: OverwriteArgs,
    }

    #[derive(Args)]
    pub struct ConvertArgs {
        #[command(flatten)]
        pub base: ConvertBase,
        /// Cap the video bitrate (e.g. 1M, 500k) instead of ffmpeg's quality-based defaults
        #[arg(short, long)]
        pub bitrate: Option<String>,
        /// Repackage the streams into the new container without re-encoding — instant and
        /// lossless, but the target format must support the source's codecs
        #[arg(long, conflicts_with = "bitrate")]
        pub remux: bool,
    }

    /// Convert a media file to another format at maximum fidelity — copying the streams
    /// losslessly when the target container already fits them, else encoding at
    /// visually-lossless settings (H.265 video) with generous audio
    pub fn convert_quality(args: ProfiledConvertArgs) {
        let ProfiledConvertArgs { base: ConvertBase { input, output, overwrite }, h264 } = args;
        let output = _convert_output(&input, &output);
        let tuning: &[&str] = if _remuxable(&input, &output, h264) {
            eprintln!(
                "media_convert_quality: the streams already fit this container — \
                 copying them bit-identically instead of re-encoding"
            );
            &["-c", "copy"]
        } else {
            _profile_flags(&output, Profile::Quality, h264)
        };
        _run_convert("media_convert_quality", &input, &output, overwrite.yes, tuning);
    }

    /// Convert a media file to another format squeezing the visuals down hard (H.265 video);
    /// audio (if any) stays on its encoder defaults
    pub fn convert_compact(args: ProfiledConvertArgs) {
        let ProfiledConvertArgs { base: ConvertBase { input, output, overwrite }, h264 } = args;
        let output = _convert_output(&input, &output);
        let tuning = _profile_flags(&output, Profile::Compact, h264);
        _run_convert("media_convert_compact", &input, &output, overwrite.yes, tuning);
    }

    /// The arguments shared by both profiled conversions — no `--bitrate` or `--remux` here: the
    /// profile owns the quality decision, and a remux never re-encodes to begin with.
    #[derive(Args)]
    pub struct ProfiledConvertArgs {
        #[command(flatten)]
        pub base: ConvertBase,
        /// Encode video as H.264 instead of the default H.265 — bigger files, but plays
        /// everywhere (only affects the mp4/mov/m4v/mkv family)
        #[arg(long)]
        pub h264: bool,
    }

    /// The two tuning directions a profiled conversion can take.
    #[derive(Clone, Copy)]
    enum Profile {
        Quality,
        Compact,
    }

    impl Profile {
        /// This profile's flag list within a table row.
        fn pick(self, knobs: &FormatKnobs) -> &'static [&'static str] {
            match self {
                Self::Quality => knobs.quality,
                Self::Compact => knobs.compact,
            }
        }

        /// The user-facing command name, for notices.
        fn command(self) -> &'static str {
            match self {
                Self::Quality => "media_convert_quality",
                Self::Compact => "media_convert_compact",
            }
        }
    }

    /// One output family's quality-governing flags at the two extremes, plus what it can carry
    /// unchanged. ffmpeg has no universal quality arg — every encoder exposes its own scale — so
    /// this table pins, per output format (that is, per the muxer's default encoders, verified
    /// live), what "maximize" and "minimize" mean. Audio knobs ride along safely: a `-b:a`
    /// matching no stream is ignored.
    struct FormatKnobs {
        /// Output extensions this row governs (lowercase).
        exts: &'static [&'static str],
        /// Maximum fidelity: visually-lossless constant-quality visuals, generous audio.
        quality: &'static [&'static str],
        /// Minimum visual payload; audio is deliberately left on encoder defaults.
        compact: &'static [&'static str],
        /// Codecs this container carries unchanged — the stream-copy accept-list behind the
        /// quality profile's remux-first path. `"*"` marks a codec-agnostic container; empty
        /// means never stream-copy. Kept conservative: a false "no" merely re-encodes at
        /// visually-lossless settings, a false "yes" writes a broken file.
        accepts: &'static [&'static str],
    }

    /// Formats without a row aren't refused — they convert on encoder defaults, with a notice.
    const FORMAT_KNOBS: &[FormatKnobs] = &[
        // The h26x family encodes H.265 (libx265: ~half the size of H.264 at like quality; its
        // crf scale sits ~5 above x264's, so 20 ≈ visually lossless, 34 ≈ a hard squeeze). The
        // `hvc1` tag makes mp4-family files recognizable to Apple players — matroska has no such
        // tag, hence the separate mkv row, which as a container also accepts anything.
        FormatKnobs {
            exts: &["mp4", "mov", "m4v"],
            quality: &["-c:v", "libx265", "-tag:v", "hvc1", "-crf", "20", "-b:a", "256k"],
            compact: &["-c:v", "libx265", "-tag:v", "hvc1", "-crf", "34"],
            accepts: &["h264", "hevc", "mpeg4", "aac", "mp3"],
        },
        FormatKnobs {
            exts: &["mkv"],
            quality: &["-c:v", "libx265", "-crf", "20", "-b:a", "256k"],
            compact: &["-c:v", "libx265", "-crf", "34"],
            accepts: &["*"],
        },
        // webm (vp9 + opus): vp9's crf spans 0-63 and only takes effect with `-b:v 0`; the
        // container is a strict-by-spec matroska subset.
        FormatKnobs {
            exts: &["webm"],
            quality: &["-crf", "15", "-b:v", "0", "-b:a", "192k"],
            compact: &["-crf", "50", "-b:v", "0"],
            accepts: &["vp8", "vp9", "av1", "vorbis", "opus"],
        },
        // avi (mpeg4 + mp3): qscale runs 1-31, lower is better; 2 is the practical best.
        FormatKnobs {
            exts: &["avi"],
            quality: &["-q:v", "2", "-b:a", "320k"],
            compact: &["-q:v", "20"],
            accepts: &["mpeg4", "mp3"],
        },
        // still images: mjpeg shares the 1-31 qscale; webp can go genuinely lossless. png and
        // gif carry no quality dial at all (lossless / palette-bound) — listed so they're
        // recognized rather than warned about. Stream-copy into an image never makes sense.
        FormatKnobs { exts: &["jpg", "jpeg"], quality: &["-q:v", "2"], compact: &["-q:v", "15"], accepts: &[] },
        FormatKnobs { exts: &["webp"], quality: &["-lossless", "1"], compact: &["-quality", "30"], accepts: &[] },
        FormatKnobs { exts: &["png", "gif"], quality: &[], compact: &[], accepts: &[] },
        // audio-only targets — `compact` leaves audio alone by contract, hence no flags there.
        // lame's V0 (`-q:a 0`) is the transparent-VBR standard; vorbis tops out around `-q:a 7`.
        // Their accept-lists only ever match all-audio sources: any video stream in the input
        // (a cover art counts) falls back to the encode path.
        FormatKnobs { exts: &["mp3"], quality: &["-q:a", "0"], compact: &[], accepts: &["mp3"] },
        FormatKnobs { exts: &["m4a", "aac"], quality: &["-b:a", "256k"], compact: &[], accepts: &["aac"] },
        FormatKnobs { exts: &["ogg", "oga"], quality: &["-q:a", "7"], compact: &[], accepts: &["vorbis", "opus"] },
        FormatKnobs { exts: &["opus"], quality: &["-b:a", "192k"], compact: &[], accepts: &["opus"] },
        // lossless audio: nothing to tune in either direction (and pcm variants are too many to
        // enumerate — wav simply re-encodes, which for pcm is lossless anyway).
        FormatKnobs { exts: &["flac"], quality: &[], compact: &[], accepts: &["flac"] },
        FormatKnobs { exts: &["wav"], quality: &[], compact: &[], accepts: &[] },
    ];

    /// The containers whose profiles encode H.265, and the `--h264` tunings that swap them back
    /// to x264's scale (its visually-lossless / hard-squeeze marks: crf 18 / 32). The flag is a
    /// no-op for every other format — nothing else here encodes an h26x codec.
    const H265_BY_DEFAULT: &[&str] = &["mp4", "mov", "m4v", "mkv"];
    const H264_QUALITY: &[&str] = &["-c:v", "libx264", "-crf", "18", "-b:a", "256k"];
    const H264_COMPACT: &[&str] = &["-c:v", "libx264", "-crf", "32"];

    /// The `profile` tuning for `output`'s format, from [`FORMAT_KNOBS`] — or the x264 variant
    /// when `h264` asks for it on an H.265-by-default container. An unlisted format gets a notice
    /// and no flags: the conversion still runs, on the encoder's own defaults.
    fn _profile_flags(output: &Path, profile: Profile, h264: bool) -> &'static [&'static str] {
        let ext = _ext(output).unwrap_or_default();
        if h264 && H265_BY_DEFAULT.contains(&ext.as_str()) {
            return match profile {
                Profile::Quality => H264_QUALITY,
                Profile::Compact => H264_COMPACT,
            };
        }
        match FORMAT_KNOBS.iter().find(|knobs| knobs.exts.contains(&ext.as_str())) {
            Some(knobs) => profile.pick(knobs),
            None => {
                eprintln!(
                    "{}: no tuning known for `.{ext}` — converting on encoder defaults",
                    profile.command()
                );
                &[]
            }
        }
    }

    /// Whether every stream of `input` can ride into `output`'s container unchanged — the
    /// precondition for the quality profile's lossless stream copy. Conservative by construction:
    /// an unlisted container, an unprobeable input, or one stream outside the accept-list means
    /// re-encoding instead — a false "no" costs nothing perceptible, a false "yes" breaks the
    /// file. With `h264_only` (the `--h264` flag), video streams must already *be* h264: copying
    /// an hevc stream would betray an explicit compatibility request.
    fn _remuxable(input: &Path, output: &Path, h264_only: bool) -> bool {
        let ext = _ext(output).unwrap_or_default();
        let accepts = FORMAT_KNOBS
            .iter()
            .find(|knobs| knobs.exts.contains(&ext.as_str()))
            .map(|knobs| knobs.accepts)
            .unwrap_or_default();
        !accepts.is_empty()
            && _streams(input).is_some_and(|streams| {
                let codecs: Vec<String> =
                    streams.iter().filter(|stream| !stream.codec.is_empty()).map(|stream| stream.codec.clone()).collect();
                _all_accepted(&codecs, accepts)
                    && (!h264_only
                        || streams.iter().filter(|stream| stream.kind == "video").all(|stream| stream.codec == "h264"))
            })
    }

    /// Whether `codecs` all sit within `accepts` (`"*"` = a codec-agnostic container, e.g. mkv).
    /// No streams at all is a "no" — there'd be nothing to copy.
    fn _all_accepted(codecs: &[String], accepts: &[&str]) -> bool {
        !codecs.is_empty()
            && codecs
                .iter()
                .all(|codec| accepts.contains(&"*") || accepts.contains(&codec.as_str()))
    }

    /// The conversion runner: build the argv, hand off to the shared writing tail.
    fn _run_convert(command: &str, input: &Path, output: &Path, overwrite: bool, tuning: &[&str]) {
        _run_writing(command, input, output, overwrite, _convert_argv(input, output, overwrite, tuning));
    }
    /// Resolve the output argument: a bare format token (`mp4`, `.ogg` — nothing but a name,
    /// dotless once the leading dot is stripped) becomes the input's path with that extension;
    /// anything else is a path, taken as given.
    fn _convert_output(input: &Path, output: &Path) -> PathBuf {
        let Some(token) = output.to_str() else { return output.to_owned() };
        let format = token.trim_start_matches('.');
        if !format.is_empty() && !format.contains(['.', '/']) {
            input.with_extension(format)
        } else {
            output.to_owned()
        }
    }

    /// ffmpeg argv for a conversion. The container and codecs follow from the output extension,
    /// steered only by the caller's `tuning` flags — without them, ffmpeg's quality-based encoder
    /// defaults apply. A still-image output gets `-frames:v 1 -update 1`: one image, not a
    /// sequence (ffmpeg otherwise warns that the filename lacks a `%03d`-style pattern).
    fn _convert_argv(input: &Path, output: &Path, overwrite: bool, tuning: &[&str]) -> Vec<OsString> {
        let mut argv: Vec<OsString> = Vec::new();
        if overwrite {
            argv.push("-y".into());
        }
        argv.push("-i".into());
        argv.push(input.as_os_str().to_owned());
        argv.extend(tuning.iter().map(OsString::from));
        if _still_image(output) {
            argv.extend(["-frames:v", "1", "-update", "1"].map(OsString::from));
        }
        argv.push(output.as_os_str().to_owned());
        argv
    }

    /// Extensions written as a single still image (gif is deliberately absent — usually animated).
    const STILL_IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "bmp", "tiff"];

    /// Whether `path` names a still image, by extension.
    fn _still_image(path: &Path) -> bool {
        _ext(path).is_some_and(|ext| STILL_IMAGE_EXTS.contains(&ext.as_str()))
    }

    /// `path`'s extension, lowercased — the key for the format lookups.
    fn _ext(path: &Path) -> Option<String> {
        path.extension().map(|ext| ext.to_string_lossy().to_lowercase())
    }

    // --- media_trim -------------------------------------------------------------

    /// Keep one stretch of a video/audio file: from a start, up to an end, or between the two —
    /// frame-accurate (re-encoding the video in its own codec, at its own bit rate), or instant
    /// and lossless with `--copy` at keyframe precision
    pub fn trim(args: TrimArgs) {
        let TrimArgs { input, points, start, end, output, overwrite, copy } = args;
        let cut = match _resolve_cut(&points, start.as_deref(), end.as_deref()) {
            Ok(cut) => cut,
            Err(why) => {
                eprintln!("media_trim: {why}");
                // A usage error, and 2 is what clap exits with for one of its own.
                std::process::exit(2);
            }
        };
        let output = output.unwrap_or_else(|| _trimmed_output(&input));
        // Where covers are taken out to, for a container that attaches them again: the system's
        // temporary directory rather than beside either file, as the input's may be read-only.
        let covers = _attaches_covers(&output)
            .then(|| std::env::temp_dir().join(format!("bashrs_trim_covers_{}", std::process::id())));
        let mut layout = _trim_layout(&input, &output, &cut, copy, covers.as_deref());
        if let Some(kept) = &mut layout {
            _extract_covers(&input, kept);
        }
        let argv = _trim_argv(&input, &output, &cut, overwrite.yes, copy, layout.as_deref());
        _run_writing_then("media_trim", &input, &output, overwrite.yes, argv, || {
            if let Some(dir) = &covers {
                let _ = std::fs::remove_dir_all(dir);
            }
        });
    }

    #[derive(Args)]
    pub struct TrimArgs {
        /// Input video/audio file
        pub input: PathBuf,
        /// Where to cut — seconds (`90`, `1.5`) or a timestamp (`1:30`, `0:01:30.5`). One point
        /// is the start; two are the start and the end. An end on its own needs `--end`
        #[arg(value_name = "DURATION", num_args = 0..=2)]
        pub points: Vec<String>,
        /// The start point, named — the same as giving it first
        #[arg(long, value_name = "DURATION")]
        pub start: Option<String>,
        /// The end point, named — the only way to give an end without a start
        #[arg(long, value_name = "DURATION")]
        pub end: Option<String>,
        /// Output file (defaults to `<input>_trimmed.<ext>`, beside the input)
        #[arg(long)]
        pub output: Option<PathBuf>,
        #[command(flatten)]
        pub overwrite: OverwriteArgs,
        /// Copy the streams instead of re-encoding: instant and lossless, but the cut snaps to
        /// the nearest keyframe — possibly seconds off on sparse-keyframe video
        #[arg(long)]
        pub copy: bool,
    }

    /// The stretch to keep, as ffmpeg will be told it: where to seek to (`-ss`, the start as
    /// typed) and how much to keep from there (`-t`, in seconds).
    ///
    /// `-t` rather than `-to`, and the difference matters. With `-ss` before `-i` ffmpeg restarts
    /// the output's clock at the seek point, so `-to` there counts from the START — `-ss 0:05 -to
    /// 0:30` keeps thirty seconds, not twenty-five. `-t` with the difference says what was meant
    /// on every ffmpeg version; with no start there is no seek, and the end IS the length. Either
    /// way it is written in seconds, the unit the subtitle fitting ([`_subtitle_fitting`])
    /// reckons in.
    #[derive(Debug, PartialEq, Eq)]
    struct Cut {
        seek: Option<String>,
        keep: Option<String>,
    }

    /// Which point is which. Named flags claim their role outright; positionals fill whatever is
    /// left, start before end — so one bare point is a start (as `media_trim_start` was), two are
    /// start and end, and a lone end has to be said with `--end`. A point that would land on a
    /// role a flag already took is an error rather than a guess.
    fn _resolve_cut(
        points: &[String],
        start: Option<&str>,
        end: Option<&str>,
    ) -> Result<Cut, String> {
        let mut unclaimed: Vec<&str> = Vec::new();
        if start.is_none() {
            unclaimed.push("start");
        }
        if end.is_none() {
            unclaimed.push("end");
        }
        if points.len() > unclaimed.len() {
            return Err(match unclaimed.as_slice() {
                [] => "both `--start` and `--end` are given, so there is no point left to place"
                    .to_string(),
                [role] => {
                    format!("only the {role} is left to give, but {} points were", points.len())
                }
                _ => format!("at most two points — a start and an end — not {}", points.len()),
            });
        }
        let mut points = points.iter().map(String::as_str);
        let start = start.or_else(|| unclaimed.contains(&"start").then(|| points.next()).flatten());
        let end = end.or_else(|| points.next());
        if start.is_none() && end.is_none() {
            return Err("nothing to cut: give a start, an end (`--end`), or both".to_string());
        }
        // Parsed for two reasons: to refuse nonsense here with a sentence rather than let ffmpeg
        // fail on it later, and because a start AND an end need arithmetic (see `Cut`).
        let parsed = |text: Option<&str>| -> Result<Option<f64>, String> {
            text.map(|text| {
                _seconds(text).ok_or_else(|| {
                    format!(
                        "`{text}` is not a duration — seconds (`90`, `1.5`) or a timestamp \
                         (`1:30`, `0:01:30.5`)"
                    )
                })
            })
            .transpose()
        };
        let (start_at, end_at) = (parsed(start)?, parsed(end)?);
        let keep = match (start_at, end_at) {
            (Some(from), Some(to)) if to <= from => {
                return Err(format!(
                    "the end ({}) is not after the start ({})",
                    end.unwrap_or_default(),
                    start.unwrap_or_default()
                ));
            }
            (Some(from), Some(to)) => Some(_ffmpeg_seconds(to - from)),
            (None, Some(to)) => Some(_ffmpeg_seconds(to)),
            (_, None) => None,
        };
        Ok(Cut { seek: start.map(str::to_string), keep })
    }

    /// A duration in ffmpeg's own syntax, as seconds: `[[HH:]MM:]SS[.frac]`, or a bare number of
    /// seconds with an optional `s`/`ms`/`us` unit. `None` for anything else — negatives
    /// included, which cut nothing.
    fn _seconds(text: &str) -> Option<f64> {
        let text = text.trim();
        if let Some((unit, scale)) = [("ms", 1e-3), ("us", 1e-6), ("s", 1.0)]
            .into_iter()
            .find(|(unit, _)| text.ends_with(unit) && !text.contains(':'))
        {
            return _positive(text.strip_suffix(unit)?).map(|value| value * scale);
        }
        let parts: Vec<&str> = text.split(':').collect();
        if parts.len() > 3 || parts.iter().any(|part| part.is_empty()) {
            return None;
        }
        parts.iter().try_fold(0.0, |total, part| Some(total * 60.0 + _positive(part)?))
    }

    /// A non-negative decimal number, or nothing.
    fn _positive(text: &str) -> Option<f64> {
        text.parse::<f64>().ok().filter(|value| value.is_finite() && *value >= 0.0)
    }

    /// Seconds for an ffmpeg argument: millisecond precision, no trailing zeros — `25`, `24.5`,
    /// `0.9` rather than the `0.9000000000000001` that subtracting two floats leaves behind.
    fn _ffmpeg_seconds(seconds: f64) -> String {
        let text = format!("{seconds:.3}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }

    /// One stream a trim keeps: where it comes from — the seeked input (`0`), or the unseeked
    /// reread (`1`) the subtitles of a cut with a start come from, see [`_trim_argv`] — and how it
    /// is written.
    #[derive(Debug, PartialEq, Eq)]
    struct Kept {
        input: usize,
        stream: Stream,
        how: How,
    }

    /// An encoder's own options, as `(option, value)` pairs — each written `-{option}:{stream}
    /// {value}`, for the one output stream it encodes.
    type EncoderSettings = &'static [(&'static str, &'static str)];

    /// How a kept stream is written.
    #[derive(Debug, PartialEq, Eq)]
    enum How {
        /// Bit for bit.
        Copy,
        /// Re-encoded in its own codec ([`IN_KIND`]), spending `rate` bits per second — the
        /// source's own over the cut, where it could be read; else at the encoder's default.
        InKind { encoder: &'static str, settings: EncoderSettings, rate: Option<u64> },
        /// Re-encoded to the sample in its own lossless codec, which loses nothing.
        Lossless,
        /// Re-encoded on the container's default encoder, no in-kind one being known for it.
        Default,
        /// A cover picture taken out of the input beforehand ([`_extract_covers`]) and attached to
        /// the output again — not mapped, which would make a video track of it (the Matroska
        /// rule, `streams::_attaches_covers`).
        Reattach(Cover),
    }

    /// The encoder writing each video codec back in kind as a trim re-encodes it, and the settings
    /// that keep it quick — each tried on 30 s of a 1080p24 AV1 video at that stretch's own rate,
    /// 609 kbit/s. SVT-AV1 at preset 8 ran a little faster than x264 at its default and landed on
    /// 599 kbit/s; preset 10 halved the time but undershot, to 469, and SVT warns of artifacts
    /// there. libvpx-vp9 (good, cpu-used 4, row threads) and x265 (medium) took about three times
    /// as long, at 702 and 633; x264's one-pass rate control overshot, to 858. A codec missing
    /// here is re-encoded on the container's default instead.
    const IN_KIND: &[(&str, &str, EncoderSettings)] = &[
        ("av1", "libsvtav1", &[("preset", "8")]),
        ("vp9", "libvpx-vp9", &[("deadline", "good"), ("cpu-used", "4"), ("row-mt", "1")]),
        ("hevc", "libx265", &[("preset", "medium"), ("x265-params", "log-level=error")]),
        ("h264", "libx264", &[]),
        ("vp8", "libvpx", &[]),
        ("mpeg4", "mpeg4", &[]),
    ];

    /// Whether an audio codec is lossless: re-encoding it costs nothing, and the cut then falls
    /// on the very sample asked for.
    fn _lossless_audio(codec: &str) -> bool {
        codec.starts_with("pcm_") || ["flac", "alac", "wavpack", "tta"].contains(&codec)
    }

    /// What the trim keeps of `input`, stream by stream ([`_keep_plan`]), the video's rates read
    /// from the source ([`_kept_bit_rate`]), its covers to be taken out to `covers` where the
    /// output attaches them — or `None`, leaving the choice to ffmpeg, where the output is another
    /// container than the input's, which may not hold all it does (changing containers is
    /// `media_convert`'s job), or where ffprobe cannot read `input`. Says so where a video goes on
    /// the container's default, or at its encoder's default quality.
    fn _trim_layout(input: &Path, output: &Path, cut: &Cut, copy: bool, covers: Option<&Path>) -> Option<Vec<Kept>> {
        if _ext(input) != _ext(output) {
            return None;
        }
        let plan = _keep_plan(&_streams(input)?, copy, cut.seek.is_some(), covers, |stream| _kept_bit_rate(input, stream, cut));
        for kept in &plan {
            match kept.how {
                How::Default => eprintln!(
                    "media_trim: no encoder known to write {} back in kind — the container's default re-encodes it",
                    kept.stream.codec
                ),
                How::InKind { rate: None, .. } => eprintln!(
                    "media_trim: {}'s bit rate could not be read off the source — re-encoding at its encoder's default quality",
                    kept.stream.codec
                ),
                _ => {}
            }
        }
        Some(plan)
    }

    /// Take each cover the plan attaches again out of `input`, to the file it names. One that will
    /// not come out stays a mapped stream instead, demoted to a plain video track but kept, and is
    /// said so.
    fn _extract_covers(input: &Path, plan: &mut [Kept]) {
        for kept in plan.iter_mut() {
            let How::Reattach(cover) = &kept.how else { continue };
            if !_take_out(input, kept.stream.index, cover) {
                eprintln!(
                    "media_trim: the cover {} would not come out to be attached again — kept as a plain video track",
                    cover.filename
                );
                kept.how = How::Copy;
            }
        }
    }

    /// What a trim keeps of `streams`, in their order: every picture, sound, subtitle and
    /// attachment — and none of the data tracks (an mp4's chapter and timecode tracks, which its
    /// chapters and metadata remake, or nothing can remux). Where `covers` names a directory, the
    /// output attaches its covers (the Matroska rule): each is taken out there, to be attached
    /// again — in an image type Matroska keeps as a cover, else copied as it came.
    /// `copy` copies the rest. Otherwise the video is re-encoded in kind, at the rate `rate` reads
    /// for its stream; lossless sound is re-encoded, to the sample; lossy sound, cover pictures,
    /// subtitles and attachments are copied — subtitles from the reread when the cut has a start
    /// (`seek`).
    fn _keep_plan(
        streams: &[Stream],
        copy: bool,
        seek: bool,
        covers: Option<&Path>,
        rate: impl Fn(usize) -> Option<u64>,
    ) -> Vec<Kept> {
        streams
            .iter()
            .filter_map(|stream| {
                let how = if let Some(cover) = covers.filter(|_| stream.cover).and_then(|dir| _cover(stream, dir)) {
                    How::Reattach(cover)
                } else {
                    match stream.kind.as_str() {
                        "video" | "audio" | "subtitle" | "attachment" if copy => How::Copy,
                        "video" if !stream.cover => match IN_KIND.iter().find(|(codec, ..)| *codec == stream.codec) {
                            Some(&(_, encoder, settings)) => How::InKind { encoder, settings, rate: rate(stream.index) },
                            None => How::Default,
                        },
                        "audio" if _lossless_audio(&stream.codec) => How::Lossless,
                        "video" | "audio" | "subtitle" | "attachment" => How::Copy,
                        _ => return None,
                    }
                };
                let input = usize::from(seek && stream.kind == "subtitle");
                Some(Kept { input, stream: stream.clone(), how })
            })
            .collect()
    }

    /// The rate the source spends on stream `stream` over the cut, in bits per second — its
    /// packets' bytes in the kept stretch, read without decoding, so the cut weighs what that
    /// stretch did in the source. `None` where ffprobe reads no packets there.
    fn _kept_bit_rate(input: &Path, stream: usize, cut: &Cut) -> Option<u64> {
        let from = cut.seek.as_deref().and_then(_seconds).unwrap_or(0.0);
        let keep = cut.keep.as_deref().and_then(|keep| keep.parse::<f64>().ok());
        let interval = match keep {
            Some(keep) => format!("{from}%+{keep}"),
            None => format!("{from}%"),
        };
        let stream = stream.to_string();
        let mut argv: Vec<OsString> = [
            "-v", "error", "-select_streams", &stream, "-read_intervals", &interval,
            "-show_entries", "packet=pts_time,duration_time,size", "-of", "csv=p=0",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        argv.push(input.as_os_str().to_owned());
        _bit_rate_of(&capture_stdout(tools::resolve("ffprobe"), argv)?, from, keep)
    }

    /// The bit rate of the packets ffprobe lists in `csv` (`pts_time,duration_time,size` rows)
    /// that start within `from` and `from + keep` — every one from `from` on, without a `keep`:
    /// the reading starts at the keyframe before `from`, whose packets are no part of the cut.
    fn _bit_rate_of(csv: &str, from: f64, keep: Option<f64>) -> Option<u64> {
        let until = keep.map_or(f64::INFINITY, |keep| from + keep);
        let (mut bytes, mut first, mut last) = (0u64, f64::INFINITY, f64::NEG_INFINITY);
        for row in csv.lines() {
            let mut fields = row.split(',');
            let (Some(pts), Some(duration), Some(size)) = (fields.next(), fields.next(), fields.next()) else { continue };
            let (Ok(pts), Ok(size)) = (pts.parse::<f64>(), size.parse::<u64>()) else { continue };
            if pts < from || pts >= until {
                continue;
            }
            bytes += size;
            first = first.min(pts);
            last = last.max(pts + duration.parse::<f64>().unwrap_or(0.0));
        }
        let span = last - first;
        (bytes > 0 && span > 0.0).then(|| (bytes as f64 * 8.0 / span).round() as u64)
    }

    /// ffmpeg argv for the trim. `-ss` sits *before* `-i`: ffmpeg then seeks instead of decoding
    /// through the skipped part — and when re-encoding it is still frame-accurate, decoding
    /// forward from the preceding keyframe and discarding up to the requested point. `-t` is an
    /// output option and sits after the input, like `-c copy`.
    ///
    /// With a `layout` ([`_trim_layout`]), each kept stream is mapped and written as it says. A
    /// cut with a start then reads the input a second time, unseeked and shifted back by the
    /// start, for the subtitles: the seek lands past the cue still on screen there — it began
    /// earlier — which the reread still has. A re-encode drops what the copied sound carries from
    /// before the start (the seek's keyframe lead-in, which the re-encoded picture does not
    /// have); a `--copy` keeps it, as its picture keeps the lead-in. A cover the output attaches
    /// again ([`How::Reattach`]) is attached after the mapped streams, under the name and type it
    /// carried. Without a layout ffmpeg picks the streams, and default encoders, itself. Wherever a cut can leave a subtitle cue
    /// overhanging it — at an end, or a start read again — the cues are fitted to it
    /// ([`_subtitle_fitting`]).
    fn _trim_argv(
        input: &Path,
        output: &Path,
        cut: &Cut,
        overwrite: bool,
        copy: bool,
        layout: Option<&[Kept]>,
    ) -> Vec<OsString> {
        let mut argv: Vec<OsString> = Vec::new();
        if overwrite {
            argv.push("-y".into());
        }
        if let Some(seek) = &cut.seek {
            argv.extend(["-ss", seek].map(OsString::from));
        }
        argv.push("-i".into());
        argv.push(input.as_os_str().to_owned());
        let reread = layout.is_some_and(|kept| kept.iter().any(|kept| kept.input == 1));
        if let (true, Some(seek)) = (reread, &cut.seek) {
            argv.extend(["-itsoffset".into(), format!("-{seek}").into(), "-i".into(), input.as_os_str().to_owned()]);
        }
        match layout {
            Some(kept) => {
                let mapped: Vec<&Kept> = kept.iter().filter(|kept| !matches!(kept.how, How::Reattach(_))).collect();
                for (at, kept) in mapped.iter().enumerate() {
                    argv.extend(["-map".to_string(), format!("{}:{}", kept.input, kept.stream.index)].map(OsString::from));
                    argv.extend(_written_as(at, kept).into_iter().map(OsString::from));
                }
                let covers: Vec<&Cover> =
                    kept.iter().filter_map(|kept| if let How::Reattach(cover) = &kept.how { Some(cover) } else { None }).collect();
                let attachments = mapped.iter().filter(|kept| kept.stream.kind == "attachment").count();
                argv.extend(_attach_argv(&covers, attachments));
            }
            None if copy => argv.extend(["-c", "copy"].map(OsString::from)),
            None => {}
        }
        if let Some(keep) = &cut.keep {
            argv.extend(["-t", keep].map(OsString::from));
        }
        if layout.is_some() && !copy && cut.seek.is_some() {
            argv.extend(["-bsf:a", r"noise=drop='lt(pts\,0)'"].map(OsString::from));
        }
        if reread || cut.keep.is_some() {
            argv.push("-bsf:s".into());
            argv.push(_subtitle_fitting(cut.keep.as_deref()).into());
        }
        argv.push(output.as_os_str().to_owned());
        argv
    }

    /// The options writing `kept` as output stream `at`, as its [`How`] says.
    fn _written_as(at: usize, kept: &Kept) -> Vec<String> {
        match &kept.how {
            How::Copy => vec![format!("-c:{at}"), "copy".to_string()],
            How::InKind { encoder, settings, rate } => {
                let mut options = vec![format!("-c:{at}"), encoder.to_string()];
                for (option, value) in *settings {
                    options.extend([format!("-{option}:{at}"), value.to_string()]);
                }
                if let Some(rate) = rate {
                    options.extend([format!("-b:{at}"), rate.to_string()]);
                }
                options
            }
            How::Lossless => vec![format!("-c:{at}"), kept.stream.codec.clone()],
            How::Default | How::Reattach(_) => Vec::new(),
        }
    }

    /// The bitstream filters fitting the subtitle cues to the cut, `keep` seconds long if it ends.
    ///
    /// A cue straddling a cut point is kept whole by ffmpeg — and a Matroska file lasts as long as
    /// the last thing in it ends, so one overhanging the end runs the player's timeline past the
    /// picture, into black (YouTube's rolling auto-captions last until the next one starts:
    /// almost any cut lands inside one). The filters run on the cues' own clock, the cut's start
    /// at 0: `noise` drops those that ended before it (only a reread delivers any), and `setts`
    /// moves an earlier start up to 0 and an end beyond `keep` back to it, the duration following
    /// suit.
    fn _subtitle_fitting(keep: Option<&str>) -> String {
        let end = match keep {
            Some(keep) => format!(r"min(PTS+DURATION\,{keep}/TB)"),
            None => "PTS+DURATION".to_string(),
        };
        format!(r"noise=drop='lte(pts+duration\,0)',setts=pts='max(PTS\,0)':dts='max(DTS\,0)':duration='{end}-max(PTS\,0)'")
    }

    /// The default trim output: the input's name with a `_trimmed` mark, in the same directory.
    fn _trimmed_output(input: &Path) -> PathBuf {
        let stem = input.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let name = match input.extension() {
            Some(ext) => format!("{stem}_trimmed.{}", ext.to_string_lossy()),
            None => format!("{stem}_trimmed"),
        };
        input.with_file_name(name)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::categories::media::streams::_parse_streams;
        use crate::categories::media::strs;

        #[test]
        fn convert_argv_is_bare_by_default_so_ffmpegs_codec_defaults_apply() {
            // No forced bitrate: the encoders' quality-based defaults beat a blanket cap.
            let argv = strs(&_convert_argv(Path::new("in.mov"), Path::new("out.mp4"), false, &[]));
            assert_eq!(argv, ["-i", "in.mov", "out.mp4"]);
        }

        #[test]
        fn convert_argv_splices_tuning_between_input_and_output() {
            let argv =
                strs(&_convert_argv(Path::new("in.mov"), Path::new("out.mp4"), true, &["-b:v", "2M"]));
            assert_eq!(argv, ["-y", "-i", "in.mov", "-b:v", "2M", "out.mp4"]);
        }

        #[test]
        fn convert_argv_writes_a_still_image_not_a_sequence() {
            // Case-insensitive extension match; video outputs must stay free of image2 options.
            let argv = strs(&_convert_argv(Path::new("clip.mp4"), Path::new("shot.PNG"), false, &[]));
            assert_eq!(argv, ["-i", "clip.mp4", "-frames:v", "1", "-update", "1", "shot.PNG"]);
            assert!(!strs(&_convert_argv(Path::new("a.mov"), Path::new("b.mkv"), false, &[]))
                .contains(&"-update".to_string()));
        }

        #[test]
        fn quality_profile_maximizes_visuals_and_audio() {
            assert_eq!(
                _profile_flags(Path::new("out.mkv"), Profile::Quality, false),
                ["-c:v", "libx265", "-crf", "20", "-b:a", "256k"]
            );
            // vp9 honors -crf only alongside -b:v 0.
            assert_eq!(
                _profile_flags(Path::new("out.webm"), Profile::Quality, false),
                ["-crf", "15", "-b:v", "0", "-b:a", "192k"]
            );
            assert_eq!(_profile_flags(Path::new("pic.webp"), Profile::Quality, false), ["-lossless", "1"]);
            assert_eq!(_profile_flags(Path::new("song.mp3"), Profile::Quality, false), ["-q:a", "0"]);
        }

        #[test]
        fn compact_profile_squeezes_visuals_but_never_touches_audio() {
            assert_eq!(
                _profile_flags(Path::new("out.mp4"), Profile::Compact, false),
                ["-c:v", "libx265", "-tag:v", "hvc1", "-crf", "34"]
            );
            assert_eq!(_profile_flags(Path::new("pic.jpg"), Profile::Compact, false), ["-q:v", "15"]);
            for compact in FORMAT_KNOBS.iter().map(|knobs| knobs.compact).chain([H264_COMPACT]) {
                assert!(
                    !compact.iter().any(|flag| flag.ends_with(":a")),
                    "compact must leave audio on encoder defaults: {compact:?}"
                );
            }
        }

        #[test]
        fn h265_is_the_default_and_the_h264_flag_swaps_it_or_noops() {
            for ext in H265_BY_DEFAULT {
                let flags = _profile_flags(Path::new(&format!("x.{ext}")), Profile::Quality, false);
                assert!(flags.windows(2).any(|pair| pair == ["-c:v", "libx265"]), "{ext}: {flags:?}");
            }
            assert_eq!(
                _profile_flags(Path::new("a.mp4"), Profile::Quality, true),
                ["-c:v", "libx264", "-crf", "18", "-b:a", "256k"]
            );
            assert_eq!(
                _profile_flags(Path::new("a.mkv"), Profile::Compact, true),
                ["-c:v", "libx264", "-crf", "32"]
            );
            // Formats that never encode an h26x codec ignore the flag entirely.
            assert_eq!(
                _profile_flags(Path::new("a.webm"), Profile::Quality, true),
                _profile_flags(Path::new("a.webm"), Profile::Quality, false)
            );
            assert_eq!(_profile_flags(Path::new("p.jpg"), Profile::Compact, true), ["-q:v", "15"]);
        }

        #[test]
        fn profile_lookup_is_case_insensitive_and_lossless_formats_have_no_knobs() {
            assert_eq!(
                _profile_flags(Path::new("A.MP4"), Profile::Quality, false),
                ["-c:v", "libx265", "-tag:v", "hvc1", "-crf", "20", "-b:a", "256k"]
            );
            for ext in ["png", "gif", "flac", "wav"] {
                let path = format!("x.{ext}");
                assert!(_profile_flags(Path::new(&path), Profile::Quality, false).is_empty(), "{ext}");
                assert!(_profile_flags(Path::new(&path), Profile::Compact, false).is_empty(), "{ext}");
            }
        }

        #[test]
        fn unknown_formats_fall_back_to_encoder_defaults_not_a_refusal() {
            assert!(_profile_flags(Path::new("x.xyz"), Profile::Quality, false).is_empty());
            assert!(_profile_flags(Path::new("noext"), Profile::Compact, false).is_empty());
        }

        /// The command line as clap sees it: every conversion takes the same input, output and `-y`
        /// ([`ConvertBase`]) and adds its own tuning on top — `--bitrate` or `--remux` (never both)
        /// for `media_convert`, `--h264` for the profiled two, whose profile owns the rest.
        #[test]
        fn conversions_share_one_base_and_each_adds_its_own_tuning() {
            #[derive(clap::Parser)]
            struct Plain {
                #[command(flatten)]
                convert: ConvertArgs,
            }
            #[derive(clap::Parser)]
            struct Profiled {
                #[command(flatten)]
                convert: ProfiledConvertArgs,
            }
            use clap::{error::ErrorKind, Parser};
            let plain = |argv: &[&str]| Plain::try_parse_from(std::iter::once("media_convert").chain(argv.iter().copied()));
            let profiled =
                |argv: &[&str]| Profiled::try_parse_from(std::iter::once("media_convert_quality").chain(argv.iter().copied()));

            let capped = plain(&["in.mov", "mp4", "-y", "-b", "1M"]).expect("capped").convert;
            assert_eq!(capped.base.input, PathBuf::from("in.mov"));
            assert_eq!(capped.base.output, PathBuf::from("mp4"));
            assert!(capped.base.overwrite.yes && !capped.remux && capped.bitrate.as_deref() == Some("1M"));
            let both = plain(&["in.mov", "mp4", "--remux", "-b", "1M"]).err().map(|err| err.kind());
            assert_eq!(both, Some(ErrorKind::ArgumentConflict), "a remux re-encodes nothing to cap");

            let h264 = profiled(&["in.mov", "out.mp4", "--h264"]).expect("h264").convert;
            assert!(h264.h264 && !h264.base.overwrite.yes);
            for tuning in [&["--bitrate", "1M"][..], &["--remux"]] {
                let refused = profiled(&[&["in.mov", "out.mp4"][..], tuning].concat()).err().map(|err| err.kind());
                assert_eq!(refused, Some(ErrorKind::UnknownArgument), "{tuning:?} is the profile's call");
            }
        }

        fn cut(seek: Option<&str>, keep: Option<&str>) -> Cut {
            Cut { seek: seek.map(str::to_string), keep: keep.map(str::to_string) }
        }

        fn resolve(points: &[&str], start: Option<&str>, end: Option<&str>) -> Result<Cut, String> {
            let points: Vec<String> = points.iter().map(|p| (*p).to_string()).collect();
            _resolve_cut(&points, start, end)
        }

        /// One bare point is the start — exactly what `media_trim_start` did — and two are the
        /// start and the end, in that order.
        #[test]
        fn bare_points_are_start_then_end() {
            assert_eq!(resolve(&["1.2"], None, None), Ok(cut(Some("1.2"), None)));
            assert_eq!(resolve(&["0:05", "0:30"], None, None), Ok(cut(Some("0:05"), Some("25"))));
        }

        /// Named flags claim their role; a bare point then fills whatever role is left. An end on
        /// its own is only sayable as `--end`.
        #[test]
        fn named_points_claim_their_role_and_bare_ones_fill_what_is_left() {
            assert_eq!(resolve(&[], None, Some("0:30")), Ok(cut(None, Some("30"))), "end alone, as the length in seconds");
            assert_eq!(resolve(&[], Some("0:05"), None), Ok(cut(Some("0:05"), None)), "start alone, named");
            assert_eq!(resolve(&[], Some("0:05"), Some("0:30")), Ok(cut(Some("0:05"), Some("25"))));
            assert_eq!(resolve(&["0:05"], None, Some("0:30")), Ok(cut(Some("0:05"), Some("25"))), "the bare one is the start");
            assert_eq!(resolve(&["0:30"], Some("0:05"), None), Ok(cut(Some("0:05"), Some("25"))), "the bare one is the end");
        }

        /// Every way of asking for nothing, too much, or nonsense is refused with a sentence,
        /// before ffmpeg is anywhere near it.
        #[test]
        fn a_cut_that_cannot_be_meant_is_refused_with_a_reason() {
            assert!(resolve(&[], None, None).unwrap_err().starts_with("nothing to cut"));
            assert!(resolve(&["1", "2", "3"], None, None).unwrap_err().starts_with("at most two points"));
            assert!(resolve(&["1"], Some("0"), Some("2")).unwrap_err().contains("no point left"));
            assert!(resolve(&["1", "2"], Some("0"), None).unwrap_err().starts_with("only the end is left"));
            assert!(resolve(&["soon"], None, None).unwrap_err().contains("`soon` is not a duration"));
            assert_eq!(
                resolve(&["0:30", "0:05"], None, None).unwrap_err(),
                "the end (0:05) is not after the start (0:30)"
            );
            assert!(resolve(&["5", "5"], None, None).is_err(), "an empty stretch is nothing to keep");
        }

        /// ffmpeg's own time syntax, both shapes, and the things that are not a time.
        #[test]
        fn durations_read_as_ffmpeg_reads_them() {
            assert_eq!(_seconds("90"), Some(90.0));
            assert_eq!(_seconds("1.5"), Some(1.5));
            assert_eq!(_seconds("1:30"), Some(90.0));
            assert_eq!(_seconds("0:01:30.5"), Some(90.5));
            assert_eq!(_seconds("3s"), Some(3.0));
            assert_eq!(_seconds("250ms"), Some(0.25));
            assert_eq!(_seconds("500us"), Some(0.0005));
            for not_a_time in ["-1", "abc", "1:2:3:4", "1::2", "", "inf", "1.5x"] {
                assert_eq!(_seconds(not_a_time), None, "{not_a_time:?}");
            }
        }

        /// The length ffmpeg is handed is the difference, written without float residue.
        #[test]
        fn the_kept_length_is_the_difference_written_plainly() {
            assert_eq!(_ffmpeg_seconds(25.0), "25");
            assert_eq!(_ffmpeg_seconds(24.5), "24.5");
            assert_eq!(_ffmpeg_seconds(1.1 - 0.2), "0.9", "not 0.9000000000000001");
            assert_eq!(resolve(&["0:00:05.5", "1:00"], None, None), Ok(cut(Some("0:00:05.5"), Some("54.5"))));
        }

        /// `-ss` seeks before the input; `-t` limits the output after it. With `-ss` before `-i`,
        /// `-to` would count from the seek point, which is why `-t` carries the difference. Left
        /// to ffmpeg's own stream choice — no layout — a cut with an end fits the subtitle cues to
        /// it, and a start alone leaves them as ffmpeg shifted them.
        #[test]
        fn trim_argv_seeks_before_the_input_and_limits_after_it() {
            let argv = |cut, overwrite, copy| strs(&_trim_argv(Path::new("in.mp4"), Path::new("out.mp4"), &cut, overwrite, copy, None));
            let fitting = |keep| _subtitle_fitting(Some(keep));
            assert_eq!(argv(cut(Some("1.2"), None), false, false), ["-ss", "1.2", "-i", "in.mp4", "out.mp4"]);
            assert_eq!(argv(cut(None, Some("30")), false, false), ["-i", "in.mp4", "-t", "30", "-bsf:s", &fitting("30"), "out.mp4"]);
            assert_eq!(
                argv(cut(Some("0:05"), Some("25")), true, true),
                ["-y", "-ss", "0:05", "-i", "in.mp4", "-c", "copy", "-t", "25", "-bsf:s", &fitting("25"), "out.mp4"]
            );
        }

        /// A YouTube download's streams, as ffprobe lists them in JSON: an AV1 picture, Opus sound,
        /// two subtitle tracks, a cover picture (attached as `cover.jpg`), a font, and a data track.
        const DOWNLOAD: &str = r#"{"streams": [
            {"index": 0, "codec_name": "av1", "codec_type": "video", "disposition": {"attached_pic": 0}},
            {"index": 1, "codec_name": "opus", "codec_type": "audio", "disposition": {"attached_pic": 0}},
            {"index": 2, "codec_name": "webvtt", "codec_type": "subtitle", "disposition": {"attached_pic": 0}},
            {"index": 3, "codec_name": "webvtt", "codec_type": "subtitle", "disposition": {"attached_pic": 0}},
            {"index": 4, "codec_name": "mjpeg", "codec_type": "video", "disposition": {"attached_pic": 1},
             "tags": {"filename": "cover.jpg", "mimetype": "image/jpeg"}},
            {"index": 5, "codec_type": "attachment", "disposition": {"attached_pic": 0},
             "tags": {"filename": "font.ttf", "mimetype": "application/x-truetype-font"}},
            {"index": 6, "codec_name": "bin_data", "codec_type": "data", "disposition": {"attached_pic": 0}}
        ]}"#;

        fn stream(index: usize, kind: &str, codec: &str, cover: bool) -> Stream {
            Stream { index, kind: kind.to_string(), codec: codec.to_string(), attached: cover, cover, filename: None, mimetype: None }
        }

        fn kept(input: usize, stream: Stream, how: How) -> Kept {
            Kept { input, stream, how }
        }

        /// Re-encoding with a start: the picture in kind at its measured rate, the sound, both
        /// subtitle tracks (from the reread), the cover and the font copied — the data track left
        /// out. `--copy` copies the same streams, the subtitles still from the reread. Into a
        /// container that attaches covers, the cover is taken out to be attached again instead.
        #[test]
        fn a_trim_keeps_every_stream_but_data_each_written_its_way() {
            let streams = _parse_streams(DOWNLOAD).unwrap();
            let svt = How::InKind { encoder: "libsvtav1", settings: &[("preset", "8")], rate: Some(576_097) };
            let plan = _keep_plan(&streams, false, true, None, |index| (index == 0).then_some(576_097));
            let ways: Vec<(usize, usize, &How)> = plan.iter().map(|kept| (kept.input, kept.stream.index, &kept.how)).collect();
            assert_eq!(ways, [(0, 0, &svt), (0, 1, &How::Copy), (1, 2, &How::Copy), (1, 3, &How::Copy), (0, 4, &How::Copy), (0, 5, &How::Copy)]);
            assert!(plan.iter().zip(&streams).all(|(kept, stream)| kept.stream == *stream), "each with its stream as probed");
            let copied = _keep_plan(&streams, true, false, None, |_| unreachable!("a copy measures nothing"));
            assert!(copied.iter().all(|kept| kept.how == How::Copy && kept.input == 0), "{copied:?}");
            assert_eq!(copied.len(), 6, "the data track still left out: {copied:?}");

            let covers = Path::new("/tmp/covers");
            let reattached = How::Reattach(Cover {
                file: covers.join("cover-4.jpg"),
                filename: "cover.jpg".to_string(),
                mimetype: "image/jpeg".to_string(),
            });
            for copy in [false, true] {
                let plan = _keep_plan(&streams, copy, false, Some(covers), |_| Some(576_097));
                assert_eq!(plan[4].how, reattached, "copy: {copy}");
                assert_eq!(plan[5].how, How::Copy, "a font is mapped, as it ever was: {plan:?}");
            }
            // An mjpeg picture that is no cover is video, not a picture to attach.
            let motion = _keep_plan(&[stream(0, "video", "mjpeg", false)], false, false, Some(covers), |_| None);
            assert_eq!(motion[0].how, How::Default, "{motion:?}");
        }

        /// A cover that will not come out — here, from an input that is not there — is kept as a
        /// mapped stream instead of being lost.
        #[test]
        fn a_cover_that_will_not_come_out_is_kept_mapped() {
            let dir = std::env::temp_dir().join(format!("bashrs_cover_fallback_{}", std::process::id()));
            let mut plan = [kept(0, stream(4, "video", "mjpeg", true), How::Reattach(Cover {
                file: dir.join("cover-4.jpg"),
                filename: "cover.jpg".to_string(),
                mimetype: "image/jpeg".to_string(),
            }))];
            _extract_covers(Path::new("/no/such/input.mkv"), &mut plan);
            let _ = std::fs::remove_dir_all(&dir);
            assert_eq!(plan[0].how, How::Copy);
        }

        /// Lossless sound is re-encoded, to the sample; a video codec with no in-kind encoder goes
        /// on the container's default; a rate that cannot be read leaves the encoder's own.
        #[test]
        fn lossless_sound_and_unknown_video_codecs_have_their_own_way() {
            let streams = [stream(0, "video", "prores", false), stream(1, "video", "h264", false), stream(2, "audio", "flac", false), stream(3, "audio", "pcm_s24le", false)];
            let ways: Vec<How> = _keep_plan(&streams, false, false, None, |_| None).into_iter().map(|kept| kept.how).collect();
            assert_eq!(ways, [How::Default, How::InKind { encoder: "libx264", settings: &[], rate: None }, How::Lossless, How::Lossless]);
            for codec in ["opus", "aac", "mp3", "vorbis", "ac3"] {
                assert!(!_lossless_audio(codec), "{codec} is lossy, so copied");
            }
        }

        /// The rate counts only packets starting within the cut — not the lead-in from the
        /// keyframe before it, nor what follows it — over the time they span.
        #[test]
        fn the_rate_is_the_cuts_own_packets_over_their_span() {
            let csv = "29.0,1.0,999999\n30.0,1.0,1000\n31.0,1.0,1000\n32.0,N/A,500\n40.0,1.0,999999\nbroken\n";
            // 2500 bytes from 30.0 to 32.0 (a duration ffprobe has no value for counts as none).
            assert_eq!(_bit_rate_of(csv, 30.0, Some(10.0)), Some(10_000));
            // 1 002 499 bytes from 30.0 to 41.0: everything from the start on.
            assert_eq!(_bit_rate_of(csv, 30.0, None), Some(729_090), "to the end");
            assert_eq!(_bit_rate_of("", 0.0, None), None, "no packets, no rate");
        }

        /// Every kept stream is mapped, and written as its plan says — by its index among the
        /// output's mapped streams; a cover attached again is attached after them all, its tags on
        /// the attachment it becomes (after the mapped ones); a re-encode with a start drops the
        /// copied sound's lead-in, a start reads the input again for the subtitles, and the cues
        /// are fitted to the cut.
        #[test]
        fn a_layout_maps_each_stream_and_writes_it_its_way() {
            let layout = [
                kept(0, stream(0, "video", "av1", false), How::InKind { encoder: "libsvtav1", settings: &[("preset", "8")], rate: Some(576_097) }),
                kept(0, stream(1, "audio", "opus", false), How::Copy),
                kept(1, stream(2, "subtitle", "webvtt", false), How::Copy),
                kept(0, stream(3, "audio", "flac", false), How::Lossless),
                kept(0, stream(4, "video", "prores", false), How::Default),
                kept(0, stream(5, "video", "mjpeg", true), How::Reattach(Cover {
                    file: PathBuf::from("/tmp/covers/cover-5.jpg"),
                    filename: "cover.jpg".to_string(),
                    mimetype: "image/jpeg".to_string(),
                })),
                kept(0, stream(6, "attachment", "ttf", false), How::Copy),
            ];
            let argv = |cut, copy| strs(&_trim_argv(Path::new("in.mkv"), Path::new("out.mkv"), &cut, false, copy, Some(&layout)));
            assert_eq!(
                argv(cut(Some("0:30"), Some("10")), false),
                [
                    "-ss", "0:30", "-i", "in.mkv",
                    "-itsoffset", "-0:30", "-i", "in.mkv",
                    "-map", "0:0", "-c:0", "libsvtav1", "-preset:0", "8", "-b:0", "576097",
                    "-map", "0:1", "-c:1", "copy",
                    "-map", "1:2", "-c:2", "copy",
                    "-map", "0:3", "-c:3", "flac",
                    "-map", "0:4",
                    "-map", "0:6", "-c:5", "copy",
                    "-attach", "/tmp/covers/cover-5.jpg", "-metadata:s:t:1", "mimetype=image/jpeg", "-metadata:s:t:1", "filename=cover.jpg",
                    "-t", "10", "-bsf:a", r"noise=drop='lt(pts\,0)'", "-bsf:s", &_subtitle_fitting(Some("10")), "out.mkv",
                ]
            );
            // As the plan lays them out: a copy, from a start; a re-encode, with no start at all.
            let streams = _parse_streams(DOWNLOAD).unwrap();
            let planned = |cut: Cut, copy| {
                let layout = _keep_plan(&streams, copy, cut.seek.is_some(), None, |_| Some(576_097));
                strs(&_trim_argv(Path::new("in.mkv"), Path::new("out.mkv"), &cut, false, copy, Some(&layout)))
            };
            let copied = planned(cut(Some("30"), None), true);
            assert!(copied.contains(&"-itsoffset".to_string()), "a copy rereads the subtitles too: {copied:?}");
            assert!(!copied.contains(&"-bsf:a".to_string()), "a copy keeps the sound's lead-in, as its picture's: {copied:?}");
            assert_eq!(copied[copied.len() - 3..], ["-bsf:s", &_subtitle_fitting(None), "out.mkv"], "{copied:?}");
            let no_start = planned(cut(None, Some("10")), false);
            assert!(!no_start.contains(&"-itsoffset".to_string()) && !no_start.contains(&"-bsf:a".to_string()), "{no_start:?}");
            assert!(no_start.contains(&"0:2".to_string()) && !no_start.contains(&"1:2".to_string()), "subtitles from the one input: {no_start:?}");
        }

        /// The fitting is ffmpeg's own filter syntax — commas inside an expression escaped, as
        /// a bare one would end the filter there — and it ends the cues at the cut only when the
        /// cut has an end.
        #[test]
        fn subtitle_fitting_drops_what_ended_before_the_start_and_clamps_both_ends() {
            assert_eq!(
                _subtitle_fitting(Some("142.5")),
                r"noise=drop='lte(pts+duration\,0)',setts=pts='max(PTS\,0)':dts='max(DTS\,0)':duration='min(PTS+DURATION\,142.5/TB)-max(PTS\,0)'"
            );
            assert_eq!(
                _subtitle_fitting(None),
                r"noise=drop='lte(pts+duration\,0)',setts=pts='max(PTS\,0)':dts='max(DTS\,0)':duration='PTS+DURATION-max(PTS\,0)'"
            );
        }

        /// The command line as clap sees it: up to two bare points, the two named ones, and the
        /// flags `media_trim_start` had — pinned because this is the first optional positional in
        /// the tree, and `num_args` is where such a thing goes quietly wrong.
        #[test]
        fn trim_takes_up_to_two_points_and_the_named_ones() {
            #[derive(clap::Parser)]
            struct Cli {
                #[command(flatten)]
                trim: TrimArgs,
            }
            use clap::Parser;
            let parse = |argv: &[&str]| Cli::try_parse_from(std::iter::once("media_trim").chain(argv.iter().copied()));

            let two = parse(&["in.mp4", "0:05", "0:30", "-y", "--copy"]).expect("two points").trim;
            assert_eq!(two.points, ["0:05", "0:30"]);
            assert!(two.overwrite.yes && two.copy && two.start.is_none() && two.end.is_none());
            let named = parse(&["in.mp4", "--end", "0:30", "--output", "cut.mp4"]).expect("named end").trim;
            assert!(named.points.is_empty() && named.end.as_deref() == Some("0:30"));
            assert_eq!(named.output, Some(PathBuf::from("cut.mp4")));
            assert!(parse(&["in.mp4"]).is_ok(), "no points parse; resolving them is what refuses");
            assert!(parse(&["in.mp4", "1", "2", "3"]).is_err(), "a third bare point is a usage error");
        }

        #[test]
        fn trimmed_output_lands_beside_the_input() {
            assert_eq!(_trimmed_output(Path::new("dir/clip.mp4")), PathBuf::from("dir/clip_trimmed.mp4"));
            assert_eq!(_trimmed_output(Path::new("noext")), PathBuf::from("noext_trimmed"));
        }

        #[test]
        fn accept_lists_admit_only_what_each_container_can_carry() {
            let accepts = |ext: &str| {
                FORMAT_KNOBS.iter().find(|knobs| knobs.exts.contains(&ext)).unwrap().accepts
            };
            let codecs = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            assert!(_all_accepted(&codecs(&["h264", "aac"]), accepts("mp4")));
            assert!(_all_accepted(&codecs(&["hevc", "aac"]), accepts("mp4")), "h.265 rides in mp4");
            assert!(!_all_accepted(&codecs(&["vp9", "opus"]), accepts("mp4")), "vp9 can't ride in mp4");
            assert!(_all_accepted(&codecs(&["vp9", "opus"]), accepts("webm")));
            assert!(!_all_accepted(&codecs(&["h264", "aac"]), accepts("webm")), "h264 can't ride in webm");
            assert!(_all_accepted(&codecs(&["hevc", "flac", "vp9"]), accepts("mkv")), "matroska takes anything");
            // A video stream (cover art included) keeps audio-only targets on the encode path.
            assert!(!_all_accepted(&codecs(&["mjpeg", "mp3"]), accepts("mp3")));
            for ext in ["jpg", "png", "webp", "gif", "wav"] {
                assert!(accepts(ext).is_empty(), "stream-copy into {ext} never makes sense");
            }
        }

        #[test]
        fn all_accepted_requires_streams_and_full_membership() {
            assert!(!_all_accepted(&[], &["h264"]), "no streams → nothing to copy");
            assert!(!_all_accepted(&["h264".to_string()], &[]), "an empty accept-list never copies");
            assert!(_all_accepted(&["anything".to_string()], &["*"]));
        }

        #[test]
        fn remuxable_is_false_when_the_input_cannot_be_probed() {
            // Deterministic with or without ffprobe installed: the probe fails either way, and an
            // unprobeable input must take the (safe) re-encode path, not the stream copy.
            assert!(!_remuxable(Path::new("/no/such/clip.mp4"), Path::new("out.mkv"), false));
            assert!(!_remuxable(Path::new("/no/such/clip.mp4"), Path::new("out.mkv"), true));
        }

        #[test]
        fn format_rows_are_lowercase_and_unique() {
            // The lookup lowercases the extension once; a mixed-case or duplicated row would
            // silently never match / shadow its twin.
            let mut seen = std::collections::BTreeSet::new();
            for knobs in FORMAT_KNOBS {
                for ext in knobs.exts {
                    assert_eq!(*ext, ext.to_lowercase(), "row extension must be lowercase");
                    assert!(seen.insert(*ext), "duplicate format row for {ext}");
                }
            }
        }

        #[test]
        fn convert_output_takes_a_bare_format_or_a_full_path() {
            // A bare token (with or without the leading dot) converts beside the input …
            assert_eq!(_convert_output(Path::new("dir/a.mov"), Path::new("mp4")), PathBuf::from("dir/a.mp4"));
            assert_eq!(_convert_output(Path::new("a.mov"), Path::new(".ogg")), PathBuf::from("a.ogg"));
            // … while anything path-shaped is taken as given.
            assert_eq!(_convert_output(Path::new("a.mov"), Path::new("out.mp4")), PathBuf::from("out.mp4"));
            assert_eq!(_convert_output(Path::new("a.mov"), Path::new("sub/out")), PathBuf::from("sub/out"));
        }

    }
}
