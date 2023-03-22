//! Integration tests that require a live caldav server.
//! Run with: cargo test -- --ignored
//! Requires a few env vars.

use http::StatusCode;
use libdav::{
    auth::Auth,
    dav::{mime_types, CollectionType, DavError},
    CalDavClient,
};
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use std::fmt::Write;

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
async fn test_create_and_delete_collection() {
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
        .delete(&new_collection, "wrong-etag")
        .await
        .unwrap_err();

    // Delete the calendar
    caldav_client.delete(new_collection, etag).await.unwrap();

    let calendars = caldav_client
        .find_calendars(home_set.clone())
        .await
        .unwrap();
    let third_calendar_count = calendars.len();

    assert_eq!(orig_calendar_count, third_calendar_count);
}

fn minimal_icalendar() -> Vec<u8> {
    let mut entry = String::new();
    let uid = random_string(12);

    entry.push_str("BEGIN:VCALENDAR\r\n");
    entry.push_str("VERSION:2.0\r\n");
    entry.push_str("PRODID:-//hacksw/handcal//NONSGML v1.0//EN\r\n");
    entry.push_str("BEGIN:VEVENT\r\n");
    write!(entry, "UID:{uid}\r\n").unwrap();
    entry.push_str("DTSTAMP:19970610T172345Z\r\n");
    entry.push_str("DTSTART:19970714T170000Z\r\n");
    entry.push_str("SUMMARY:hello, testing\r\n");
    entry.push_str("END:VEVENT\r\n");
    entry.push_str("END:VCALENDAR\r\n");

    entry.into()
}

#[tokio::test]
#[ignore]
async fn test_create_and_delete_resource() {
    let caldav_client = create_test_client_from_env().await;
    let home_set = caldav_client.calendar_home_set.as_ref().unwrap().clone();

    let collection = format!("{}{}/", home_set.path(), &random_string(16));
    caldav_client
        .create_collection(&collection, CollectionType::Calendar)
        .await
        .unwrap();

    let resource = format!("{}{}.ics", collection, &random_string(12));
    let content = minimal_icalendar();

    caldav_client
        .create_resource(&resource, content.clone(), mime_types::CALENDAR)
        .await
        .unwrap();

    let items = caldav_client.list_resources(&collection).await.unwrap();
    assert_eq!(items.len(), 1);

    let updated_entry = String::from_utf8(content)
        .unwrap()
        .replace("hello", "goodbye")
        .as_bytes()
        .to_vec();

    // ASSERTION: deleting with a wrong etag fails.
    caldav_client
        .delete(&resource, "wrong-lol")
        .await
        .unwrap_err();

    // ASSERTION: creating conflicting resource fails.
    caldav_client
        .create_resource(&resource, updated_entry.clone(), mime_types::CALENDAR)
        .await
        .unwrap_err();

    // ASSERTION: item with matching href exists.
    let etag = items
        .into_iter()
        .find_map(|i| {
            if i.href == resource {
                Some(i.prop.etag)
            } else {
                None
            }
        })
        .unwrap()
        .unwrap();

    // ASSERTION: updating with wrong etag fails
    match caldav_client
        .update_resource(
            &resource,
            updated_entry.clone(),
            &resource,
            mime_types::CALENDAR,
        )
        .await
        .unwrap_err()
        .0
    {
        DavError::BadStatusCode(StatusCode::PRECONDITION_FAILED) => {}
        _ => panic!("updating entry with the wrong etag did not return the wrong error type"),
    }

    // ASSERTION: updating with correct etag work
    caldav_client
        .update_resource(&resource, updated_entry, &etag, mime_types::CALENDAR)
        .await
        .unwrap();

    // ASSERTION: deleting with outdated etag fails
    caldav_client.delete(&resource, &etag).await.unwrap_err();

    let items = caldav_client.list_resources(&collection).await.unwrap();
    assert_eq!(items.len(), 1);

    let etag = items
        .into_iter()
        .find_map(|i| {
            if i.href == resource {
                Some(i.prop.etag)
            } else {
                None
            }
        })
        .unwrap()
        .unwrap();

    // ASSERTION: deleting with correct etag works
    caldav_client.delete(&resource, &etag).await.unwrap();

    let items = caldav_client.list_resources(&collection).await.unwrap();
    assert_eq!(items.len(), 0);
}

#[tokio::test]
#[ignore]
async fn test_create_and_fetch_resource() {
    let caldav_client = create_test_client_from_env().await;
    let home_set = caldav_client.calendar_home_set.as_ref().unwrap().clone();

    let collection = format!("{}{}/", home_set.path(), &random_string(16));
    caldav_client
        .create_collection(&collection, CollectionType::Calendar)
        .await
        .unwrap();

    let resource = format!("{}{}.ics", collection, &random_string(12));
    caldav_client
        .create_resource(&resource, minimal_icalendar(), mime_types::CALENDAR)
        .await
        .unwrap();

    let items = caldav_client.list_resources(&collection).await.unwrap();
    assert_eq!(items.len(), 1);

    let fetched = caldav_client
        .get_resources(&collection, vec![&items[0].href])
        .await
        .unwrap();
    assert_eq!(fetched.len(), 1);

    // FIXME: some servers will fail here due to tampering PRODID
    // FIXME: order of lines may vary but items are still equivalent.
    // assert_eq!(
    //     fetched[0].data,
    //     String::from_utf8(minimal_icalendar()).unwrap()
    // );
}
