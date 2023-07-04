// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use anyhow::Context;
use clap::{Parser, Subcommand};
use libdav::{auth::Auth, CardDavClient};

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

impl Server {
    async fn carddav_client(&self) -> anyhow::Result<CardDavClient> {
        let password = std::env::var("DAVCLI_PASSWORD")
            .context("failed to determine password")?
            .into();
        CardDavClient::builder()
            .with_uri(self.server_url.clone())
            .with_auth(Auth::Basic {
                username: self.username.clone(),
                password: Some(password),
            })
            .build()
            .auto_bootstrap()
            .await
            .map_err(anyhow::Error::from)
    }
}

impl CardDavArgs {
    #[tokio::main(flavor = "current_thread")]
    pub(crate) async fn execute(&self) -> anyhow::Result<()> {
        let client = self.server.carddav_client().await?;

        match self.command {
            CardDavCommand::Discover => discover(client),
        };

        Ok(())
    }
}

fn discover(client: CardDavClient) {
    println!("Discovery successful.");
    println!("- Context path: {}", &client.context_path());
    match client.addressbook_home_set {
        Some(home_set) => println!("- Address book home set: {home_set}"),
        None => println!("- Address book home set not found."),
    }
}
