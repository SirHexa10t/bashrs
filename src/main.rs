use bashrs::cli::Cli;
use clap::Parser;

fn main() {
    Cli::parse().command.run();
}
