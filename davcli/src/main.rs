use anyhow::Context;
use clap::Parser;
use libdav::CalDavClient;

mod cli;

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
        println!("Found calendar: {}", collection.href);
    }

    Ok(())
}

async fn list_resources(client: CalDavClient, href: String) -> anyhow::Result<()> {
    let resources = client.list_resources(&href).await?;
    if resources.is_empty() {
        println!("No items in collection");
    } else {
        for resource in resources {
            println!("Found item: {}", resource.href);
        }
    }

    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // TODO: also support email as input?
    let cli = cli::Cli::parse();
    let password = std::env::var("DAVCLI_PASSWORD").context("failed to determine password")?;
    let client = cli.server.build_client(password).await?;

    match cli.command {
        cli::Command::Discover => discover(client),
        cli::Command::Get { href } => get(client, href).await?,
        cli::Command::ListCollections => list_collections(client).await?,
        cli::Command::ListResources { href } => list_resources(client, href).await?,
    }

    Ok(())
}
