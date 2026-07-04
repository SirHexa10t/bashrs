pub mod filesystem;
pub mod media;

use clap::Subcommand;
use enum_dispatch::enum_dispatch;
use strum::VariantNames;
use strum_macros::VariantNames as VariantNamesMacro;
use filesystem::FilesystemCommand;
use media::MediaCommand;

pub trait Run {
    fn run(self);
}

#[enum_dispatch(Run)]
#[derive(Subcommand, VariantNamesMacro)]
pub enum Command {
    #[command(flatten)]
    Filesystem(FilesystemCommand),
    #[command(flatten)]
    Media(MediaCommand),
}

impl Command {
    pub fn all_names() -> Vec<&'static str> {
        FilesystemCommand::VARIANTS
            .iter()
            .chain(MediaCommand::VARIANTS)
            .copied()
            .collect()
    }
}