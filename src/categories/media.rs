#[bashrs_macros::category(command = MediaCommand, prefix = "media_")]
mod commands {
    use crate::support::exec::{capture_stdout, run_reporting};
    use crate::tools;
    use clap::{Args, ValueEnum};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    // --- media_conv -----------------------------------------------------------

    /// Convert a media file using ffmpeg
    pub fn conv(args: ConvArgs) {
        if run_reporting(tools::resolve("ffmpeg"), _conv_argv(&args)) {
            _report_saved(args.output);
        }
    }

    #[derive(Args)]
    pub struct ConvArgs {
        /// Input file
        pub input: PathBuf,
        /// Output file (extension determines format)
        pub output: PathBuf,
        /// Video bitrate (e.g. 1M, 500k)
        #[arg(short, long, default_value = "1M")]
        pub bitrate: String,
        /// Overwrite output file if it exists
        #[arg(short = 'y', long)]
        pub overwrite: bool,
    }

    /// Build the ffmpeg argument vector for a conversion. Kept separate from
    /// execution so the argument ordering can be unit-tested without ffmpeg.
    fn _conv_argv(args: &ConvArgs) -> Vec<OsString> {
        let mut argv: Vec<OsString> = Vec::new();
        if args.overwrite {
            argv.push("-y".into());
        }
        argv.push("-i".into());
        argv.push(args.input.clone().into());
        argv.push("-b:v".into());
        argv.push(args.bitrate.clone().into());
        argv.push(args.output.clone().into());
        argv
    }

    // --- media_metadata -------------------------------------------------------

    /// Get metadata of an audio/video/image file
    pub fn metadata(args: MetadataArgs) {
        run_reporting(tools::resolve("ffprobe"), _metadata_argv(&args)); // ffprobe prints the report itself
    }

    #[derive(Args)]
    pub struct MetadataArgs {
        /// File to inspect (audio, video, or image)
        pub file: PathBuf,
    }

    fn _metadata_argv(args: &MetadataArgs) -> Vec<OsString> {
        let mut argv: Vec<OsString> = [
            "-v", "error",
            "-show_entries", "stream=width,height,r_frame_rate,nb_frames,codec_name,channels,bit_rate,duration",
            "-show_entries", "format=filename,size",
            "-hide_banner",
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
        // The canvas height is the one fact ffmpeg can't derive inside the filtergraph (each
        // input's `pad` sees only its own dimensions), so it's probed up front via ffprobe.
        let Some(canvas_height) = _max_height(&inputs) else { return };
        let output = output.unwrap_or_else(|| _default_merge_output(&inputs));
        if run_reporting(tools::resolve("ffmpeg"), _hmerge_argv(&inputs, &output, overwrite, align, canvas_height)) {
            _report_saved(output);
        }
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
        #[arg(long, value_enum, default_value_t = Align::Middle)]
        pub align: Align,
        /// Input images (at least 2 required); merged at full size, in the order given.
        #[arg(required = true, num_args = 2..)]
        pub inputs: Vec<PathBuf>,
    }

    /// Vertical placement of each image within the merge canvas.
    #[derive(Clone, Copy, Debug, ValueEnum)]
    pub enum Align {
        Top,
        Middle,
        Bottom,
    }

    /// The `pad` y-offset expression for an alignment — `oh` is the canvas height, `ih` the image's.
    fn _align_expr(align: Align) -> &'static str {
        match align {
            Align::Top => "0",
            Align::Middle => "(oh-ih)/2",
            Align::Bottom => "oh-ih",
        }
    }

    /// Default output name when `--output` is omitted: the input file stems
    /// joined with underscores, e.g. inputs `a.png b.jpg` -> `merged_a_b.png`.
    fn _default_merge_output(inputs: &[PathBuf]) -> PathBuf {
        let stems: String = inputs
            .iter()
            .map(|path| format!("_{}", path.file_stem().unwrap_or_default().to_string_lossy()))
            .collect();
        PathBuf::from(format!("merged{stems}.png"))
    }

    fn _hmerge_argv(
        inputs: &[PathBuf],
        output: &Path,
        overwrite: bool,
        align: Align,
        canvas_height: u32,
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
        argv.push(_hmerge_filter(inputs.len(), canvas_height, align).into());
        // A single image, not a sequence — without this, modern ffmpeg warns about the filename
        // lacking a `%03d`-style pattern.
        argv.push("-update".into());
        argv.push("1".into());
        argv.push(output.as_os_str().to_owned());
        argv
    }

    /// The merge filtergraph: every input is converted to one shared pixel format (`hstack`
    /// refuses mixed formats, e.g. RGBA png beside YUV jpg) and padded to the canvas height —
    /// keeping its own width, positioned by `align`, the filler transparent (black where the
    /// output format has no alpha) — then all the equal-height columns are stacked. `hstack` sums
    /// the widths itself, so only the height needs probing.
    fn _hmerge_filter(count: usize, canvas_height: u32, align: Align) -> String {
        let y = _align_expr(align);
        let columns: String = (0..count)
            .map(|i| format!("[{i}:v]format=rgba,pad=w=iw:h={canvas_height}:x=0:y={y}:color=black@0[p{i}];"))
            .collect();
        let refs: String = (0..count).map(|i| format!("[p{i}]")).collect();
        format!("{columns}{refs}hstack=inputs={count}")
    }

    /// The tallest input's pixel height — the merge canvas height. `None` (already reported) when
    /// any input can't be measured.
    fn _max_height(inputs: &[PathBuf]) -> Option<u32> {
        inputs.iter().map(|path| _image_height(path)).collect::<Option<Vec<_>>>()?.into_iter().max()
    }

    /// One image's pixel height, read via ffprobe (`None` after reporting, on any failure).
    fn _image_height(path: &Path) -> Option<u32> {
        let Some(out) = capture_stdout(tools::resolve("ffprobe"), _height_argv(path)) else {
            eprintln!("media_hmerge_imgs: could not measure {}", path.display());
            return None;
        };
        match out.trim().parse() {
            Ok(height) => Some(height),
            Err(_) => {
                eprintln!("media_hmerge_imgs: unexpected ffprobe height {out:?} for {}", path.display());
                None
            }
        }
    }

    /// ffprobe argv printing just an image's height, as a bare number.
    fn _height_argv(path: &Path) -> Vec<OsString> {
        let mut argv: Vec<OsString> =
            ["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=height", "-of", "csv=p=0"]
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
        fn conv_argv_orders_input_bitrate_then_output() {
            let args = ConvArgs {
                input: PathBuf::from("in.mov"),
                output: PathBuf::from("out.mp4"),
                bitrate: "2M".to_string(),
                overwrite: false,
            };
            assert_eq!(strs(&_conv_argv(&args)), ["-i", "in.mov", "-b:v", "2M", "out.mp4"]);
        }

        #[test]
        fn conv_argv_prepends_overwrite_flag() {
            let args = ConvArgs {
                input: PathBuf::from("in.mov"),
                output: PathBuf::from("out.mp4"),
                bitrate: "1M".to_string(),
                overwrite: true,
            };
            assert_eq!(strs(&_conv_argv(&args))[0], "-y");
        }

        #[test]
        fn default_merge_output_joins_input_stems() {
            let inputs = [PathBuf::from("a.png"), PathBuf::from("dir/b.jpg")];
            assert_eq!(_default_merge_output(&inputs), PathBuf::from("merged_a_b.png"));
        }

        #[test]
        fn hmerge_argv_has_one_input_flag_per_image_then_the_filtergraph() {
            let inputs = [PathBuf::from("a.png"), PathBuf::from("b.png")];
            let argv = strs(&_hmerge_argv(&inputs, Path::new("out.png"), false, Align::Top, 64));
            assert_eq!(argv[..4], ["-i", "a.png", "-i", "b.png"]);
            assert_eq!(argv[4], "-filter_complex");
            assert_eq!(argv[5], _hmerge_filter(2, 64, Align::Top));
            assert_eq!(argv[6..], ["-update", "1", "out.png"]);
        }

        #[test]
        fn hmerge_filter_pads_every_input_to_the_canvas_then_stacks_all() {
            // Three inputs → three pad columns and `inputs=3` (a bare `hstack` would silently
            // merge only the first two).
            let filter = _hmerge_filter(3, 1080, Align::Top);
            assert_eq!(filter.matches("pad=w=iw:h=1080:x=0:y=0").count(), 3, "{filter}");
            assert!(filter.contains("[2:v]format=rgba,"), "{filter}");
            assert!(filter.ends_with("[p0][p1][p2]hstack=inputs=3"), "{filter}");
        }

        #[test]
        fn hmerge_alignment_maps_to_pad_offsets() {
            assert!(_hmerge_filter(2, 100, Align::Top).contains(":y=0:"));
            assert!(_hmerge_filter(2, 100, Align::Middle).contains(":y=(oh-ih)/2:"));
            assert!(_hmerge_filter(2, 100, Align::Bottom).contains(":y=oh-ih:"));
        }

        #[test]
        fn height_argv_asks_for_a_bare_height_of_the_file() {
            let argv = strs(&_height_argv(Path::new("img.png")));
            assert_eq!(argv.last().unwrap(), "img.png");
            assert!(argv.contains(&"stream=height".to_string()));
            assert!(argv.contains(&"csv=p=0".to_string()), "bare number output: {argv:?}");
        }

        #[test]
        fn max_height_reports_and_bails_on_an_unreadable_input() {
            // Deterministic with or without ffprobe installed: a missing file fails the probe
            // either way, and the failure must abort the merge (None), not default to something.
            assert_eq!(_max_height(&[PathBuf::from("/no/such/image.png")]), None);
        }

        #[test]
        fn metadata_argv_ends_with_the_target_file() {
            let args = MetadataArgs { file: PathBuf::from("clip.mp4") };
            let argv = strs(&_metadata_argv(&args));
            assert_eq!(argv.last().unwrap(), "clip.mp4");
            assert!(argv.contains(&"-print_format".to_string()));
        }
    }
}
