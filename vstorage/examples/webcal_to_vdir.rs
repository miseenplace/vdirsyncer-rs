//! This example copies all entries from a remote webcal storage into a local filesystem storage.
//! It DOES NOT synchronise items; it does a blind one-way copy.
//!
//! This is mostly a proof of concept of the basic storage implementations.

use std::path::PathBuf;
use url::Url;
use vstorage::base::Collection;
use vstorage::base::Storage;
use vstorage::filesystem::FilesystemDefinition;
use vstorage::filesystem::FilesystemStorage;
use vstorage::webcal::WebCalDefinition;
use vstorage::webcal::WebCalStorage;

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args();
    arguments
        .next()
        .expect("binary has been called with a name");
    let raw_url = arguments.next().expect("$1 is a valid URL");
    let raw_path = arguments.next().expect("$2 is a valid path");

    let url = Url::parse(raw_url.as_str()).expect("provided URL must be valid");
    let path = PathBuf::from(raw_path);

    let webcal = WebCalStorage::new(WebCalDefinition {
        url,
        collection_name: String::from("holidays_nl"),
    })
    .expect("can create webcal storage");
    let mut fs = FilesystemStorage::new(FilesystemDefinition {
        path,
        extension: String::from("ics"),
    })
    .expect("can create fs storage");

    let webcal_collection = webcal
        .open_collection("holidays_nl")
        .expect("can open webcal collection");
    let mut fs_collection = fs
        .create_collection("holidays_nl")
        .await
        .expect("can create fs collection");

    for (_href, item, _etag) in webcal_collection
        .get_all()
        .await
        .expect("webcal remote has items")
    {
        fs_collection
            .add(&item)
            .await
            .expect("write to local filesystem collection");
    }

    let count = fs_collection
        .list()
        .await
        .expect("list items in filesystem collection")
        .len();

    println!("Copied {count} items");
}
