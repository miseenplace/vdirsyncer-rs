use anyhow::Context;
use clap::{Parser, Subcommand};
use libdav::CalDavClient;
use log::info;

use crate::cli::Server;

#[derive(Subcommand)]
pub(crate) enum CalDavCommand {
    /// Perform discovery and print results
    Discover,
    /// Find calendars under the calendar home set.
    FindCalendars,
    /// List calendar components under a given calendar collection.
    ListCalendarComponents { collection_href: String },
    /// Fetches a single calendar component.
    Get { resource_href: String },
}

#[derive(Parser)]
pub struct CalDavArgs {
    #[command(flatten)]
    pub(crate) server: Server,

    #[command(subcommand)]
    command: CalDavCommand,
}

impl CalDavArgs {
    #[tokio::main(flavor = "current_thread")]
    pub(crate) async fn execute(self) -> anyhow::Result<()> {
        let password = std::env::var("DAVCLI_PASSWORD").context("failed to determine password")?;
        let client = self.server.build_client(password).await?;

        match self.command {
            CalDavCommand::Discover => discover(client),
            CalDavCommand::FindCalendars => list_collections(client).await?,
            CalDavCommand::ListCalendarComponents { collection_href } => {
                list_resources(client, collection_href).await?
            }
            CalDavCommand::Get { resource_href } => get(client, resource_href).await?,
        };

        Ok(())
    }
}

fn discover(client: CalDavClient) {
    println!("Discovery successful.");
    println!("- Context path: {}", &client.context_path());
    match client.calendar_home_set {
        Some(home_set) => println!("- Calendar home set: {home_set}"),
        None => println!("- Calendar home set not found."),
    }
}

async fn get(client: CalDavClient, href: String) -> anyhow::Result<()> {
    let target_url = client
        .calendar_home_set
        .as_ref()
        .context("No calendar home set available")?
        .to_string();

    let response = client
        .get_resources(target_url, &[href])
        .await?
        .into_iter()
        .next()
        .context("Server returned a response with no resources")?;

    let raw = &response
        .content
        .as_ref()
        .map_err(|code| anyhow::anyhow!("Server returned error code: {0}", code))?
        .data;

    println!("{raw}");

    Ok(())
}

async fn list_collections(client: CalDavClient) -> anyhow::Result<()> {
    let response = client.find_calendars(None).await?;
    for collection in response {
        println!("{}", collection.href);
    }

    Ok(())
}

async fn list_resources(client: CalDavClient, href: String) -> anyhow::Result<()> {
    let resources = client.list_resources(&href).await?;
    if resources.is_empty() {
        info!("No items in collection");
    } else {
        for resource in resources {
            println!("{}", resource.href);
        }
    }

    Ok(())
}
