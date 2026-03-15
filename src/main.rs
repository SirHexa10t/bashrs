mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "bashrs", about = "Shell utilities")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print a greeting
    Hello {
        #[arg(default_value = "World")]
        name: String,
    },
    /// Print export statement for HISTTIMEFORMAT
    Histfmt {
        #[arg(default_value = "%F_%T  ")]
        fmt: String,
    },
}

fn main() {
    match Cli::parse().command {
        Command::Hello { name }  => println!("Hello, {}!", name),
        Command::Histfmt { fmt } => println!("export HISTTIMEFORMAT=\"{}\"", fmt),
    }
}