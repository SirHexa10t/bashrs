#[bashrs_macros::category(command = FilesystemCommand, prefix = "fs_")]
mod commands {
    use clap::Args;
    use std::fs;
    use std::path::PathBuf;

    /// List directory contents
    #[prefixed]
    #[unprefixed]
    pub fn lss(args: LsArgs) {
        let entries = match fs::read_dir(&args.path) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("lss: {}: {}", args.path.display(), err);
                return;
            }
        };

        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let name = entry.file_name();
            let name = name.to_string_lossy();

            if !args.all && name.starts_with('.') {
                continue;
            }

            if args.sizes {
                let size = entry
                    .metadata()
                    .map(|meta| _human_size(meta.len()))
                    .unwrap_or_else(|_| "?".to_string());
                println!("{size:>8}  {name}");
            } else {
                println!("{name}");
            }
        }
    }

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

    /// Format a byte count as a short human-readable string (e.g. `1.5K`, `3.2M`).
    /// Units saturate at terabytes; sub-kilobyte values are printed as exact bytes.
    fn _human_size(bytes: u64) -> String {
        const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
        let mut size = bytes as f64;
        let mut unit = 0;
        while size >= 1024.0 && unit < UNITS.len() - 1 {
            size /= 1024.0;
            unit += 1;
        }
        if unit == 0 {
            format!("{bytes}{}", UNITS[unit])
        } else {
            format!("{size:.1}{}", UNITS[unit])
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sub_kilobyte_values_are_exact_bytes() {
            assert_eq!(_human_size(0), "0B");
            assert_eq!(_human_size(1), "1B");
            assert_eq!(_human_size(1023), "1023B");
        }

        #[test]
        fn units_promote_at_1024_boundaries() {
            assert_eq!(_human_size(1024), "1.0K");
            assert_eq!(_human_size(1536), "1.5K");
            assert_eq!(_human_size(1024 * 1024), "1.0M");
            assert_eq!(_human_size(1024 * 1024 * 1024), "1.0G");
            assert_eq!(_human_size(1024_u64.pow(4)), "1.0T");
        }

        #[test]
        fn units_saturate_at_terabytes_without_panicking() {
            // u64::MAX is far past a terabyte; the unit must clamp at "T".
            let formatted = _human_size(u64::MAX);
            assert!(formatted.ends_with('T'), "expected T suffix, got {formatted}");
        }
    }
}
