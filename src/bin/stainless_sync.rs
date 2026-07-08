//! Compile-time companion binary: retrieve/update the non-Rust ("stainless") repos and cache each
//! tool's `--help` description, so the main binary's `generate` can alias them with a comment.
//!
//! Built by `cargo build` but *run by `COMPILE.sh`* (and never installed under `~/.bashrs`), so repo
//! fetching is never wired into `cargo build` / `build.rs`. Best-effort: [`sync`] only warns on
//! failure, so a missing network or venv can't abort the compile.
//!
//! [`sync`]: bashrs::shell_conf::stainless::sync

fn main() {
    bashrs::shell_conf::stainless::sync();
}
