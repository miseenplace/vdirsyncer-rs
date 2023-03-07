//! An example of some basic usage of the `CalDavClient` type.
//!
//! Usage:
//!
//!     cargo run --example=find_calendars https://example.com user@example.com MYPASSWORD
//!     cargo run --example=find_calendars $SERVER_URL         $USERNAME        $PASSWORD
//!
//! Example output (with $1 = "https://fastmail.com"):
//!
//! ```
//! Resolved server URL to: https://d277161.caldav.fastmail.com/dav/calendars
//! found 1 calendars...
//! 📅 name: Some("Calendar"), colour: Some("#3a429c"), path: "/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/"
//! Href and Etag for components in calendar:
//! - /dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/395b00a0-eebc-40fd-a98e-176a06367c82.ics, "e7577ff2b0924fe8e9a91d3fb2eb9072598bf9fb"
//! ```
use http::Uri;
use vcaldav::auth::Auth;
use vcaldav::CalDavClient;

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

    let caldav_client = CalDavClient::auto_bootstrap(
        base_url,
        Auth::Basic {
            username,
            password: Some(password),
        },
    )
    .await
    .unwrap();

    println!("Resolved server URL to: {}", caldav_client.context_path());

    let home_set = caldav_client.calendar_home_set.as_ref().unwrap().clone();
    let calendars = caldav_client.find_calendars(home_set).await.unwrap();

    println!("found {} calendars...", calendars.len());

    for ref calendar in calendars {
        let name = caldav_client
            .get_calendar_displayname(calendar)
            .await
            .unwrap();
        let color = caldav_client.get_calendar_colour(calendar).await.unwrap();
        println!("📅 name: {name:?}, colour: {color:?}, path: {calendar:?}");
        let items = caldav_client
            .list_collection(calendar)
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.unwrap())
            .filter(|i| !i.prop.is_collection);
        println!("Href and Etag for components in calendar:");
        for item in items {
            println!("- {}, {}", item.href, item.prop.etag);
        }
    }
}
