use std::io::Write;

use anyhow::{bail, Context};
use clap::Parser;
use http::Uri;
use libdav::{auth::Auth, CalDavClient};
use termion::input::TermRead;

mod cli;

/// Returns `(username, password)`
fn prompt_for_password() -> anyhow::Result<String> {
    let mut stdout = std::io::stdout().lock();
    let mut stdin = std::io::stdin().lock();

    let password = loop {
        stdout.write_all(b"password: ")?;
        stdout.flush()?;
        if let Some(pwd) = stdin.read_passwd(&mut stdout)? {
            break pwd;
        };
    };
    stdout.write_all(b"\n")?;
    stdout.flush()?;

    Ok(password)
}

fn get_password() -> anyhow::Result<String> {
    match std::env::var("DAVCLI_PASSWORD") {
        Ok(pwd) => Ok(pwd),
        Err(std::env::VarError::NotPresent) => prompt_for_password(),
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("DAVCLI_PASSWORD is not unicode; unsupported")
        }
    }
}

async fn discover(base_uri: String, username: String) -> anyhow::Result<()> {
    let parsed_uri: Uri = base_uri.parse().context("failed to parse base_uri")?;
    let password = get_password().context("failed to determine password")?;

    let caldav_client = CalDavClient::builder()
        .with_uri(parsed_uri)
        .with_auth(Auth::Basic {
            username,
            password: Some(password),
        })
        .build()
        .auto_bootstrap()
        .await?;

    println!("Discovery successful.");
    println!("- Context path: {}", &caldav_client.context_path());
    match caldav_client.calendar_home_set {
        Some(home_set) => println!("- Calendar home set: {}", home_set),
        None => println!("- Calendar home set not found."),
    }

    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    match cli.command {
        // TODO: also support email as input?
        cli::Command::Discover { base_uri, username } => discover(base_uri, username).await?,
    }

    Ok(())
}
