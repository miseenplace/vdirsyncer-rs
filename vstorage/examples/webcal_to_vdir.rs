//! This example copies all entries from a remote webcal storage into a local filesystem storage.
//! It DOES NOT synchronise items; it does a blind one-way copy.
//!
//! This is mostly a proof of concept of the basic storage implementations.
//!
//! Usage:
//!
//! ```
//! cargo run --example=webcal_to_vdir https://www.officeholidays.com/ics/netherlands /tmp/holidays
//! ```

use http::Uri;
use std::path::PathBuf;
use vstorage::base::Collection;
use vstorage::base::Definition;
use vstorage::base::Storage;
use vstorage::filesystem::FilesystemDefinition;
use vstorage::webcal::WebCalDefinition;

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args();
    arguments
        .next()
        .expect("binary has been called with a name");
    let raw_url = arguments.next().expect("$1 is a valid URL");
    let raw_path = arguments.next().expect("$2 is a valid path");

    let url = Uri::try_from(raw_url.as_str()).expect("provided URL must be valid");
    let path = PathBuf::from(raw_path);

    let webcal = WebCalDefinition {
        url,
        collection_name: String::from("holidays_nl"),
    }
    .storage()
    .await
    .expect("can create webcal storage");
    let mut fs = FilesystemDefinition {
        path,
        extension: String::from("ics"),
    }
    .storage()
    .await
    .expect("can create fs storage");

    let webcal_collection = webcal
        .open_collection("holidays_nl")
        .expect("can open webcal collection");
    let fs_collection = fs
        .create_collection("holidays_nl")
        .await
        .expect("can create fs collection");

    let copied = copy_collection(&webcal, webcal_collection, &mut fs, fs_collection).await;

    println!("Copied {copied} items");
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
