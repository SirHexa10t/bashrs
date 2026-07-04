use clap::Subcommand;
use strum_macros::{IntoStaticStr, VariantNames};
use super::Run;
use crate::modules::media as m;

#[derive(Subcommand, IntoStaticStr, VariantNames)]
pub enum MediaCommand {
    /// Convert a media file using ffmpeg
    #[strum(serialize = "m_conv")]
    MConv(m::MConvArgs),
    /// Get metadata of an audio/video/image file
    #[strum(serialize = "media_metadata")]
    MediaMetadata(m::MediaMetadataArgs),
    /// Merge images horizontally into a single image
    #[strum(serialize = "media_hmerge_imgs")]
    MediaHmergeImgs(m::MediaHmergeImgsArgs),
}

impl Run for MediaCommand {
    fn run(self) {
        match self {
            MediaCommand::MConv(args)           => m::m_conv(args),
            MediaCommand::MediaMetadata(args)   => m::media_metadata(args),
            MediaCommand::MediaHmergeImgs(args) => m::media_hmerge_imgs(args),
        }
    }
}