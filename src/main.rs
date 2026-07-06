use bashrs::cli::Cli;
use clap::Parser;

fn main() {
    // Rust ignores SIGPIPE at startup, so writing to a closed pipe — `bashrs lll | head`,
    // or quitting a pager mid-output — makes `print!`/`println!` panic with "Broken pipe".
    // Restore the default disposition so we exit quietly on SIGPIPE instead, as `ls`/`grep`
    // do. SAFETY: called once at startup before any threads exist; it only sets a handler.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    Cli::parse().command.run();
}
