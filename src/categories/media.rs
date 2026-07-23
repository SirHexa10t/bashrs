#[bashrs_macros::category(command = MediaCommand, prefix = "media_")]
mod commands {
    use crate::support::doc_render;
    use crate::support::doc_style::_header;
    use crate::support::exec::{capture_stdout, run_reporting_code};
    use crate::tools;
    use clap::{Args, ValueEnum};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    // --- media_convert (+ quality / compact profiles) ---------------------------

    /// Convert a media file to another format
    pub fn convert(args: ConvertArgs) {
        let ConvertArgs { input, output, overwrite, bitrate, remux } = args;
        let output = _convert_output(&input, &output);
        let mut tuning: Vec<&str> = Vec::new();
        if remux {
            tuning.extend(["-c", "copy"]);
        }
        if let Some(bitrate) = bitrate.as_deref() {
            tuning.extend(["-b:v", bitrate]);
        }
        _run_convert("media_convert", &input, &output, overwrite, &tuning);
    }

    #[derive(Args)]
    pub struct ConvertArgs {
        /// Input file
        pub input: PathBuf,
        /// Output path — or just a format (`mp4`, `.ogg`, …), converting beside the input
        pub output: PathBuf,
        /// Overwrite output file if it exists
        #[arg(short = 'y', long)]
        pub overwrite: bool,
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
        let ProfiledConvertArgs { input, output, overwrite, h264 } = args;
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
        _run_convert("media_convert_quality", &input, &output, overwrite, tuning);
    }

    /// Convert a media file to another format squeezing the visuals down hard (H.265 video);
    /// audio (if any) stays on its encoder defaults
    pub fn convert_compact(args: ProfiledConvertArgs) {
        let ProfiledConvertArgs { input, output, overwrite, h264 } = args;
        let output = _convert_output(&input, &output);
        let tuning = _profile_flags(&output, Profile::Compact, h264);
        _run_convert("media_convert_compact", &input, &output, overwrite, tuning);
    }

    /// The arguments shared by both profiled conversions — no `--bitrate` or `--remux` here: the
    /// profile owns the quality decision, and a remux never re-encodes to begin with.
    #[derive(Args)]
    pub struct ProfiledConvertArgs {
        /// Input file
        pub input: PathBuf,
        /// Output path — or just a format (`mp4`, `.ogg`, …), converting beside the input
        pub output: PathBuf,
        /// Overwrite output file if it exists
        #[arg(short = 'y', long)]
        pub overwrite: bool,
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
            && _stream_codecs(input, false).is_some_and(|codecs| _all_accepted(&codecs, accepts))
            && (!h264_only
                || _stream_codecs(input, true)
                    .is_some_and(|video| video.iter().all(|codec| codec == "h264")))
    }

    /// Whether `codecs` all sit within `accepts` (`"*"` = a codec-agnostic container, e.g. mkv).
    /// No streams at all is a "no" — there'd be nothing to copy.
    fn _all_accepted(codecs: &[String], accepts: &[&str]) -> bool {
        !codecs.is_empty()
            && codecs
                .iter()
                .all(|codec| accepts.contains(&"*") || accepts.contains(&codec.as_str()))
    }

    /// The codec names of `input`'s streams (all of them, or just the video ones), via ffprobe —
    /// `None` when unprobeable (callers treat that as "don't stream-copy", never as an error).
    fn _stream_codecs(input: &Path, only_video: bool) -> Option<Vec<String>> {
        let mut argv: Vec<OsString> = ["-v", "error", "-show_entries", "stream=codec_name"]
            .into_iter()
            .map(OsString::from)
            .collect();
        if only_video {
            argv.extend(["-select_streams", "v"].map(OsString::from));
        }
        argv.extend(["-of", "csv=p=0"].map(OsString::from));
        argv.push(input.as_os_str().to_owned());
        let out = capture_stdout(tools::resolve("ffprobe"), argv)?;
        Some(out.split_whitespace().map(str::to_owned).collect())
    }

    /// The conversion runner: build the argv, hand off to the shared writing tail.
    fn _run_convert(command: &str, input: &Path, output: &Path, overwrite: bool, tuning: &[&str]) {
        _run_writing(command, input, output, _convert_argv(input, output, overwrite, tuning));
    }

    /// The tail every ffmpeg-writing command shares: refuse writing onto the input itself, run,
    /// and pass ffmpeg's exit code through — it keeps ignorable warnings out of its status (a
    /// warned-but-clean run exits 0), so its code is the honest signal for chaining scripts.
    fn _run_writing(command: &str, input: &Path, output: &Path, argv: Vec<OsString>) {
        if output == input {
            eprintln!("{command}: the output is the input itself ({})", input.display());
            std::process::exit(1);
        }
        let code = run_reporting_code(tools::resolve("ffmpeg"), argv);
        if code != 0 {
            std::process::exit(code);
        }
        _report_saved(output.to_owned());
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

    // --- media_trim_start -------------------------------------------------------

    /// Cut the first part off a video/audio file — frame-accurate (re-encoding on the output
    /// container's defaults), or instant and lossless with `--copy` at keyframe precision
    pub fn trim_start(args: TrimStartArgs) {
        let TrimStartArgs { input, duration, output, overwrite, copy } = args;
        let output = output.unwrap_or_else(|| _trimmed_output(&input));
        _run_writing(
            "media_trim_start",
            &input,
            &output,
            _trim_argv(&input, &output, &duration, overwrite, copy),
        );
    }

    #[derive(Args)]
    pub struct TrimStartArgs {
        /// Input video/audio file
        pub input: PathBuf,
        /// How much to cut off the front — seconds (`1.2`) or a timestamp (`0:05.5`)
        pub duration: String,
        /// Output file (defaults to `<input>_trimmed.<ext>`, beside the input)
        #[arg(long)]
        pub output: Option<PathBuf>,
        /// Overwrite output file if it exists
        #[arg(short = 'y', long)]
        pub overwrite: bool,
        /// Copy the streams instead of re-encoding: instant and lossless, but the cut snaps to
        /// the nearest keyframe — possibly seconds off on sparse-keyframe video
        #[arg(long)]
        pub copy: bool,
    }

    /// ffmpeg argv for the trim. `-ss` sits *before* `-i`: ffmpeg then seeks instead of decoding
    /// through the skipped part — and when re-encoding it is still frame-accurate, decoding
    /// forward from the preceding keyframe and discarding up to the requested point.
    fn _trim_argv(
        input: &Path,
        output: &Path,
        duration: &str,
        overwrite: bool,
        copy: bool,
    ) -> Vec<OsString> {
        let mut argv: Vec<OsString> = Vec::new();
        if overwrite {
            argv.push("-y".into());
        }
        argv.extend(["-ss", duration].map(OsString::from));
        argv.push("-i".into());
        argv.push(input.as_os_str().to_owned());
        if copy {
            argv.extend(["-c", "copy"].map(OsString::from));
        }
        argv.push(output.as_os_str().to_owned());
        argv
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

    // --- media_metadata -------------------------------------------------------

    /// Get metadata of an audio/video/image file: container and per-stream facts, plus any
    /// embedded tags (a yt-dlp download carries title, uploader, date, description, and source
    /// URL) — one style throughout: a blue title leading its value on the same line, or an
    /// indented block from the next line for multi-line values. A file with no tags just shows
    /// no tags.
    pub fn metadata(args: MetadataArgs) {
        // ffprobe's own stderr passes through, so an unreadable file already said why.
        let Some(json) = capture_stdout(tools::resolve("ffprobe"), _metadata_argv(&args)) else {
            std::process::exit(1);
        };
        let Some(report) = _render_report(&json) else {
            eprintln!("media_metadata: could not parse ffprobe's report");
            std::process::exit(1);
        };
        print!("{report}");
        let mut argv: Vec<OsString> =
            ["-v", "error", "-show_entries", "format_tags"].map(OsString::from).to_vec();
        argv.push(args.file.as_os_str().to_owned());
        let Some(raw) = capture_stdout(tools::resolve("ffprobe"), argv) else {
            std::process::exit(1);
        };
        print!("{}", _tags_yamlish(&raw));
    }

    /// ffprobe's JSON report reshaped into the tag section's style: one `title: value` line per
    /// fact, container facts first, then one line per stream (`video: h264, 640x480, …`; a
    /// second stream of the same kind reads `audio 2:`). Absent facts are simply skipped — an
    /// image has no duration, an mkv stream no bit rate. `None` when the JSON doesn't parse.
    fn _render_report(json: &str) -> Option<String> {
        let report: serde_json::Value = serde_json::from_str(json).ok()?;
        let mut out = String::new();
        let mut push = |key: &str, value: Option<String>| {
            if let Some(value) = value {
                out += &format!("{}: {value}\n", _header(key));
            }
        };
        let format = &report["format"];
        push("file", _field(format, "filename"));
        push("container", _field(format, "format_name"));
        push("size", _field(format, "size").and_then(|s| s.parse().ok()).map(_human_size));
        push("duration", _field(format, "duration").and_then(|s| s.parse().ok()).map(_clock));
        push("bit rate", _field(format, "bit_rate").and_then(|s| s.parse().ok()).map(_bit_rate));
        let empty = Vec::new();
        let streams = report["streams"].as_array().unwrap_or(&empty);
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for stream in streams {
            let kind = _field(stream, "codec_type").unwrap_or_else(|| "stream".to_string());
            let nth = seen.entry(kind.clone()).and_modify(|n| *n += 1).or_insert(1);
            let label = if *nth > 1 { format!("{kind} {nth}") } else { kind };
            push(&label, Some(_stream_line(stream)));
        }
        Some(out)
    }

    /// One stream's facts as a `, `-joined line, in a fixed order that reads naturally for
    /// video (codec, geometry, rate, length) and audio (codec, sampling, channels, length)
    /// alike — every absent field just doesn't appear.
    fn _stream_line(stream: &serde_json::Value) -> String {
        let geometry = match (_field(stream, "width"), _field(stream, "height")) {
            (Some(width), Some(height)) => Some(format!("{width}x{height}")),
            _ => None,
        };
        let parts = [
            _field(stream, "codec_name"),
            geometry,
            _field(stream, "pix_fmt"),
            _field(stream, "r_frame_rate").and_then(|r| _fps(&r)).map(|fps| format!("{fps} fps")),
            _field(stream, "sample_rate").and_then(|s| s.parse().ok()).map(_khz),
            _field(stream, "channels").map(|n| match n.as_str() {
                "1" => "1 channel".to_string(),
                n => format!("{n} channels"),
            }),
            _field(stream, "nb_frames").map(|n| format!("{n} frames")),
            _field(stream, "duration").and_then(|s| s.parse().ok()).map(_clock),
            _field(stream, "bit_rate").and_then(|s| s.parse().ok()).map(_bit_rate),
        ];
        parts.into_iter().flatten().collect::<Vec<_>>().join(", ")
    }

    /// A JSON field as display text — ffprobe mixes types (`width` is a number, `size` a
    /// string), and both read the same here. `None` for anything else, including absence.
    fn _field(value: &serde_json::Value, key: &str) -> Option<String> {
        match &value[key] {
            serde_json::Value::String(text) => Some(text.clone()),
            serde_json::Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }

    /// `5086670` → `4.9 MiB` — binary units, one decimal (dropped when zero); exact below 1 KiB.
    fn _human_size(bytes: u64) -> String {
        let mut value = bytes as f64;
        let mut unit = "B";
        for bigger in ["KiB", "MiB", "GiB", "TiB"] {
            if value < 1024.0 {
                break;
            }
            value /= 1024.0;
            unit = bigger;
        }
        if unit == "B" { format!("{bytes} B") } else { _one_decimal(value, unit) }
    }

    /// `2238186` → `2.2 Mbit/s` — decimal (SI) units, as rates are conventionally quoted.
    fn _bit_rate(bits_per_second: u64) -> String {
        let mut value = bits_per_second as f64;
        let mut unit = "bit/s";
        for bigger in ["kbit/s", "Mbit/s", "Gbit/s"] {
            if value < 1000.0 {
                break;
            }
            value /= 1000.0;
            unit = bigger;
        }
        _one_decimal(value, unit)
    }

    /// `44100` → `44.1 kHz`; sub-kHz rates stay in Hz.
    fn _khz(hz: u64) -> String {
        if hz < 1000 { format!("{hz} Hz") } else { _one_decimal(hz as f64 / 1000.0, "kHz") }
    }

    fn _one_decimal(value: f64, unit: &str) -> String {
        let text = format!("{value:.1}");
        format!("{} {unit}", text.strip_suffix(".0").unwrap_or(&text))
    }

    /// Seconds → `H:MM:SS.cc` (`19.06` → `0:00:19.06`) — ffprobe-style clock, centisecond cut.
    fn _clock(seconds: f64) -> String {
        let whole = seconds as u64;
        format!("{}:{:02}:{:05.2}", whole / 3600, (whole % 3600) / 60, seconds % 60.0)
    }

    /// A frame-rate ratio (`30/1`, `30000/1001`) as a readable rate (`30`, `29.97`); `None`
    /// for the `0/0` placeholder non-video streams carry.
    fn _fps(ratio: &str) -> Option<String> {
        let (numerator, denominator) = ratio.split_once('/')?;
        let (numerator, denominator): (f64, f64) =
            (numerator.parse().ok()?, denominator.parse().ok()?);
        (denominator > 0.0 && numerator > 0.0).then(|| {
            let text = format!("{:.2}", numerator / denominator);
            text.trim_end_matches('0').trim_end_matches('.').to_string()
        })
    }

    /// ffprobe's `TAG:`-prefixed block reshaped as yaml: lowercased keys (in `lll`'s header
    /// style), multi-line values as indented block scalars — instead of continuation lines
    /// dumped flush-left under a `TAG:KEY=first-line` opener. A tag-less file yields nothing
    /// at all: the report above stands alone, no placeholder line.
    fn _tags_yamlish(raw: &str) -> String {
        let tags = _parse_tags(raw);
        if tags.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        for (key, value) in tags {
            let key = _header(&key);
            if value.contains('\n') {
                out += &format!("{key}: |\n");
                for line in value.lines() {
                    out += &format!("  {line}\n");
                }
            } else {
                out += &format!("{key}: {value}\n");
            }
        }
        out
    }

    /// Parse ffprobe's `-show_entries format_tags` default output: a `TAG:key=value` line opens
    /// a tag, bare lines continue the previous value (that's how multi-line descriptions
    /// arrive), and the `[FORMAT]` wrappers are noise.
    fn _parse_tags(raw: &str) -> Vec<(String, String)> {
        let mut tags: Vec<(String, String)> = Vec::new();
        for line in raw.lines() {
            if line == "[FORMAT]" || line == "[/FORMAT]" {
                continue;
            }
            if let Some(pair) = line.strip_prefix("TAG:") {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                tags.push((key.to_lowercase(), value.to_string()));
            } else if let Some((_, value)) = tags.last_mut() {
                value.push('\n');
                value.push_str(line);
            }
        }
        tags
    }

    #[derive(Args)]
    pub struct MetadataArgs {
        /// File to inspect (audio, video, or image)
        pub file: PathBuf,
    }

    /// ffprobe argv for the machine-readable JSON that [`_render_report`] reshapes. The stream
    /// fields cover video and audio alike, with `codec_type` labelling which stream is which;
    /// duration and bit rate are also read at format level, where they exist even for containers
    /// whose streams don't carry them (mkv, notably). Values arrive raw (no `-pretty`) — the
    /// renderer's own humanizers control the formatting. `-v error` alone suppresses the banner.
    fn _metadata_argv(args: &MetadataArgs) -> Vec<OsString> {
        let mut argv: Vec<OsString> = [
            "-v", "error",
            "-show_entries",
            "stream=codec_type,codec_name,width,height,pix_fmt,r_frame_rate,nb_frames,sample_rate,channels,bit_rate,duration",
            "-show_entries", "format=filename,format_name,size,duration,bit_rate",
            "-print_format", "json",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        argv.push(args.file.clone().into());
        argv
    }

    // --- media_remove_vocals ----------------------------------------------------

    /// Copy an audio or video file with the vocals removed — center-channel cancellation over a
    /// tunable frequency band; video, subtitles, and cover art pass through untouched
    pub fn remove_vocals(args: RemoveVocalsArgs) {
        if args.range_help {
            print!("{}", doc_render::render_doc(RANGE_HELP));
            return;
        }
        match _remove_vocals(&args) {
            Ok(output) => println!("wrote {}", output.display()),
            Err(msg) => {
                eprintln!("media_remove_vocals: {msg}");
                std::process::exit(1);
            }
        }
    }

    /// The vocal-frequency reference `--range-help` prints (rendered like the `dl -c` page):
    /// how to place --from/--to, then the per-voice/per-genre fundamental tables to place them by.
    const RANGE_HELP: &str = include_str!("templates/remove_vocals_ranges.md");

    #[derive(Args)]
    pub struct RemoveVocalsArgs {
        /// Input file (audio, or video with audio)
        #[arg(required_unless_present = "range_help")]
        pub input: Option<PathBuf>,
        /// Output path (defaults to `<input>_novocals.<ext>` beside the input)
        pub output: Option<PathBuf>,
        /// Cancel above this Hz; bass below it is kept. Accepted 0-500 (--range-help)
        #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u32).range(0..=500))]
        pub from: u32,
        /// Cancel up to this Hz; treble above it is kept. Accepted: above --from, ≤18000 (--range-help)
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=18000))]
        pub to: Option<u32>,
        /// Print the vocal-frequency reference for choosing --from/--to, then exit
        #[arg(long)]
        pub range_help: bool,
        /// Overwrite output file if it exists
        #[arg(short = 'y', long)]
        pub overwrite: bool,
    }

    /// The vocal-removal graph for a cancellation band of `from`..`to` Hz. Vocals sit dead-center
    /// in a stereo mix, so subtracting the channels cancels them (the karaoke trick) — but only
    /// the chosen band is cancelled: everything below `from` (bass, usually center-panned too)
    /// and above `to` (cymbals, "air") is split off and carried through intact. Each crossover
    /// edge is four stacked 2-pole filters (≈48 dB/octave): a single gentle slope measurably
    /// leaked centered vocals across the seam at only ~-22 dB — the behaviour test caught it.
    fn _devocal_filter(from: u32, to: Option<u32>) -> String {
        // Four stacked 2-pole filters ≈ a 48 dB/octave crossover edge.
        let steep = |kind: &str, hz: u32| vec![format!("{kind}=f={hz}"); 4].join(",");
        let low = from > 0; // keep everything below `from`? (a `from` of 0 keeps no low band)
        let high = to.is_some(); // keep everything above `to`?

        // Fan the input out: one branch per kept band, plus the mids that get cancelled.
        let inputs: Vec<&str> = std::iter::once("mids_in")
            .chain(low.then_some("low_in"))
            .chain(high.then_some("high_in"))
            .collect();
        let labels = |names: &[&str]| names.iter().map(|n| format!("[{n}]")).collect::<String>();
        let mut chains =
            vec![format!("[0:a]asplit={}{}", inputs.len(), labels(&inputs))];

        // The cancelled band: bandlimit the mids to `from`..`to`, then subtract L−R / R−L.
        let mut mids = format!("[mids_in]{}", steep("highpass", from.max(1)));
        if let Some(to) = to {
            mids.push_str(&format!(",{}", steep("lowpass", to)));
        }
        mids.push_str(",pan=stereo|c0=c0-c1|c1=c1-c0[karaoke]");
        chains.push(mids);

        // The carried-through bands, mixed back in with the cancelled mids at unity gain.
        let mut keeps = vec!["karaoke"];
        if low {
            chains.push(format!("[low_in]{}[low]", steep("lowpass", from)));
            keeps.push("low");
        }
        if let Some(to) = to {
            chains.push(format!("[high_in]{}[high]", steep("highpass", to)));
            keeps.push("high");
        }
        chains.push(format!("{}amix=inputs={}:normalize=0[novocals]", labels(&keeps), keeps.len()));
        chains.join(";")
    }

    /// The checks and the ffmpeg run behind [`remove_vocals`] — split off so failure paths
    /// return instead of exiting. Cheap, tool-free refusals come first; the stereo check needs
    /// a probe (cancellation is a two-channel trick: mono has no center to subtract, and
    /// surround carries its own dedicated center channel this filter doesn't address).
    fn _remove_vocals(args: &RemoveVocalsArgs) -> Result<PathBuf, String> {
        let input = args.input.as_ref().ok_or("no input file given")?;
        if let Some(to) = args.to {
            if to <= args.from {
                return Err(format!(
                    "--to ({to} Hz) must be above --from ({} Hz) — the cancellation band is from…to",
                    args.from
                ));
            }
        }
        let output = args.output.clone().unwrap_or_else(|| _novocals_output(input));
        if &output == input {
            return Err("the output is the input itself — pick another name".to_string());
        }
        if !args.overwrite && output.exists() {
            return Err(format!(
                "'{}' already exists — pass -y/--overwrite to replace it",
                output.display()
            ));
        }
        match _audio_channels(input)? {
            2 => {}
            1 => {
                return Err(
                    "mono audio has no center channel to cancel — vocal removal works on stereo mixes"
                        .to_string(),
                )
            }
            n => {
                return Err(format!(
                    "{n}-channel audio isn't supported — downmix to stereo first (surround keeps \
                     its voice in a dedicated center channel, a different removal than this one)"
                ))
            }
        }
        let filter = _devocal_filter(args.from, args.to);
        if run_reporting_code(tools::resolve("ffmpeg"), _remove_vocals_argv(input, &output, &filter)) != 0 {
            return Err("ffmpeg failed (its message above has the reason)".to_string());
        }
        Ok(output)
    }

    /// ffmpeg argv for the de-vocal run: the filtered audio replaces the original, while video
    /// (including an attached cover), subtitles, and attachments are stream-copied when present
    /// (`?`-maps). `-y` is safe here — the exists/overwrite policy was already enforced.
    fn _remove_vocals_argv(input: &Path, output: &Path, filter: &str) -> Vec<OsString> {
        let mut argv: Vec<OsString> = ["-v", "error", "-y", "-i"].map(OsString::from).to_vec();
        argv.push(input.as_os_str().to_owned());
        argv.extend(
            [
                "-filter_complex", filter,
                "-map", "[novocals]",
                "-map", "0:v?", "-c:v", "copy",
                "-map", "0:s?", "-c:s", "copy",
                "-map", "0:t?", "-c:t", "copy",
            ]
            .map(OsString::from),
        );
        argv.push(output.as_os_str().to_owned());
        argv
    }

    /// The first audio stream's channel count, by ffprobe. A file with no audio stream at all
    /// reads as an error (there is nothing to de-vocal in it).
    fn _audio_channels(input: &Path) -> Result<u32, String> {
        let mut argv: Vec<OsString> =
            ["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=channels", "-of", "csv=p=0"]
                .map(OsString::from)
                .to_vec();
        argv.push(input.as_os_str().to_owned());
        let out = capture_stdout(tools::resolve("ffprobe"), argv)
            .ok_or_else(|| "could not read the file (ffprobe failed)".to_string())?;
        out.trim().parse().map_err(|_| "the file has no audio stream".to_string())
    }

    /// The default de-vocal output: the input's name with a `_novocals` mark, same directory.
    fn _novocals_output(input: &Path) -> PathBuf {
        let stem = input.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let name = match input.extension() {
            Some(ext) => format!("{stem}_novocals.{}", ext.to_string_lossy()),
            None => format!("{stem}_novocals"),
        };
        input.with_file_name(name)
    }

    // --- media_hmerge_imgs ----------------------------------------------------

    /// Merge images side by side onto one canvas — as tall as the tallest input, as wide as all
    /// inputs combined; nothing is scaled, and `--align` places each image vertically
    pub fn hmerge_imgs(args: HmergeImgsArgs) {
        let HmergeImgsArgs { output, overwrite, align, inputs } = args;
        _merge_imgs(Axis::Horizontal, align.offset_expr(), output, overwrite, inputs);
    }

    #[derive(Args)]
    pub struct HmergeImgsArgs {
        /// Output file (defaults to auto-generated name)
        #[arg(long)]
        pub output: Option<PathBuf>,
        /// Overwrite output file if it exists
        #[arg(short = 'y', long)]
        pub overwrite: bool,
        /// Where each image sits vertically on the canvas (which is as tall as the tallest input).
        #[arg(long, value_enum, default_value_t = VerticalAlign::Middle)]
        pub align: VerticalAlign,
        /// Input images (at least 2 required); merged at full size, in the order given.
        #[arg(required = true, num_args = 2..)]
        pub inputs: Vec<PathBuf>,
    }

    /// Vertical placement of each image within a side-by-side merge.
    #[derive(Clone, Copy, Debug, ValueEnum)]
    pub enum VerticalAlign {
        Top,
        Middle,
        Bottom,
    }

    impl VerticalAlign {
        /// The pad `y` expression — `oh` is the canvas height, `ih` the image's own.
        fn offset_expr(self) -> &'static str {
            match self {
                Self::Top => "0",
                Self::Middle => "(oh-ih)/2",
                Self::Bottom => "oh-ih",
            }
        }
    }

    // --- media_vmerge_imgs ----------------------------------------------------

    /// Merge images top to bottom onto one canvas — as wide as the widest input, as tall as all
    /// inputs combined; nothing is scaled, and `--align` places each image horizontally
    pub fn vmerge_imgs(args: VmergeImgsArgs) {
        let VmergeImgsArgs { output, overwrite, align, inputs } = args;
        _merge_imgs(Axis::Vertical, align.offset_expr(), output, overwrite, inputs);
    }

    #[derive(Args)]
    pub struct VmergeImgsArgs {
        /// Output file (defaults to auto-generated name)
        #[arg(long)]
        pub output: Option<PathBuf>,
        /// Overwrite output file if it exists
        #[arg(short = 'y', long)]
        pub overwrite: bool,
        /// Where each image sits horizontally on the canvas (which is as wide as the widest input).
        #[arg(long, value_enum, default_value_t = HorizontalAlign::Center)]
        pub align: HorizontalAlign,
        /// Input images (at least 2 required); merged at full size, in the order given.
        #[arg(required = true, num_args = 2..)]
        pub inputs: Vec<PathBuf>,
    }

    /// Horizontal placement of each image within a top-to-bottom merge.
    #[derive(Clone, Copy, Debug, ValueEnum)]
    pub enum HorizontalAlign {
        Left,
        Center,
        Right,
    }

    impl HorizontalAlign {
        /// The pad `x` expression — `ow` is the canvas width, `iw` the image's own.
        fn offset_expr(self) -> &'static str {
            match self {
                Self::Left => "0",
                Self::Center => "(ow-iw)/2",
                Self::Right => "ow-iw",
            }
        }
    }

    // --- shared merge machinery -----------------------------------------------

    /// Everything that differs between the two merges: which dimension the canvas fixes (and
    /// ffprobe must therefore measure), how each image is padded onto it, and which stack filter
    /// joins the results.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Axis {
        /// Side by side — the canvas fixes a shared height, `hstack` sums the widths.
        Horizontal,
        /// Top to bottom — the canvas fixes a shared width, `vstack` sums the heights.
        Vertical,
    }

    impl Axis {
        /// The ffprobe entry naming the canvas dimension: the tallest height, or the widest width.
        fn probe_entry(self) -> &'static str {
            match self {
                Self::Horizontal => "stream=height",
                Self::Vertical => "stream=width",
            }
        }

        /// Pad one image up to the canvas dimension — keeping its other dimension — placed along
        /// the canvas by the alignment's `offset` expression.
        fn pad(self, canvas: u32, offset: &str) -> String {
            match self {
                Self::Horizontal => format!("pad=w=iw:h={canvas}:x=0:y={offset}"),
                Self::Vertical => format!("pad=w={canvas}:h=ih:x={offset}:y=0"),
            }
        }

        /// The filter joining the padded images; it derives the summed dimension itself.
        fn stack(self) -> &'static str {
            match self {
                Self::Horizontal => "hstack",
                Self::Vertical => "vstack",
            }
        }

        /// The user-facing command name, for error reports.
        fn command(self) -> &'static str {
            match self {
                Self::Horizontal => "media_hmerge_imgs",
                Self::Vertical => "media_vmerge_imgs",
            }
        }

        /// The default output name's first word: which way this merge stacked.
        fn merged_stem(self) -> &'static str {
            match self {
                Self::Horizontal => "hmerged",
                Self::Vertical => "vmerged",
            }
        }
    }

    /// The shared merge driver: probe the canvas dimension, settle the output name, run ffmpeg.
    fn _merge_imgs(
        axis: Axis,
        offset: &'static str,
        output: Option<PathBuf>,
        overwrite: bool,
        inputs: Vec<PathBuf>,
    ) {
        // The canvas dimension is the one fact ffmpeg can't derive inside the filtergraph (each
        // input's `pad` sees only its own dimensions), so it's probed up front via ffprobe.
        let Some(canvas) = _max_dimension(axis, &inputs) else { std::process::exit(1) };
        let output = output.unwrap_or_else(|| _default_merge_output(axis, &inputs));
        let code = run_reporting_code(
            tools::resolve("ffmpeg"),
            _merge_argv(axis, &inputs, &output, overwrite, offset, canvas),
        );
        if code != 0 {
            std::process::exit(code);
        }
        _report_saved(output);
    }

    /// Default output name when `--output` is omitted: the merge's direction, then the input file
    /// stems joined with underscores — e.g. inputs `a.png b.jpg` -> `hmerged_a_b.png`.
    fn _default_merge_output(axis: Axis, inputs: &[PathBuf]) -> PathBuf {
        let stems: String = inputs
            .iter()
            .map(|path| format!("_{}", path.file_stem().unwrap_or_default().to_string_lossy()))
            .collect();
        PathBuf::from(format!("{}{stems}.png", axis.merged_stem()))
    }

    fn _merge_argv(
        axis: Axis,
        inputs: &[PathBuf],
        output: &Path,
        overwrite: bool,
        offset: &str,
        canvas: u32,
    ) -> Vec<OsString> {
        let mut argv: Vec<OsString> = Vec::new();
        if overwrite {
            argv.push("-y".into());
        }
        for input in inputs {
            argv.push("-i".into());
            argv.push(input.clone().into());
        }
        argv.push("-filter_complex".into());
        argv.push(_merge_filter(axis, inputs.len(), canvas, offset).into());
        // A single image, not a sequence — without this, modern ffmpeg warns about the filename
        // lacking a `%03d`-style pattern.
        argv.push("-update".into());
        argv.push("1".into());
        argv.push(output.as_os_str().to_owned());
        argv
    }

    /// The merge filtergraph: every input is converted to one shared pixel format (the stacks
    /// refuse mixed formats, e.g. RGBA png beside YUV jpg) and padded to the canvas dimension —
    /// keeping its own size, positioned by `offset`, the filler transparent (black where the
    /// output format has no alpha) — then all the equal-sized columns/rows are stacked. The stack
    /// sums the other dimension itself, so only the canvas dimension needs probing.
    fn _merge_filter(axis: Axis, count: usize, canvas: u32, offset: &str) -> String {
        let pad = axis.pad(canvas, offset);
        let padded: String = (0..count)
            .map(|i| format!("[{i}:v]format=rgba,{pad}:color=black@0[p{i}];"))
            .collect();
        let refs: String = (0..count).map(|i| format!("[p{i}]")).collect();
        format!("{padded}{refs}{}=inputs={count}", axis.stack())
    }

    /// The largest canvas dimension among the inputs — the merge canvas size. `None` (already
    /// reported) when any input can't be measured.
    fn _max_dimension(axis: Axis, inputs: &[PathBuf]) -> Option<u32> {
        inputs
            .iter()
            .map(|path| _image_dimension(axis, path))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .max()
    }

    /// One image's size along the canvas dimension, read via ffprobe (`None` after reporting, on
    /// any failure).
    fn _image_dimension(axis: Axis, path: &Path) -> Option<u32> {
        let Some(out) = capture_stdout(tools::resolve("ffprobe"), _probe_argv(axis, path)) else {
            eprintln!("{}: could not measure {}", axis.command(), path.display());
            return None;
        };
        match out.trim().parse() {
            Ok(dimension) => Some(dimension),
            Err(_) => {
                eprintln!(
                    "{}: unexpected ffprobe size {out:?} for {}",
                    axis.command(),
                    path.display()
                );
                None
            }
        }
    }

    /// ffprobe argv printing just the canvas dimension of `path`, as a bare number.
    fn _probe_argv(axis: Axis, path: &Path) -> Vec<OsString> {
        let mut argv: Vec<OsString> =
            ["-v", "error", "-select_streams", "v:0", "-show_entries", axis.probe_entry(), "-of", "csv=p=0"]
                .into_iter()
                .map(OsString::from)
                .collect();
        argv.push(path.as_os_str().to_owned());
        argv
    }

    // --- helper ---------------------------------------------------------------

    /// Report a just-written output file by its canonical path.
    fn _report_saved(path: PathBuf) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        println!("Saved: {}", path.display());
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn strs(argv: &[OsString]) -> Vec<String> {
            argv.iter().map(|a| a.to_string_lossy().into_owned()).collect()
        }

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

        #[test]
        fn trim_argv_seeks_before_the_input() {
            // `-ss` after `-i` would decode through the skipped part instead of seeking past it.
            let argv = strs(&_trim_argv(Path::new("in.mp4"), Path::new("out.mp4"), "1.2", false, false));
            assert_eq!(argv, ["-ss", "1.2", "-i", "in.mp4", "out.mp4"]);
            let copy = strs(&_trim_argv(Path::new("in.mp4"), Path::new("out.mp4"), "0:05", true, true));
            assert_eq!(copy, ["-y", "-ss", "0:05", "-i", "in.mp4", "-c", "copy", "out.mp4"]);
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

        #[test]
        fn metadata_argv_labels_streams_and_ends_with_the_target_file() {
            let args = MetadataArgs { file: PathBuf::from("clip.mp4") };
            let argv = strs(&_metadata_argv(&args));
            assert_eq!(argv.last().unwrap(), "clip.mp4");
            let streams = argv.iter().find(|arg| arg.starts_with("stream=")).expect("stream entries");
            for field in ["codec_type", "pix_fmt", "sample_rate"] {
                assert!(streams.contains(field), "missing {field} in {streams}");
            }
        }

        #[test]
        fn ffprobe_tags_parse_including_multiline_values() {
            let raw = "[FORMAT]\nTAG:title=[t] PS\nTAG:DESCRIPTION=line one\nline two\nTAG:DATE=20190620\n[/FORMAT]\n";
            let tags = _parse_tags(raw);
            assert_eq!(tags[0], ("title".into(), "[t] PS".into()));
            assert_eq!(tags[1], ("description".into(), "line one\nline two".into()));
            assert_eq!(tags[2].0, "date");
        }

        #[test]
        fn tags_render_yaml_style_with_block_scalars() {
            let out = _tags_yamlish("TAG:DATE=20190620\nTAG:DESCRIPTION=a\nb\n");
            assert!(out.contains(": 20190620\n"), "{out}");
            assert!(out.contains(": |\n  a\n  b\n"), "multi-line values become indented blocks: {out}");
            assert_eq!(_tags_yamlish(""), "", "a tag-less file shows nothing, not a placeholder");
        }

        #[test]
        fn the_report_renders_container_then_streams_in_the_tag_sections_style() {
            // The shape ffprobe emits for a yt-dlp mkv (mixed value types on purpose: numbers
            // for geometry, strings for the format-level figures) plus a second audio stream,
            // which must come out numbered.
            let json = r#"{
                "streams": [
                    {"codec_type": "video", "codec_name": "h264", "width": 640, "height": 480,
                     "pix_fmt": "yuv420p", "r_frame_rate": "30000/1001", "nb_frames": "573"},
                    {"codec_type": "audio", "codec_name": "aac", "sample_rate": "44100",
                     "channels": 2, "r_frame_rate": "0/0", "bit_rate": "127999"},
                    {"codec_type": "audio", "codec_name": "opus", "sample_rate": "48000", "channels": 1}
                ],
                "format": {"filename": "clip.mkv", "format_name": "matroska,webm",
                           "size": "5086670", "duration": "19.061000", "bit_rate": "2134940"}
            }"#;
            let out = _render_report(json).expect("parses");
            let line = |key: &str, value: &str| format!("{}: {value}\n", _header(key));
            assert!(out.starts_with(&line("file", "clip.mkv")), "{out}");
            assert!(out.contains(&line("container", "matroska,webm")), "{out}");
            assert!(out.contains(&line("size", "4.9 MiB")), "{out}");
            assert!(out.contains(&line("duration", "0:00:19.06")), "{out}");
            assert!(out.contains(&line("bit rate", "2.1 Mbit/s")), "{out}");
            assert!(
                out.contains(&line("video", "h264, 640x480, yuv420p, 29.97 fps, 573 frames")),
                "{out}"
            );
            assert!(
                out.contains(&line("audio", "aac, 44.1 kHz, 2 channels, 128 kbit/s")),
                "the 0/0 placeholder rate must not appear: {out}"
            );
            assert!(out.contains(&line("audio 2", "opus, 48 kHz, 1 channel")), "{out}");
            assert!(_render_report("not json").is_none());
        }

        #[test]
        fn the_report_skips_what_a_file_does_not_have() {
            // An image: no duration, no bit rate, no audio facts anywhere.
            let json = r#"{
                "streams": [{"codec_type": "video", "codec_name": "png", "width": 32, "height": 32}],
                "format": {"filename": "pic.png", "format_name": "png_pipe", "size": "512"}
            }"#;
            let out = _render_report(json).expect("parses");
            assert!(out.contains(&format!("{}: 512 B\n", _header("size"))), "{out}");
            assert!(!out.contains("duration") && !out.contains("bit rate"), "{out}");
            assert!(out.contains(&format!("{}: png, 32x32\n", _header("video"))), "{out}");
        }

        #[test]
        fn the_humanizers_pick_sane_units_and_drop_empty_decimals() {
            assert_eq!(_human_size(512), "512 B");
            assert_eq!(_human_size(1024), "1 KiB");
            assert_eq!(_human_size(5_086_670), "4.9 MiB");
            assert_eq!(_bit_rate(800), "800 bit/s");
            assert_eq!(_bit_rate(128_000), "128 kbit/s");
            assert_eq!(_bit_rate(2_238_186), "2.2 Mbit/s");
            assert_eq!(_khz(800), "800 Hz");
            assert_eq!(_khz(44_100), "44.1 kHz");
            assert_eq!(_khz(48_000), "48 kHz");
            assert_eq!(_clock(19.061), "0:00:19.06");
            assert_eq!(_clock(3661.5), "1:01:01.50");
            assert_eq!(_fps("30/1").as_deref(), Some("30"));
            assert_eq!(_fps("30000/1001").as_deref(), Some("29.97"));
            assert_eq!(_fps("0/0"), None, "the placeholder ratio is not a rate");
        }

        #[test]
        fn metadata_argv_reads_duration_and_bit_rate_at_format_level_too() {
            // Some containers (mkv, notably) carry duration/bit_rate only at format level;
            // stream-only entries would show nothing at all for them.
            let args = MetadataArgs { file: PathBuf::from("clip.mkv") };
            let argv = strs(&_metadata_argv(&args));
            assert!(argv.contains(&"format=filename,format_name,size,duration,bit_rate".to_string()), "{argv:?}");
        }

        #[test]
        fn novocals_output_marks_the_name_beside_the_input() {
            assert_eq!(_novocals_output(Path::new("a/song.mp3")), PathBuf::from("a/song_novocals.mp3"));
            assert_eq!(_novocals_output(Path::new("bare")), PathBuf::from("bare_novocals"));
        }

        /// A RemoveVocalsArgs with the band defaults, for the pure checks.
        fn devocal_args(input: &str, output: Option<&str>) -> RemoveVocalsArgs {
            RemoveVocalsArgs {
                input: Some(PathBuf::from(input)),
                output: output.map(PathBuf::from),
                from: 120,
                to: None,
                range_help: false,
                overwrite: false,
            }
        }

        #[test]
        fn remove_vocals_argv_filters_audio_and_copies_everything_else() {
            let filter = _devocal_filter(120, None);
            let argv = strs(&_remove_vocals_argv(Path::new("in.mkv"), Path::new("out.mkv"), &filter));
            assert_eq!(argv.last().unwrap(), "out.mkv");
            assert!(argv.contains(&"[novocals]".to_string()), "the filtered audio is mapped");
            assert!(argv.contains(&filter), "the built filter graph is passed through");
            for (map, codec) in [("0:v?", "-c:v"), ("0:s?", "-c:s"), ("0:t?", "-c:t")] {
                assert!(argv.contains(&map.to_string()), "optional {map} rides along");
                assert!(argv.contains(&codec.to_string()), "{codec} copy, no re-encode");
            }
        }

        #[test]
        fn the_devocal_filter_bands_track_from_and_to() {
            // Default: a bass-keep below `from`, cancellation above it, no ceiling.
            let default = _devocal_filter(120, None);
            assert!(default.contains("c0=c0-c1"), "center cancellation is always present");
            assert!(default.contains("lowpass=f=120"), "bass kept below --from: {default}");
            assert!(default.contains("highpass=f=120"), "mids cancelled from --from up: {default}");
            assert!(!default.contains("high_in"), "no treble-keep band without --to: {default}");
            assert_eq!(default.matches("asplit=2").count(), 1, "two branches: mids + low");

            // --to adds a treble-keep band and bounds the cancelled mids on top.
            let bounded = _devocal_filter(180, Some(9000));
            assert!(bounded.contains("asplit=3"), "three branches: mids + low + high: {bounded}");
            assert!(bounded.contains("highpass=f=9000"), "treble kept above --to: {bounded}");
            assert!(bounded.contains("lowpass=f=9000"), "mids cancelled only up to --to: {bounded}");
            assert!(bounded.contains("highpass=f=180") && bounded.contains("lowpass=f=180"));

            // --from 0: no bass-keep band at all, cancellation runs from the bottom.
            let full_low = _devocal_filter(0, None);
            assert!(full_low.contains("asplit=1"), "only the mids branch: {full_low}");
            assert!(!full_low.contains("[low]"), "no low keep when --from is 0: {full_low}");
            // Every band edge is the steep 4-pole crossover (guards the anti-leak slope).
            assert_eq!(default.matches("lowpass=f=120").count(), 4, "4-pole low edge: {default}");
        }

        #[test]
        fn remove_vocals_refuses_bad_inputs_before_touching_any_tool() {
            let onto_itself = devocal_args("x.mp3", Some("x.mp3"));
            assert!(_remove_vocals(&onto_itself).unwrap_err().contains("input itself"));

            let mut inverted = devocal_args("in.mp3", Some("out.mp3"));
            inverted.from = 200;
            inverted.to = Some(150);
            assert!(_remove_vocals(&inverted).unwrap_err().contains("must be above"), "to<=from is refused");

            let dir = std::env::temp_dir().join(format!("bashrs_devocal_{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let taken = dir.join("taken.mp3");
            std::fs::write(&taken, b"x").unwrap();
            let onto_existing = devocal_args("in.mp3", taken.to_str());
            assert!(_remove_vocals(&onto_existing).unwrap_err().contains("already exists"));
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn range_help_connects_the_knobs_to_the_frequency_tables() {
            // The reference is useless for THIS command if it never names the knobs, and the
            // knobs are useless without the pitch data to place them by — both must be present.
            assert!(RANGE_HELP.contains("--from") && RANGE_HELP.contains("--to"), "names the knobs");
            assert!(RANGE_HELP.contains("Fundamental (Hz)"), "carries the pitch reference");
            // Renders through the same path as `dl -c` without mangling — headings coloured,
            // table rows intact.
            let rendered = doc_render::render_doc(RANGE_HELP);
            assert!(rendered.contains("Whistle register"), "table content survives rendering");
        }

        /// Skip-with-notice for the behavioural de-vocal tests (they need real ffmpeg/ffprobe,
        /// resolved the same way the command resolves them).
        fn ffmpeg_or_skip(test: &str) -> bool {
            let works = ["ffmpeg", "ffprobe"].iter().all(|tool| {
                std::process::Command::new(crate::tools::resolve(tool))
                    .arg("-version")
                    .output()
                    .is_ok_and(|out| out.status.success())
            });
            if !works {
                eprintln!("SKIPPED {test}: no usable ffmpeg/ffprobe available");
            }
            works
        }

        /// A 2-second wav synthesized from an ffmpeg lavfi source expression.
        fn build_audio(source: &str, out: &Path) {
            let ok = std::process::Command::new(crate::tools::resolve("ffmpeg"))
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", source])
                .arg(out)
                .status()
                .is_ok_and(|status| status.success());
            assert!(ok, "could not synthesize {source}");
        }

        /// The file's `mean_volume` in dB, per ffmpeg's volumedetect.
        fn mean_volume(file: &Path) -> f32 {
            let out = std::process::Command::new(crate::tools::resolve("ffmpeg"))
                .arg("-i")
                .arg(file)
                .args(["-af", "volumedetect", "-f", "null", "-"])
                .output()
                .expect("run ffmpeg");
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .find_map(|line| line.split("mean_volume:").nth(1))
                .and_then(|rest| rest.trim().strip_suffix(" dB").and_then(|db| db.trim().parse().ok()))
                .expect("volumedetect reported a mean")
        }

        fn devocal(input: &Path) -> PathBuf {
            devocal_band(input, 120, None)
        }

        /// De-vocal `input` with an explicit cancellation band, returning the output path.
        fn devocal_band(input: &Path, from: u32, to: Option<u32>) -> PathBuf {
            let args = RemoveVocalsArgs {
                input: Some(input.to_path_buf()),
                output: None,
                from,
                to,
                range_help: false,
                overwrite: true,
            };
            _remove_vocals(&args).expect("de-vocal run works")
        }

        #[test]
        fn centered_content_cancels_while_side_content_and_bass_survive() {
            // The physics of the trick, measured: a dead-center tone (how vocals are mixed)
            // must cancel to near-silence; off-center tones (instruments) and center BASS
            // (kept via the crossover) must come through at comparable loudness.
            if !ffmpeg_or_skip("de-vocal behaviour") {
                return;
            }
            let dir = std::env::temp_dir().join(format!("bashrs_devocal_beh_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            let vocal = dir.join("vocal.wav"); // identical L/R = perfectly centered, 440 Hz
            build_audio("aevalsrc=sin(440*2*PI*t)|sin(440*2*PI*t):d=2", &vocal);
            let out = devocal(&vocal);
            assert!(
                mean_volume(&out) < mean_volume(&vocal) - 40.0,
                "a centered tone must all but vanish: {} → {} dB",
                mean_volume(&vocal),
                mean_volume(&out)
            );

            let side = dir.join("side.wav"); // different tones per channel = off-center music
            build_audio("aevalsrc=sin(300*2*PI*t)|sin(600*2*PI*t):d=2", &side);
            let out = devocal(&side);
            assert!(
                mean_volume(&out) > mean_volume(&side) - 10.0,
                "off-center content must survive: {} → {} dB",
                mean_volume(&side),
                mean_volume(&out)
            );

            let bass = dir.join("bass.wav"); // centered like a vocal, but below the crossover
            build_audio("aevalsrc=sin(80*2*PI*t)|sin(80*2*PI*t):d=2", &bass);
            let out = devocal(&bass);
            assert!(
                mean_volume(&out) > mean_volume(&bass) - 8.0,
                "center bass rides the kept low band: {} → {} dB",
                mean_volume(&bass),
                mean_volume(&out)
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn the_band_edges_actually_move_what_gets_cancelled() {
            // The whole point of the knobs: a centered tone that the DEFAULT band cancels must
            // SURVIVE once it's placed outside a narrowed band — proving --from and --to relocate
            // the cancellation, not just decorate the help.
            if !ffmpeg_or_skip("de-vocal band tuning") {
                return;
            }
            let dir = std::env::temp_dir().join(format!("bashrs_devocal_band_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            // Explicit distinct outputs: the default output name is derived from the INPUT, so
            // two runs over one input would collide and the second would overwrite the first.
            let run = |input: &Path, out: &str, from: u32, to: Option<u32>| -> PathBuf {
                let out = dir.join(out);
                let args = RemoveVocalsArgs {
                    input: Some(input.to_path_buf()),
                    output: Some(out),
                    from,
                    to,
                    range_help: false,
                    overwrite: true,
                };
                _remove_vocals(&args).expect("de-vocal run works")
            };

            // A centered 150 Hz tone: the default band (from=120) cancels it; raising --from to
            // 200 moves it into the kept low band, so it survives.
            let tone = dir.join("t.wav");
            build_audio("aevalsrc=sin(150*2*PI*t)|sin(150*2*PI*t):d=2", &tone);
            let cancelled = run(&tone, "cancelled.wav", 120, None);
            let kept = run(&tone, "kept.wav", 200, None);
            assert!(
                mean_volume(&kept) > mean_volume(&cancelled) + 12.0, // a 4x+ amplitude gap: unmistakable
                "raising --from past the tone must spare it: default {} dB vs --from 200 {} dB",
                mean_volume(&cancelled),
                mean_volume(&kept)
            );

            // A centered 12 kHz tone ("cymbal"): the default (no ceiling) cancels it; a --to of
            // 9000 puts it in the kept treble band, so it survives.
            let hi = dir.join("hi.wav");
            build_audio("aevalsrc=sin(12000*2*PI*t)|sin(12000*2*PI*t):d=2", &hi);
            let hi_cancelled = run(&hi, "hi_cancelled.wav", 120, None);
            let hi_kept = run(&hi, "hi_kept.wav", 120, Some(9000));
            assert!(
                mean_volume(&hi_kept) > mean_volume(&hi_cancelled) + 12.0,
                "a --to ceiling must spare a tone above it: no-ceiling {} dB vs --to 9000 {} dB",
                mean_volume(&hi_cancelled),
                mean_volume(&hi_kept)
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn devocaling_a_video_copies_the_video_stream_and_refuses_mono() {
            if !ffmpeg_or_skip("de-vocal on video / mono") {
                return;
            }
            let dir = std::env::temp_dir().join(format!("bashrs_devocal_vid_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            let clip = dir.join("clip.mkv");
            let ok = std::process::Command::new(crate::tools::resolve("ffmpeg"))
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=blue:s=64x64:d=2"])
                .args(["-f", "lavfi", "-i", "aevalsrc=sin(440*2*PI*t)|sin(440*2*PI*t):d=2"])
                .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
                .arg(&clip)
                .status()
                .is_ok_and(|status| status.success());
            assert!(ok, "could not build the clip");
            let out = devocal(&clip);
            let codec = std::process::Command::new(crate::tools::resolve("ffprobe"))
                .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_name", "-of", "csv=p=0"])
                .arg(&out)
                .output()
                .expect("probe");
            assert_eq!(
                String::from_utf8_lossy(&codec.stdout).trim(),
                "h264",
                "the video stream is copied through, not re-encoded away"
            );
            assert!(mean_volume(&out) < -40.0, "the centered tone is gone from the video's audio");

            let mono = dir.join("mono.wav");
            build_audio("sine=frequency=440:duration=1", &mono);
            let args = RemoveVocalsArgs {
                input: Some(mono),
                output: None,
                from: 120,
                to: None,
                range_help: false,
                overwrite: true,
            };
            assert!(
                _remove_vocals(&args).unwrap_err().contains("stereo"),
                "mono is refused with the honest reason"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn default_merge_output_names_the_direction_and_joins_input_stems() {
            // Distinct per direction: both merges over the same inputs must not collide.
            let inputs = [PathBuf::from("a.png"), PathBuf::from("dir/b.jpg")];
            assert_eq!(_default_merge_output(Axis::Horizontal, &inputs), PathBuf::from("hmerged_a_b.png"));
            assert_eq!(_default_merge_output(Axis::Vertical, &inputs), PathBuf::from("vmerged_a_b.png"));
        }

        #[test]
        fn merge_argv_has_one_input_flag_per_image_then_the_filtergraph() {
            let inputs = [PathBuf::from("a.png"), PathBuf::from("b.png")];
            let argv = strs(&_merge_argv(Axis::Horizontal, &inputs, Path::new("out.png"), false, "0", 64));
            assert_eq!(argv[..4], ["-i", "a.png", "-i", "b.png"]);
            assert_eq!(argv[4], "-filter_complex");
            assert_eq!(argv[5], _merge_filter(Axis::Horizontal, 2, 64, "0"));
            assert_eq!(argv[6..], ["-update", "1", "out.png"]);
        }

        #[test]
        fn hmerge_filter_pads_every_input_to_the_canvas_then_stacks_all() {
            // Three inputs → three pad columns and `inputs=3` (a bare `hstack` would silently
            // merge only the first two).
            let filter = _merge_filter(Axis::Horizontal, 3, 1080, "0");
            assert_eq!(filter.matches("pad=w=iw:h=1080:x=0:y=0").count(), 3, "{filter}");
            assert!(filter.contains("[2:v]format=rgba,"), "{filter}");
            assert!(filter.ends_with("[p0][p1][p2]hstack=inputs=3"), "{filter}");
        }

        #[test]
        fn vmerge_filter_pads_widths_and_stacks_vertically() {
            let filter = _merge_filter(Axis::Vertical, 2, 640, "(ow-iw)/2");
            assert_eq!(filter.matches("pad=w=640:h=ih:x=(ow-iw)/2:y=0").count(), 2, "{filter}");
            assert!(filter.ends_with("[p0][p1]vstack=inputs=2"), "{filter}");
        }

        #[test]
        fn alignments_map_to_pad_offset_expressions() {
            assert_eq!(VerticalAlign::Top.offset_expr(), "0");
            assert_eq!(VerticalAlign::Middle.offset_expr(), "(oh-ih)/2");
            assert_eq!(VerticalAlign::Bottom.offset_expr(), "oh-ih");
            assert_eq!(HorizontalAlign::Left.offset_expr(), "0");
            assert_eq!(HorizontalAlign::Center.offset_expr(), "(ow-iw)/2");
            assert_eq!(HorizontalAlign::Right.offset_expr(), "ow-iw");
        }

        #[test]
        fn probe_argv_asks_for_the_bare_canvas_dimension_of_its_axis() {
            let argv = strs(&_probe_argv(Axis::Horizontal, Path::new("img.png")));
            assert_eq!(argv.last().unwrap(), "img.png");
            assert!(argv.contains(&"stream=height".to_string()), "{argv:?}");
            assert!(argv.contains(&"csv=p=0".to_string()), "bare number output: {argv:?}");
            let vertical = strs(&_probe_argv(Axis::Vertical, Path::new("img.png")));
            assert!(vertical.contains(&"stream=width".to_string()), "{vertical:?}");
        }

        #[test]
        fn max_dimension_reports_and_bails_on_an_unreadable_input() {
            // Deterministic with or without ffprobe installed: a missing file fails the probe
            // either way, and the failure must abort the merge (None), not default to something.
            assert_eq!(_max_dimension(Axis::Horizontal, &[PathBuf::from("/no/such/image.png")]), None);
        }
    }
}
