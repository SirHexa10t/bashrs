//! Audio-signal effects over a media file's sound. Currently center-channel vocal cancellation
//! across a tunable frequency band (`media_remove_vocals`), whose `--range-help` frequency
//! reference is the Markdown doc in `../templates/`.

#[bashrs_macros::category(command = MediaAudioFxCommand, prefix = "media_")]
mod commands {
    use crate::support::doc_render;
    use crate::support::exec::capture_stdout;
    use crate::tools;
    use clap::Args;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    // --- media_remove_vocals ----------------------------------------------------

    /// Copy an audio or video file with the vocals removed — center-channel cancellation over a
    /// tunable frequency band; video, subtitles, and cover art pass through untouched
    pub fn remove_vocals(args: RemoveVocalsArgs) {
        if args.range_help {
            // The frequency tables render `table_fancy`-style, sized to the live window.
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
    const RANGE_HELP: &str = include_str!("../templates/remove_vocals_ranges.md");

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
        if super::_run_ffmpeg(_remove_vocals_argv(input, &output, &filter)) != 0 {
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::categories::media::strs;

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
            // The render mechanics (colour; prose 1:1; framed, window-bounded tables; no leaked
            // markers) are the shared checker's job — run this doc through it too.
            doc_render::assert_render_invariants(RANGE_HELP);
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
            let ok = std::process::Command::new(crate::tools::resolve("ffmpeg")).stdin(std::process::Stdio::null())
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", source])
                .arg(out)
                .status()
                .is_ok_and(|status| status.success());
            assert!(ok, "could not synthesize {source}");
        }

        /// The file's `mean_volume` in dB, per ffmpeg's volumedetect.
        fn mean_volume(file: &Path) -> f32 {
            let out = std::process::Command::new(crate::tools::resolve("ffmpeg")).stdin(std::process::Stdio::null())
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
            let ok = std::process::Command::new(crate::tools::resolve("ffmpeg")).stdin(std::process::Stdio::null())
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

    }
}
