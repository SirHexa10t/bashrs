//! Report what a media file *is* — container, per-stream facts and embedded tags — rendered in
//! the `lll` table style. Reads only; writes nothing.

#[bashrs_macros::category(command = MediaMetadataCommand, prefix = "media_")]
mod commands {
    use crate::support::doc_style::_header;
    use crate::support::exec::capture_stdout;
    use crate::tools;
    use clap::Args;
    use std::ffi::OsString;
    use std::path::PathBuf;

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

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::categories::media::strs;

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

    }
}
