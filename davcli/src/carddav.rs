use clap::{Parser, Subcommand};

use crate::cli::Server;

#[derive(Parser)]
pub struct CardDavArgs {
    #[command(flatten)]
    pub(crate) server: Server,

    #[command(subcommand)]
    command: CardDavCommand,
}

#[derive(Subcommand)]
pub(crate) enum CardDavCommand {
    /// Perform discovery and print results
    Discover,
}

impl CardDavArgs {
    pub(crate) fn execute(&self) -> anyhow::Result<()> {
        todo!()
    }
}
