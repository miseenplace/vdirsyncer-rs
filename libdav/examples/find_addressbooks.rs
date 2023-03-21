//! An example of some basic usage of the `CardDavClient` type.
//!
//! Usage:
//!
//!     cargo run --example=find_addressbooks https://example.com user@example.com MYPASSWORD
//!     cargo run --example=find_addressbooks $SERVER_URL         $USERNAME        $PASSWORD
//!
//! Example output (with $1 = "https://fastmail.com"):
//!
//! ```
//! Resolved server URL to: https://d277161.carddav.fastmail.com/dav/addressbooks
//! found 2 addressbooks...
//! 📇 name: Some("Personal"), path: "/dav/addressbooks/user/vdirsyncer@fastmail.com/Default/"
//! Href and Etag for components in addressbook:
//! 📇 name: Some("test-vdirsyncer-ci-bedd62c5-ede3-4e07-87c0-163c259c634f"), path: "/dav/addressbooks/user/vdirsyncer@fastmail.com/test-vdirsyncer-ci-bedd62c5-ede3-4e07-87c0-163c259c634f/"
//! Href and Etag for components in addressbook:
//! ```
use http::Uri;
use libdav::auth::Auth;
use libdav::CardDavClient;

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args();
    arguments
        .next()
        .expect("binary has been called with a name");
    let base_url: Uri = arguments
        .next()
        .expect("$1 is defined")
        .parse()
        .expect("$1 is a valid URL");
    let username = arguments.next().expect("$2 is a valid username");
    let password = arguments.next().expect("$3 is a valid password");

    let carddav_client = CardDavClient::auto_bootstrap(
        base_url,
        Auth::Basic {
            username,
            password: Some(password),
        },
    )
    .await
    .unwrap();

    println!("Resolved server URL to: {}", carddav_client.context_path());

    let home_set = carddav_client
        .addressbook_home_set
        .as_ref()
        .unwrap()
        .clone();
    let addressbooks = carddav_client.find_addresbooks(home_set).await.unwrap();

    println!("found {} addressbooks...", addressbooks.len());

    for (ref addressbook, ref etag) in addressbooks {
        let name = carddav_client
            .get_collection_displayname(addressbook)
            .await
            .unwrap();
        println!("📇 name: {name:?}, path: {addressbook:?}, etag: {etag}");
        let items = carddav_client
            .list_resources(addressbook)
            .await
            .unwrap()
            .into_iter()
            .filter(|i| !i.prop.is_collection);
        println!("Href and Etag for components in addressbook:");
        for item in items {
            println!("- {}, {}", item.href, item.prop.etag);
        }
    }
}
