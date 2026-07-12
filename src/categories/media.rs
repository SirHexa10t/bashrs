#[bashrs_macros::category(command = MediaCommand, prefix = "media_")]
mod commands {
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

    /// Get metadata of an audio/video/image file
    pub fn metadata(args: MetadataArgs) {
        // ffprobe prints the report itself; an unreadable file's own status becomes ours.
        let code = run_reporting_code(tools::resolve("ffprobe"), _metadata_argv(&args));
        if code != 0 {
            std::process::exit(code);
        }
    }

    /// Get metadata plus the file's embedded tags — a yt-dlp download carries title, uploader,
    /// date, description, and source URL — rendered yaml-style
    pub fn metadata_yt(args: MetadataArgs) {
        let code = run_reporting_code(tools::resolve("ffprobe"), _metadata_argv(&args));
        if code != 0 {
            std::process::exit(code);
        }
        let mut argv: Vec<OsString> =
            ["-v", "error", "-show_entries", "format_tags"].map(OsString::from).to_vec();
        argv.push(args.file.as_os_str().to_owned());
        let Some(raw) = capture_stdout(tools::resolve("ffprobe"), argv) else {
            std::process::exit(1);
        };
        print!("{}", _tags_yamlish(&raw));
    }

    /// ffprobe's `TAG:`-prefixed block reshaped as yaml: lowercased keys (in `lll`'s header
    /// style), multi-line values as indented block scalars — instead of continuation lines
    /// dumped flush-left under a `TAG:KEY=first-line` opener.
    fn _tags_yamlish(raw: &str) -> String {
        let tags = _parse_tags(raw);
        if tags.is_empty() {
            return "(no embedded tags)\n".to_string();
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

    /// ffprobe argv for a human-readable (`-pretty`) JSON report. The stream fields cover video
    /// and audio alike, with `codec_type` labelling which stream is which; duration and bit rate
    /// are also read at format level, where they exist even for containers whose streams don't
    /// carry them (mkv, notably). `-v error` alone suppresses the banner.
    fn _metadata_argv(args: &MetadataArgs) -> Vec<OsString> {
        let mut argv: Vec<OsString> = [
            "-v", "error",
            "-show_entries",
            "stream=codec_type,codec_name,width,height,pix_fmt,r_frame_rate,nb_frames,sample_rate,channels,bit_rate,duration",
            "-show_entries", "format=filename,format_name,size,duration,bit_rate",
            "-pretty",
            "-print_format", "json",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        argv.push(args.file.clone().into());
        argv
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
            assert_eq!(_tags_yamlish(""), "(no embedded tags)\n");
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
