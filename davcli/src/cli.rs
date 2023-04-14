use clap::{builder::PossibleValue, Args, Parser, Subcommand, ValueEnum};
use http::Uri;

#[derive(Clone, ValueEnum)]
enum Verbosity {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
#[derive(Clone, Default)]
pub(crate) enum DavType {
    #[default]
    CalDav,
    CardDav,
}

impl clap::ValueEnum for DavType {
    fn value_variants<'a>() -> &'a [Self] {
        &[DavType::CalDav, DavType::CardDav]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        match self {
            DavType::CalDav => Some(PossibleValue::new("caldav")),
            DavType::CardDav => Some(PossibleValue::new("carddav")),
        }
    }
}

#[derive(Args)]
pub(crate) struct ServerDetails {
    /// A base URL from which to discover the server.
    ///
    /// Examples: `http://localhost:8080`, `https://example.com`.
    #[arg(long)]
    pub(crate) base_uri: Uri,

    /// Username for authentication.
    #[arg(long)]
    pub(crate) username: String,

    /// Server type.
    #[arg(long, value_enum, default_value_t)]
    pub(crate) server_type: DavType,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Perform discovery and print results
    Discover {},
}

#[derive(Parser)]
#[clap(author, version = env!("DAVCLI_VERSION"), about, long_about = None)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) server: ServerDetails,

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

#[test]
fn verify_cli() {
    use clap::CommandFactory;
    Cli::command().debug_assert()
}
