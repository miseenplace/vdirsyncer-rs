//! Integration tests that require a live caldav server.
//! Run with: cargo test -- --ignored
//! Requires a few env vars.

use http::StatusCode;
use libdav::{
    auth::Auth,
    dav::{CollectionType, DavError},
    CalDavClient,
};
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

const MINIMAL_ICALENDAR: &[u8] = br#"BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//hacksw/handcal//NONSGML v1.0//EN
BEGIN:VEVENT
UID:19970610T172345Z-AF23B2@example.com
DTSTAMP:19970610T172345Z
DTSTART:19970714T170000Z
END:VEVENT
END:VCALENDAR"#;

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
    caldav_client
        .create_resource(&resource, MINIMAL_ICALENDAR.to_vec())
        .await
        .unwrap();

    let items = caldav_client.list_resources(&collection).await.unwrap();
    assert_eq!(items.len(), 1);

    // ASSERTION: deleting with a wrong etag fails.
    caldav_client
        .delete(&resource, "wrong-lol")
        .await
        .unwrap_err();

    // ASSERTION: creating conflicting resource fails.
    caldav_client
        .create_resource(&resource, MINIMAL_ICALENDAR.to_vec())
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
        .unwrap();

    let updated_entry = String::from_utf8(MINIMAL_ICALENDAR.to_vec())
        .unwrap()
        .replace("1997", "2023")
        .as_bytes()
        .to_vec();

    // ASSERTION: updating with wrong etag fails
    match caldav_client
        .update_resource(&resource, updated_entry.clone(), &resource)
        .await
        .unwrap_err()
        .0
    {
        DavError::BadStatusCode(StatusCode::PRECONDITION_FAILED) => {}
        _ => panic!("updating entry with the wrong etag did not return the wrong error type"),
    }

    // ASSERTION: updating with correct etag work
    caldav_client
        .update_resource(&resource, updated_entry, &etag)
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
        .unwrap();

    // ASSERTION: deleting with correct etag works
    caldav_client.delete(&resource, &etag).await.unwrap();

    let items = caldav_client.list_resources(&collection).await.unwrap();
    assert_eq!(items.len(), 0);
}
