use clap::Args;
use std::fs;
use std::path::PathBuf;

#[derive(Args)]
pub struct LsArgs {
    /// Directory to list (defaults to current directory)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Show hidden files
    #[arg(short, long)]
    pub all: bool,

    /// Show file sizes in human-readable format
    #[arg(short = 's', long)]
    pub sizes: bool,
}

pub fn lss(args: LsArgs) {
    let entries = match fs::read_dir(&args.path) {
        Ok(e) => e,
        Err(e) => { eprintln!("ls: {}: {}", args.path.display(), e); return; }
    };

    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if !args.all && name.starts_with('.') {
            continue;
        }

        if args.sizes {
            let size = entry.metadata()
                .map(|m| human_size(m.len()))
                .unwrap_or_else(|_| "?".to_string());
            println!("{:>8}  {}", size, name);
        } else {
            println!("{}", name);
        }
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{}{}", bytes, UNITS[unit])
    } else {
        format!("{:.1}{}", size, UNITS[unit])
    }
}