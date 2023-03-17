//! Integration tests that require a live caldav server.
//! Run with: cargo test -- --ignored
//! Requires a few env vars.

use libdav::{auth::Auth, dav::CollectionType, CalDavClient};
use rand::{distributions::Alphanumeric, thread_rng, Rng};

async fn create_test_client_from_env() -> CalDavClient {
    let server = std::env::var("CALDAV_SERVER").unwrap();
    let username = std::env::var("CALDAV_USERNAME").unwrap();
    let password = std::env::var("CALDAV_PASSWORD").unwrap();

    CalDavClient::auto_bootstrap(
        server.parse().unwrap(),
        Auth::Basic {
            username,
            password: Some(password),
        },
    )
    .await
    .unwrap()
}

fn random_string(len: usize) -> String {
    thread_rng()
        .sample_iter(Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

#[tokio::test]
#[ignore]
async fn test_create_and_delete() {
    let caldav_client = create_test_client_from_env().await;
    let home_set = caldav_client.calendar_home_set.as_ref().unwrap().clone();
    let calendars = caldav_client
        .find_calendars(home_set.clone())
        .await
        .unwrap();

    let orig_calendar_count = calendars.len();

    let new_collection = format!("{}{}/", home_set.path(), &random_string(16));
    caldav_client
        .create_collection(&new_collection, CollectionType::Calendar)
        .await
        .unwrap();

    let calendars = caldav_client
        .find_calendars(home_set.clone())
        .await
        .unwrap();
    let new_calendar_count = calendars.len();

    assert_eq!(orig_calendar_count + 1, new_calendar_count);

    // Get the etag of the newly created calendar:
    // ASSERTION: this validates that a collection with a matching href was created.
    let etag = caldav_client
        .find_calendars(home_set.clone())
        .await
        .unwrap()
        .into_iter()
        .find(|(href, _etag)| href == &new_collection)
        .unwrap()
        .1;

    // Try deleting with the wrong etag.
    caldav_client
        .delete_collection(&new_collection, "wrong-etag")
        .await
        .unwrap_err();

    // Delete the calendar
    caldav_client
        .delete_collection(new_collection, etag)
        .await
        .unwrap();

    let calendars = caldav_client
        .find_calendars(home_set.clone())
        .await
        .unwrap();
    let third_calendar_count = calendars.len();

    assert_eq!(orig_calendar_count, third_calendar_count);
}
