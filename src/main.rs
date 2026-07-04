use bashrs::categories::Cli;
use clap::Parser;

fn main() {
    Cli::parse().command.run();
}
