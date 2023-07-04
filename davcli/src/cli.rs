// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use clap::{Args, Parser, Subcommand, ValueEnum};
use http::Uri;

use crate::{caldav::CalDavArgs, carddav::CardDavArgs};

#[derive(Clone, ValueEnum)]
enum Verbosity {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Args)]
pub(crate) struct Server {
    /// A base URL from which to discover the server.
    ///
    /// Examples: `http://localhost:8080`, `https://example.com`.
    #[arg(long)]
    pub(crate) server_url: Uri,

    /// Username for authentication.
    #[arg(long)]
    pub(crate) username: String,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Operate on a CalDav server.
    Caldav(CalDavArgs),
    /// Operate on a CardDav server.
    Carddav(CardDavArgs),
}

#[derive(Parser)]
#[clap(author, version = env!("DAVCLI_VERSION"), about, long_about = None)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,

    /// Change logging verbosity
    ///
    /// Logging is always directed to `stderr`.
    #[clap(short, long)]
    verbose: Option<Verbosity>,
}

impl Cli {
    pub(crate) fn execute(self) -> anyhow::Result<()> {
        match self.command {
            Command::Caldav(cmd) => cmd.execute(),
            Command::Carddav(cmd) => cmd.execute(),
        }
    }

    /// Returns the desired log level based on the amount of `-v` flags.
    /// The default log level is WARN.
    pub(crate) fn log_level(&self) -> log::Level {
        match self.verbose {
            Some(Verbosity::Error) => log::Level::Error,
            Some(Verbosity::Warn) | None => log::Level::Warn,
            Some(Verbosity::Info) => log::Level::Info,
            Some(Verbosity::Debug) => log::Level::Debug,
            Some(Verbosity::Trace) => log::Level::Trace,
        }
    }
}

#[test]
fn verify_cli() {
    use clap::CommandFactory;
    Cli::command().debug_assert()
}
