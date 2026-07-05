//! Commands backed by my other repos, imported as dependencies. Each is exposed
//! under the external tool's own name (so they're `#[unprefixed]`), and reuses
//! that project's own argument struct — the args are defined once, upstream.

#[bashrs_macros::category(command = ComfyReposCommand, prefix = "comfy_")]
mod commands {
    /// Align whitespace-delimited columns into a neat table (table_formatter)
    #[unprefixed]
    pub fn table(args: table_formatter::Args) {
        if let Err(err) = table_formatter::run_with(args) {
            eprintln!("table: {err}");
        }
    }
}
