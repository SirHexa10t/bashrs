use clap::Args;
use std::path::PathBuf;
use std::process::Command;

// --- m_conv ---

#[derive(Args)]
pub struct MConvArgs {
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

pub fn m_conv(args: MConvArgs) {
    let mut cmd = Command::new("ffmpeg");
    if args.overwrite { cmd.arg("-y"); }
    cmd.arg("-i").arg(&args.input)
        .arg("-b:v").arg(&args.bitrate)
        .arg(&args.output);
    match cmd.status() {
        Ok(s) if s.success() => println!("Done: {}", args.output.display()),
        Ok(s)  => eprintln!("ffmpeg exited with status: {}", s),
        Err(e) => eprintln!("Failed to run ffmpeg: {}", e),
    }
}

// --- media_metadata ---

#[derive(Args)]
pub struct MediaMetadataArgs {
    /// File to inspect (audio, video, or image)
    pub file: PathBuf,
}

pub fn media_metadata(args: MediaMetadataArgs) {
    let status = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "stream=width,height,r_frame_rate,nb_frames,codec_name,channels,bit_rate,duration",
            "-show_entries", "format=filename,size",
            "-hide_banner",
            "-pretty",
            "-print_format", "json",
        ])
        .arg(&args.file)
        .status();
    match status {
        Ok(s) if !s.success() => eprintln!("ffprobe failed with status: {}", s),
        Err(e) => eprintln!("Failed to run ffprobe: {}", e),
        _ => {}
    }
}

// --- media_hmerge_imgs ---

#[derive(Args)]
pub struct MediaHmergeImgsArgs {
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

pub fn media_hmerge_imgs(args: MediaHmergeImgsArgs) {
    let output = args.output.unwrap_or_else(|| {
        let names: String = args.inputs.iter()
            .map(|p| format!("_{}", p.file_stem().unwrap_or_default().to_string_lossy()))
            .collect();
        PathBuf::from(format!("merged{}.png", names))
    });
    let mut cmd = Command::new("ffmpeg");
    if args.overwrite { cmd.arg("-y"); }
    for input in &args.inputs {
        cmd.arg("-i").arg(input);
    }
    cmd.args(["-filter_complex", "hstack"]).arg(&output);
    match cmd.status() {
        Ok(s) if s.success() => {
            let path = std::fs::canonicalize(&output).unwrap_or_else(|_| output.clone());
            println!("Saved merged image: {}", path.display());
        }
        Ok(s)  => eprintln!("ffmpeg failed with status: {}", s),
        Err(e) => eprintln!("Failed to run ffmpeg: {}", e),
    }
}