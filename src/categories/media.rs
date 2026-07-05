#[bashrs_macros::category(command = MediaCommand, prefix = "media_")]
mod commands {
    use crate::support::exec::run_reporting;
    use clap::Args;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    // --- media_conv -----------------------------------------------------------

    /// Convert a media file using ffmpeg
    pub fn conv(args: ConvArgs) {
        if run_reporting("ffmpeg", _conv_argv(&args)) {
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
        run_reporting("ffprobe", _metadata_argv(&args)); // ffprobe prints the report itself
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

    /// Merge images horizontally into a single image
    pub fn hmerge_imgs(args: HmergeImgsArgs) {
        let HmergeImgsArgs { output, overwrite, inputs } = args;
        let output = output.unwrap_or_else(|| _default_merge_output(&inputs));
        if run_reporting("ffmpeg", _hmerge_argv(&inputs, &output, overwrite)) {
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
        /// Input images (at least 2 required)
        #[arg(required = true, num_args = 2..)]
        pub inputs: Vec<PathBuf>,
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

    fn _hmerge_argv(inputs: &[PathBuf], output: &Path, overwrite: bool) -> Vec<OsString> {
        let mut argv: Vec<OsString> = Vec::new();
        if overwrite {
            argv.push("-y".into());
        }
        for input in inputs {
            argv.push("-i".into());
            argv.push(input.clone().into());
        }
        argv.push("-filter_complex".into());
        argv.push("hstack".into());
        argv.push(output.as_os_str().to_owned());
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
        fn hmerge_argv_has_one_input_flag_per_image_then_hstack() {
            let inputs = [PathBuf::from("a.png"), PathBuf::from("b.png")];
            assert_eq!(
                strs(&_hmerge_argv(&inputs, Path::new("out.png"), false)),
                ["-i", "a.png", "-i", "b.png", "-filter_complex", "hstack", "out.png"],
            );
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
