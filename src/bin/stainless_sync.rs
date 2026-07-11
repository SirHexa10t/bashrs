//! Compile-time companion binary: retrieve/update the non-Rust ("stainless") repos, and bundle the
//! self-contained external tools (`~/.bashrs/tools/` — ffmpeg, python) for machines that lack them.
//!
//! Built by `cargo build` but *run by `COMPILE.SH`* (and never installed under `~/.bashrs`), so
//! fetching is never wired into `cargo build` / `build.rs`. Best-effort throughout: each sync only
//! warns on failure, so a missing network can't abort the compile.

fn main() {
    // The configuration's shape first (archiving an outdated file) — the syncs read it.
    if let Err(err) = bashrs::conf::config_file::ensure_current() {
        eprintln!("bashrs: could not prepare the configuration: {err}");
    }
    // Tools before repos: the companion repos' python dependencies install into the bundled
    // environment, so python and uv must already be in place when the repos sync.
    bashrs::tools::sync();
    bashrs::tools::stainless::sync();
}
