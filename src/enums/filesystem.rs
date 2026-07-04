use clap::Subcommand;
use strum_macros::{IntoStaticStr, VariantNames};
use super::Run;
use crate::modules::filesystem as fs;

#[derive(Subcommand, IntoStaticStr, VariantNames)]
pub enum FilesystemCommand {
    /// List directory contents
    #[strum(serialize = "lss")]
    Lss(fs::LsArgs),
}

impl Run for FilesystemCommand {
    fn run(self) {
        match self {
            FilesystemCommand::Lss(args) => fs::lss(args),
        }
    }
}