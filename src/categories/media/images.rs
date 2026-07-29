//! Compose image files onto one canvas — side by side (`media_hmerge_imgs`) or stacked
//! (`media_vmerge_imgs`), each padding every input to the canvas on its cross axis.

#[bashrs_macros::category(command = MediaImagesCommand, prefix = "media_")]
mod commands {
    use crate::support::exec::{capture_stdout, run_reporting_code};
    use crate::tools;
    use clap::{Args, ValueEnum};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use crate::categories::media::_report_saved;
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::categories::media::strs;

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
