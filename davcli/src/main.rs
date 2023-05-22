#![deny(clippy::pedantic)]

use clap::Parser;

mod caldav;
mod carddav;
mod cli;

fn main() -> anyhow::Result<()> {
    // TODO: also support email as input?
    let cli = cli::Cli::parse();
    simple_logger::init_with_level(cli.log_level()).expect("logger configuration is valid");

    cli.execute()
}
