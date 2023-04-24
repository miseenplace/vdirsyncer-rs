use libdav::auth::Auth;
use vstorage::{
    base::{Collection, Definition, Storage},
    caldav::CalDavDefinition,
    filesystem::FilesystemDefinition,
};

async fn create_caldav_from_env() -> Box<dyn Storage> {
    let server = std::env::var("CALDAV_SERVER").unwrap();
    let username = std::env::var("CALDAV_USERNAME").unwrap();
    let password = std::env::var("CALDAV_PASSWORD").unwrap();

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

async fn create_vdir_from_env() -> Box<dyn Storage> {
    let path = std::env::var("VDIR_PATH").unwrap();
    FilesystemDefinition {
        path: path.try_into().unwrap(),
        extension: "ics".to_string(),
    }
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

        copy_collection(&caldav_storage, collection, &mut vdir_storage, new_collection).await;
    }
}

/// Copies from `source` to `target` and returns the amount of items copied.
///
/// NOTE: This function serves an extra purpose: the validates that the `Collection` trait is
/// object safe and works well when used in such way.
async fn copy_collection(
    source_storage: &Box<dyn Storage>,
    source_collection: Collection,
    target_storage: &mut Box<dyn Storage>,
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
