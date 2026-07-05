//! Argument structs shared across command categories.

use clap::Args;

/// An empty argument set, for commands that take no arguments.
#[derive(Args)]
pub struct NoArgs {}
