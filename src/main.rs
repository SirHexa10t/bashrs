use clap::Parser;
use bashrs::enums::{Command, Run};

#[derive(Parser)]
#[command(name = "bashrs", about = "Rust-based bashrc")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

fn main() {
    Cli::parse().command.run();
}