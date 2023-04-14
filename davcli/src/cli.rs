use clap::{Parser, Subcommand, ValueEnum};

#[derive(Clone, ValueEnum)]
enum Verbosity {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Perform discovery and print results
    Discover {
        // TODO: flag to specify caldav/carddav
        base_uri: String,
        username: String,
    },
}

#[derive(Parser)]
#[clap(author, version = env!("DAVCLI_VERSION"), about, long_about = None)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,

    /// Change logging verbosity
    #[clap(short, long)]
    verbose: Option<Verbosity>,
}

impl Cli {
    /// Returns the desired log level based on the amount of `-v` flags.
    /// The default log level is WARN.
    pub(crate) fn log_level(&self) -> log::Level {
        match self.verbose {
            Some(Verbosity::Error) => log::Level::Error,
            Some(Verbosity::Warn) => log::Level::Warn,
            Some(Verbosity::Info) => log::Level::Info,
            Some(Verbosity::Debug) => log::Level::Debug,
            Some(Verbosity::Trace) => log::Level::Trace,
            None => log::Level::Warn,
        }
    }
}
