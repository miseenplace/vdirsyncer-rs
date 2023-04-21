use anyhow::{bail, Context};
use http::StatusCode;
use libdav::{
    auth::Auth,
    dav::{mime_types, CollectionType, DavError},
    CalDavClient,
};
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use std::fmt::Write;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    simple_logger::init_with_level(log::Level::Error).expect("logger configuration is valid");

    let client = create_test_client_from_env()
        .await
        .context("could not initialise test client")?;
    println!("🗓️ Running tests for: {}", client.context_path());

    let results = vec![
        test_create_and_delete_collection(&client)
            .await
            .context("create and delete collection"),
        test_create_and_force_delete_collection(&client)
            .await
            .context("create and force delete collection"),
        test_create_and_delete_resource(&client)
            .await
            .context("create and delete resource"),
        test_create_and_fetch_resource(&client)
            .await
            .context("create and fetch resource"),
        test_fetch_missing(&client)
            .await
            .context("attempt to fetch inexistant resource"),
        test_check_support(&client)
            .await
            .context("check that server advertises caldav support"),
    ];

    let mut failed = 0;
    for ref result in results.iter() {
        if let Err(err) = result {
            println!("🔥 Test failed: {:?}", err);
            failed += 1;
            println!("-----");
        }
    }
    let total = results.len();
    let passed = total - failed;

    println!("✅ Tests passed: {}/{}", passed, total);
    if failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}

async fn create_test_client_from_env() -> anyhow::Result<CalDavClient> {
    let server = std::env::var("CALDAV_SERVER").context("Could not read CALDAV_SERVER")?;
    let username = std::env::var("CALDAV_USERNAME").context("Could not read CALDAV_USERNAME")?;
    let password = std::env::var("CALDAV_PASSWORD").context("Could not read CALDAV_PASSWORD")?;

    let client = CalDavClient::builder()
        .with_uri(server.parse()?)
        .with_auth(Auth::Basic {
            username,
            password: Some(password),
        })
        .build()
        .auto_bootstrap()
        .await?;
    Ok(client)
}

fn random_string(len: usize) -> String {
    thread_rng()
        .sample_iter(Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

async fn test_create_and_delete_collection(caldav_client: &CalDavClient) -> anyhow::Result<()> {
    let home_set = caldav_client
        .calendar_home_set
        .as_ref()
        .context("no calendar home set found for client")?
        .clone();
    let calendars = caldav_client.find_calendars(&home_set).await?;

    let orig_calendar_count = calendars.len();

    let new_collection = format!("{}{}/", home_set.path(), &random_string(16));
    caldav_client
        .create_collection(&new_collection, CollectionType::Calendar)
        .await?;

    let calendars = caldav_client.find_calendars(&home_set).await?;
    let new_calendar_count = calendars.len();

    assert_eq!(orig_calendar_count + 1, new_calendar_count);

    // Get the etag of the newly created calendar:
    // ASSERTION: this validates that a collection with a matching href was created.
    let etag = caldav_client
        .find_calendars(&home_set)
        .await?
        .into_iter()
        .find(|(href, _etag)| href == &new_collection)
        .context("created calendar was not returned when finding calendars")?
        .1;

    // Try deleting with the wrong etag.
    caldav_client
        .delete(&new_collection, "wrong-etag")
        .await
        .unwrap_err();

    let etag = match etag {
        Some(e) => e,
        None => bail!("deletion is only supported on servers which provide etags"),
    };

    // Delete the calendar
    caldav_client.delete(new_collection, etag).await?;

    let calendars = caldav_client.find_calendars(&home_set).await?;
    let third_calendar_count = calendars.len();

    assert_eq!(orig_calendar_count, third_calendar_count);

    Ok(())
}

async fn test_create_and_force_delete_collection(
    caldav_client: &CalDavClient,
) -> anyhow::Result<()> {
    let home_set = caldav_client
        .calendar_home_set
        .as_ref()
        .context("no calendar home set found for client")?
        .clone();
    let calendars = caldav_client.find_calendars(&home_set).await?;

    let orig_calendar_count = calendars.len();

    let new_collection = format!("{}{}/", home_set.path(), &random_string(16));
    caldav_client
        .create_collection(&new_collection, CollectionType::Calendar)
        .await?;

    let calendars = caldav_client.find_calendars(&home_set).await?;
    let after_creationg_calendar_count = calendars.len();

    assert_eq!(orig_calendar_count + 1, after_creationg_calendar_count);

    // Try deleting with the wrong etag.
    caldav_client.force_delete(&new_collection).await?;

    let calendars = caldav_client.find_calendars(&home_set).await?;
    let after_deletion_calendar_count = calendars.len();

    assert_eq!(orig_calendar_count, after_deletion_calendar_count);
    Ok(())
}

fn minimal_icalendar() -> anyhow::Result<Vec<u8>> {
    let mut entry = String::new();
    let uid = random_string(12);

    entry.push_str("BEGIN:VCALENDAR\r\n");
    entry.push_str("VERSION:2.0\r\n");
    entry.push_str("PRODID:-//hacksw/handcal//NONSGML v1.0//EN\r\n");
    entry.push_str("BEGIN:VEVENT\r\n");
    write!(entry, "UID:{uid}\r\n")?;
    entry.push_str("DTSTAMP:19970610T172345Z\r\n");
    entry.push_str("DTSTART:19970714T170000Z\r\n");
    entry.push_str("SUMMARY:hello, testing\r\n");
    entry.push_str("END:VEVENT\r\n");
    entry.push_str("END:VCALENDAR\r\n");

    Ok(entry.into())
}

async fn test_create_and_delete_resource(caldav_client: &CalDavClient) -> anyhow::Result<()> {
    let home_set = caldav_client
        .calendar_home_set
        .as_ref()
        .context("no calendar home set found for client")?
        .clone();

    let collection = format!("{}{}/", home_set.path(), &random_string(16));
    caldav_client
        .create_collection(&collection, CollectionType::Calendar)
        .await?;

    let resource = format!("{}{}.ics", collection, &random_string(12));
    let content = minimal_icalendar()?;

    caldav_client
        .create_resource(&resource, content.clone(), mime_types::CALENDAR)
        .await?;

    let items = caldav_client.list_resources(&collection).await?;
    assert_eq!(items.len(), 1);

    let updated_entry = String::from_utf8(content)?
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
                Some(i.details.etag)
            } else {
                None
            }
        })
        .context("todo")?
        .context("todo")?;

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
    {
        DavError::BadStatusCode(StatusCode::PRECONDITION_FAILED) => {}
        _ => panic!("updating entry with the wrong etag did not return the wrong error type"),
    }

    // ASSERTION: updating with correct etag work
    caldav_client
        .update_resource(&resource, updated_entry, &etag, mime_types::CALENDAR)
        .await?;

    // ASSERTION: deleting with outdated etag fails
    caldav_client.delete(&resource, &etag).await.unwrap_err();

    let items = caldav_client.list_resources(&collection).await?;
    assert_eq!(items.len(), 1);

    let etag = items
        .into_iter()
        .find_map(|i| {
            if i.href == resource {
                Some(i.details.etag)
            } else {
                None
            }
        })
        .context("todo")?
        .context("todo")?;

    // ASSERTION: deleting with correct etag works
    caldav_client.delete(&resource, &etag).await?;

    let items = caldav_client.list_resources(&collection).await?;
    assert_eq!(items.len(), 0);
    Ok(())
}

async fn test_create_and_fetch_resource(caldav_client: &CalDavClient) -> anyhow::Result<()> {
    let home_set = caldav_client
        .calendar_home_set
        .as_ref()
        .context("no calendar home set found for client")?
        .clone();

    let collection = format!("{}{}/", home_set.path(), &random_string(16));
    caldav_client
        .create_collection(&collection, CollectionType::Calendar)
        .await?;

    let resource = format!("{}{}.ics", collection, &random_string(12));
    caldav_client
        .create_resource(&resource, minimal_icalendar()?, mime_types::CALENDAR)
        .await?;

    let items = caldav_client.list_resources(&collection).await?;
    assert_eq!(items.len(), 1);

    let fetched = caldav_client
        .get_resources(&collection, &[&items[0].href])
        .await?;
    assert_eq!(fetched.len(), 1);

    // FIXME: some servers will fail here due to tampering PRODID
    // FIXME: order of lines may vary but items are still equivalent.
    // assert_eq!(
    //     fetched[0].data,
    //     String::from_utf8(minimal_icalendar()?)?
    // );
    Ok(())
}

async fn test_fetch_missing(caldav_client: &CalDavClient) -> anyhow::Result<()> {
    let home_set = caldav_client
        .calendar_home_set
        .as_ref()
        .context("no calendar home set found for client")?
        .clone();

    let collection = format!("{}{}/", home_set.path(), &random_string(16));
    caldav_client
        .create_collection(&collection, CollectionType::Calendar)
        .await?;

    let resource = format!("{}{}.ics", collection, &random_string(12));
    caldav_client
        .create_resource(&resource, minimal_icalendar()?, mime_types::CALENDAR)
        .await?;

    let missing = format!("{}{}.ics", collection, &random_string(8));
    let fetched = caldav_client
        .get_resources(&collection, &[&resource, &missing])
        .await?;
    log::debug!("{:?}", &fetched);
    // Nextcloud omits missing entries, rather than return 404, so we might have just one result.
    match fetched.len() {
        1 => {}
        2 => {
            // ASSERTION: one of the two entries is the 404 one
            fetched
                .iter()
                .find(|r| r.content == Err(StatusCode::NOT_FOUND))
                .context("no entry was missing, but one was expected")?;
        }
        _ => bail!("bogus amount of resources found"),
    }
    // ASSERTION: one entry is the matching resource
    fetched
        .iter()
        .find(|r| r.content.is_ok())
        .context("no entry was found, but one was expected")?;
    Ok(())
}

async fn test_check_support(caldav_client: &CalDavClient) -> anyhow::Result<()> {
    caldav_client
        .check_support(caldav_client.context_path())
        .await?;

    Ok(())
}
