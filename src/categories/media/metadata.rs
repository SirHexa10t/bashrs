//! Report what a media file *is* — container, per-stream facts and embedded tags — rendered in
//! the `lll` table style; or several files' reports side by side, what they share lined up.
//! Reads only; writes nothing.

#[bashrs_macros::category(command = MediaMetadataCommand, prefix = "media_")]
mod commands {
    use crate::support::comfy_repos::{comparison_row, comparison_table_options};
    use crate::support::doc_style::_header;
    use crate::support::exec::capture_stdout;
    use crate::tools;
    use clap::Args;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use table_formatter::FormatOptions;

    // --- media_metadata -------------------------------------------------------

    /// Get metadata of an audio/video/image file — or of up to four, side by side: container and
    /// per-stream facts, plus any embedded tags (a yt-dlp download carries title, uploader, date,
    /// description, and source URL) — one style throughout: a blue title leading its value on the
    /// same line, or an indented block from the next line for multi-line values. A file with no
    /// tags just shows no tags.
    pub fn metadata(args: MetadataArgs) {
        if let [file] = args.files.as_slice() {
            let Some((json, raw)) = _probe(file) else { std::process::exit(1) };
            let Some(report) = _render_report(&json) else {
                eprintln!("media_metadata: could not parse ffprobe's report");
                std::process::exit(1);
            };
            print!("{report}{}", _tags_yamlish(&raw));
            return;
        }
        let mut reports: Vec<Vec<Field>> = Vec::new();
        for file in &args.files {
            let Some((json, raw)) = _probe(file) else { std::process::exit(1) };
            let Some(mut fields) = _report_fields(&json) else {
                eprintln!("media_metadata: could not parse ffprobe's report on {}", file.display());
                std::process::exit(1);
            };
            fields.extend(_tag_fields(&raw));
            reports.push(fields);
        }
        for line in _side_by_side(&reports, &comparison_table_options()) {
            println!("{line}");
        }
    }

    /// What ffprobe says of `file`: the JSON report [`_report_fields`] reads, and the tag block
    /// [`_tag_fields`] reads — `None` once either run fails, ffprobe's own stderr passing through,
    /// so an unreadable file has already said why.
    fn _probe(file: &Path) -> Option<(String, String)> {
        let json = capture_stdout(tools::resolve("ffprobe"), _metadata_argv(file))?;
        let mut argv: Vec<OsString> =
            ["-v", "error", "-show_entries", "format_tags"].map(OsString::from).to_vec();
        argv.push(file.as_os_str().to_owned());
        let raw = capture_stdout(tools::resolve("ffprobe"), argv)?;
        Some((json, raw))
    }

    /// One fact as the report prints it: the lines it takes, the first leading with its key —
    /// `title: value`, or `description: |` with the value's lines indented beneath. Its section and
    /// key are what a comparison lines facts up by.
    struct Field {
        section: Section,
        key: String,
        lines: Vec<String>,
    }

    /// Where a fact comes from: the report's own reading of the container and its streams, or the
    /// file's embedded tags — two vocabularies, so a tag named `duration` is no container's.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Section {
        Facts,
        Tags,
    }

    /// `fields` as the report prints them, one after another.
    fn _text(fields: &[Field]) -> String {
        fields.iter().flat_map(|field| &field.lines).map(|line| format!("{line}\n")).collect()
    }

    /// ffprobe's JSON report reshaped into the tag section's style: one `title: value` line per
    /// fact, container facts first, then one line per stream (`video: h264, 640x480, …`; a
    /// second stream of the same kind reads `audio 2:`). Absent facts are simply skipped — an
    /// image has no duration, an mkv stream no bit rate. `None` when the JSON doesn't parse.
    fn _render_report(json: &str) -> Option<String> {
        _report_fields(json).map(|fields| _text(&fields))
    }

    /// The facts [`_render_report`] prints, one [`Field`] each.
    fn _report_fields(json: &str) -> Option<Vec<Field>> {
        let report: serde_json::Value = serde_json::from_str(json).ok()?;
        let mut out = Vec::new();
        let mut push = |key: &str, value: Option<String>| {
            if let Some(value) = value {
                out.push(Field {
                    section: Section::Facts,
                    key: key.to_string(),
                    lines: vec![format!("{}: {value}", _header(key))],
                });
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
        _text(&_tag_fields(raw))
    }

    /// The tags [`_tags_yamlish`] prints, one [`Field`] each.
    fn _tag_fields(raw: &str) -> Vec<Field> {
        _parse_tags(raw)
            .into_iter()
            .map(|(key, value)| {
                let header = _header(&key);
                let lines = if value.contains('\n') {
                    std::iter::once(format!("{header}: |")).chain(value.lines().map(|line| format!("  {line}"))).collect()
                } else {
                    vec![format!("{header}: {value}")]
                };
                Field { section: Section::Tags, key, lines }
            })
            .collect()
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

    /// The reports side by side — one column per file, the comparison shape at `options` — from
    /// [`_comparison_rows`]. Should the library refuse (only a width too narrow for even one
    /// ` | ` can make it), the same rows, unaligned.
    fn _side_by_side(reports: &[Vec<Field>], options: &FormatOptions) -> Vec<String> {
        let rows = _comparison_rows(reports);
        let lines: Vec<String> = rows.iter().map(|cells| comparison_row(cells)).collect();
        table_formatter::format_table(&lines, options).unwrap_or_else(|_| rows.iter().map(|cells| cells.join(" | ")).collect())
    }

    /// The reports as rows of one cell per file: first the facts every file has, in the first
    /// file's order; then the facts only some have, still level with each other, a blank where a
    /// file has none; last, each file's own, stacked under its column. A fact that takes more
    /// lines in one file than in another pads the others with empty cells, so the next fact
    /// starts level again.
    fn _comparison_rows(reports: &[Vec<Field>]) -> Vec<Vec<String>> {
        // Each fact by its section, key, and which of the file's facts by that name it is — a
        // tag can repeat.
        let identities: Vec<Vec<(Section, &str, usize)>> = reports
            .iter()
            .map(|fields| {
                fields
                    .iter()
                    .enumerate()
                    .map(|(at, field)| {
                        let earlier = fields[..at].iter().filter(|other| other.section == field.section && other.key == field.key);
                        (field.section, field.key.as_str(), earlier.count())
                    })
                    .collect()
            })
            .collect();
        // In the order they first appear: the first file's, then each later file's new ones.
        let mut order: Vec<(Section, &str, usize)> = Vec::new();
        for id in identities.iter().flatten() {
            if !order.contains(id) {
                order.push(*id);
            }
        }
        let holders = |id: &(Section, &str, usize)| identities.iter().filter(|ids| ids.contains(id)).count();
        let (shared, own): (Vec<_>, Vec<_>) = order.into_iter().partition(|id| holders(id) > 1);
        let (everywhere, somewhere): (Vec<_>, Vec<_>) = shared.into_iter().partition(|id| holders(id) == reports.len());
        let mut rows = Vec::new();
        for id in everywhere.iter().chain(&somewhere) {
            let columns: Vec<&[String]> = reports
                .iter()
                .zip(&identities)
                .map(|(fields, ids)| ids.iter().position(|it| it == id).map_or(&[][..], |at| &fields[at].lines[..]))
                .collect();
            _level(&mut rows, &columns);
        }
        let own_lines: Vec<Vec<String>> = reports
            .iter()
            .zip(&identities)
            .map(|(fields, ids)| {
                fields.iter().zip(ids).filter(|(_, id)| own.contains(id)).flat_map(|(field, _)| field.lines.clone()).collect()
            })
            .collect();
        _level(&mut rows, &own_lines.iter().map(Vec::as_slice).collect::<Vec<_>>());
        rows
    }

    /// `columns` appended to `rows` side by side, as many rows as the tallest column takes — the
    /// shorter ones padded with empty cells.
    fn _level(rows: &mut Vec<Vec<String>>, columns: &[&[String]]) {
        let height = columns.iter().map(|lines| lines.len()).max().unwrap_or(0);
        for at in 0..height {
            rows.push(columns.iter().map(|lines| lines.get(at).cloned().unwrap_or_default()).collect());
        }
    }

    #[derive(Args)]
    pub struct MetadataArgs {
        /// File to inspect (audio, video, or image) — or up to four, compared side by side
        #[arg(required = true, num_args = 1..=4, value_name = "FILE")]
        pub files: Vec<PathBuf>,
    }

    /// ffprobe argv for the machine-readable JSON that [`_render_report`] reshapes. The stream
    /// fields cover video and audio alike, with `codec_type` labelling which stream is which;
    /// duration and bit rate are also read at format level, where they exist even for containers
    /// whose streams don't carry them (mkv, notably). Values arrive raw (no `-pretty`) — the
    /// renderer's own humanizers control the formatting. `-v error` alone suppresses the banner.
    fn _metadata_argv(file: &Path) -> Vec<OsString> {
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
        argv.push(file.as_os_str().to_owned());
        argv
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::categories::media::strs;

        #[test]
        fn metadata_argv_labels_streams_and_ends_with_the_target_file() {
            let argv = strs(&_metadata_argv(Path::new("clip.mp4")));
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
            let argv = strs(&_metadata_argv(Path::new("clip.mkv")));
            assert!(argv.contains(&"format=filename,format_name,size,duration,bit_rate".to_string()), "{argv:?}");
        }

        /// A fact as a comparison sees one: its section, its key, and the lines it prints as —
        /// unstyled here, since lining facts up never reads the lines.
        fn fact(section: Section, key: &str, lines: &[&str]) -> Field {
            Field { section, key: key.to_string(), lines: lines.iter().map(|line| (*line).to_string()).collect() }
        }

        fn line(key: &str, value: &str) -> Field {
            fact(Section::Facts, key, &[&format!("{key}: {value}")])
        }

        fn tag(key: &str, value: &str) -> Field {
            fact(Section::Tags, key, &[&format!("{key}: {value}")])
        }

        /// The facts both files have come first, in the first file's order — the second file's
        /// own order does not move them — and each file's own come last, under its column.
        #[test]
        fn shared_facts_lead_in_the_first_files_order_and_each_files_own_come_last() {
            let first = vec![line("file", "a.mkv"), line("duration", "0:01"), tag("title", "A"), line("audio 2", "opus")];
            let second = vec![line("file", "b.mkv"), tag("title", "B"), line("duration", "0:02"), tag("encoder", "Lavf")];
            assert_eq!(
                _comparison_rows(&[first, second]),
                [
                    ["file: a.mkv", "file: b.mkv"],
                    ["duration: 0:01", "duration: 0:02"],
                    ["title: A", "title: B"],
                    ["audio 2: opus", "encoder: Lavf"],
                ]
            );
        }

        /// A fact taking more lines in one file pads the others with empty cells, so the fact after
        /// it starts level in every column.
        #[test]
        fn a_fact_taller_in_one_file_pads_the_others_to_its_height() {
            let first = vec![fact(Section::Tags, "description", &["description: |", "  one", "  two"]), tag("date", "1")];
            let second = vec![tag("description", "short"), tag("date", "2")];
            assert_eq!(
                _comparison_rows(&[first, second]),
                [
                    ["description: |", "description: short"],
                    ["  one", ""],
                    ["  two", ""],
                    ["date: 1", "date: 2"],
                ]
            );
        }

        /// With three files, a fact only two have still lines up — after every fact all three
        /// have, however early the first file lists it, a blank where the third has none — and
        /// only a fact one file alone has goes to the bottom of its column.
        #[test]
        fn facts_only_some_files_have_stay_level_after_the_shared_ones() {
            let first = vec![line("file", "a"), tag("comment", "x"), tag("date", "1"), tag("genre", "film")];
            let second = vec![line("file", "b"), tag("date", "2")];
            let third = vec![line("file", "c"), tag("comment", "y"), tag("date", "3"), tag("purl", "url")];
            assert_eq!(
                _comparison_rows(&[first, second, third]),
                [
                    ["file: a", "file: b", "file: c"],
                    ["date: 1", "date: 2", "date: 3"],
                    ["comment: x", "", "comment: y"],
                    ["genre: film", "", "purl: url"],
                ]
            );
        }

        /// A tag named like one of the report's own facts is a different fact: they never pair.
        /// And a tag the file repeats pairs by which of its namesakes it is.
        #[test]
        fn facts_pair_by_section_key_and_place_among_namesakes() {
            let first = vec![line("duration", "0:10"), tag("duration", "a tag"), tag("artist", "one"), tag("artist", "two")];
            let second = vec![line("duration", "0:20"), tag("artist", "three")];
            assert_eq!(
                _comparison_rows(&[first, second]),
                [
                    ["duration: 0:10", "duration: 0:20"],
                    ["artist: one", "artist: three"],
                    ["duration: a tag", ""],
                    ["artist: two", ""],
                ]
            );
        }

        /// Laid out: one ` | ` per gap, each at the same column on every line; empty cells left
        /// empty, never the library's `-` filler; a block's indentation kept in the first column
        /// too; and narrowed to the width given, by wrapping, where the table would not fit.
        #[test]
        fn the_comparison_lays_the_columns_out_level_and_within_the_width() {
            let first = vec![line("file", "a.mkv"), fact(Section::Tags, "description", &["description: |", "  indented"])];
            let second = vec![line("file", "b.mkv"), tag("description", "flat")];
            let wide = _side_by_side(&[first, second], &crate::support::comfy_repos::comparison_table_options_at(200));
            assert_eq!(wide, ["file: a.mkv    | file: b.mkv", "description: | | description: flat", "  indented     |"]);

            let long = "word ".repeat(30);
            let narrow = _side_by_side(&[vec![tag("note", &long)], vec![tag("note", "short")]], &crate::support::comfy_repos::comparison_table_options_at(60));
            assert!(narrow.len() > 1, "the long note wrapped: {narrow:#?}");
            for row in &narrow {
                assert!(table_formatter::visible_len(row) <= 60, "over the width: {row:?}");
            }
        }

        /// One file is the report as it always was; up to four compare; a fifth, or none, is a
        /// usage error.
        #[test]
        fn one_to_four_files_are_taken() {
            #[derive(clap::Parser)]
            struct Cli {
                #[command(flatten)]
                metadata: MetadataArgs,
            }
            use clap::Parser;
            let parse = |files: &[&str]| Cli::try_parse_from(std::iter::once("media_metadata").chain(files.iter().copied()));
            assert_eq!(parse(&["a.mkv"]).expect("one").metadata.files, [PathBuf::from("a.mkv")]);
            assert_eq!(parse(&["a", "b", "c", "d"]).expect("four").metadata.files.len(), 4);
            assert!(parse(&["a", "b", "c", "d", "e"]).is_err(), "a fifth file");
            assert!(parse(&[]).is_err(), "no file at all");
        }
    }
}
