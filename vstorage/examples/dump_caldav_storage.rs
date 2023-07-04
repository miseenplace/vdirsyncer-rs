// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use libdav::auth::Auth;
use vstorage::{
    base::{Collection, Definition, IcsItem, Storage},
    caldav::CalDavDefinition,
    filesystem::FilesystemDefinition,
};

async fn create_caldav_from_env() -> Box<dyn Storage<IcsItem>> {
    let server = std::env::var("CALDAV_SERVER").unwrap();
    let username = std::env::var("CALDAV_USERNAME").unwrap();
    let password = std::env::var("CALDAV_PASSWORD").unwrap().into();

    CalDavDefinition {
        url: server.parse().unwrap(),
        auth: Auth::Basic {
            username,
            password: Some(password),
        },
    }
    .storage()
    .await
    .unwrap()
}

async fn create_vdir_from_env() -> Box<dyn Storage<IcsItem>> {
    let path = std::env::var("VDIR_PATH").unwrap();
    FilesystemDefinition::new(path.try_into().unwrap(), "ics".to_string())
        .storage()
        .await
        .unwrap()
}
#[tokio::main]
async fn main() {
    let caldav_storage = create_caldav_from_env().await;
    let mut vdir_storage = create_vdir_from_env().await;

    let collections = caldav_storage.discover_collections().await.unwrap();

    println!("Found {} collections", collections.len());
    for collection in collections {
        println!("Creating {}", collection.href());
        let collection_name = collection
            .href()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .expect("collection has at least one path segument");
        let new_collection = vdir_storage
            .create_collection(collection_name)
            .await
            .unwrap();

        copy_collection(
            &caldav_storage,
            collection,
            &mut vdir_storage,
            new_collection,
        )
        .await;
    }
}

/// Copies from `source` to `target` and returns the amount of items copied.
async fn copy_collection(
    source_storage: &Box<dyn Storage<IcsItem>>,
    source_collection: Collection,
    target_storage: &mut Box<dyn Storage<IcsItem>>,
    target_collection: Collection,
) -> usize {
    let mut count = 0;
    for (_href, item, _etag) in source_storage
        .get_all_items(&source_collection)
        .await
        .expect("webcal remote has items")
    {
        count += 1;
        target_storage
            .add_item(&target_collection, &item)
            .await
            .expect("write to local filesystem collection");
    }

    count
}
