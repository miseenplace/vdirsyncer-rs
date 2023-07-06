// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use anyhow::{bail, ensure, Context};
use http::{StatusCode, Uri};
use libdav::{
    auth::Auth,
    dav::{mime_types, CollectionType, DavError},
    CalDavClient, CardDavClient,
};
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use std::{collections::HashMap, fmt::Write, fs::File, io::Read, path::Path};

/// A profile for a test server
///
/// Profiles are expected to be defined in files which specify details for connecting
/// to the server and exceptions to rules for tests (e.g.: expected failures).
#[derive(serde::Deserialize, Debug, Clone)]
struct Profile {
    host: String,
    username: String,
    password: String,
    #[serde(default = "HashMap::new")]
    xfail: HashMap<String, String>,
}

impl Profile {
    /// Load a profile from a given path.
    fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let mut file = File::open(path.as_ref()).context("could not open profile file")?;
        // toml crate won't allow reading from a file.
        // See: https://github.com/toml-rs/toml/pull/349
        let mut config = Vec::new();
        file.read_to_end(&mut config)?;

        Ok(toml::de::from_str(std::str::from_utf8(&config)?)?)
    }
}

struct TestData {
    caldav: CalDavClient,
    carddav: CardDavClient,
    calendar_home_set: Uri,
    address_home_set: Uri,
    profile: Profile,
}

impl TestData {
    async fn from_profile(profile: Profile) -> anyhow::Result<Self> {
        let caldav = CalDavClient::builder()
            .with_uri(profile.host.parse()?)
            .with_auth(Auth::Basic {
                username: profile.username.clone(),
                password: Some(profile.password.clone().into()),
            })
            .build()
            .auto_bootstrap()
            .await
            .context("could not initialise test client")?;
        let calendar_home_set = caldav
            .calendar_home_set
            .as_ref()
            .context("no calendar home set found")?
            .clone();

        let carddav = CardDavClient::builder()
            .with_uri(profile.host.parse()?)
            .with_auth(Auth::Basic {
                username: profile.username.clone(),
                password: Some(profile.password.clone().into()),
            })
            .build()
            .auto_bootstrap()
            .await
            .context("could not initialise test client")?;
        let address_home_set = carddav
            .addressbook_home_set
            .as_ref()
            .context("no calendar home set found")?
            .clone();

        Ok(TestData {
            caldav,
            carddav,
            calendar_home_set,
            address_home_set,
            profile,
        })
    }

    async fn calendar_count(&self) -> anyhow::Result<usize> {
        self.caldav
            .find_calendars(None)
            .await
            .map(|calendars| calendars.len())
            .context("fetch calendar count")
    }
}

macro_rules! run_tests {
    ($test_data:expr, $($test:expr,)*) => {
        {
            let mut total = 0;
            let mut failed = 0;
            $(
                let name = stringify!($test);
                let result = $test($test_data).await;
                print!("- {name}: ");
                if let Some((_, reason)) = $test_data.profile.xfail.iter().find(|(k, _)| k.as_str() == name) {
                    if result.is_ok() {
                        println!("⛔ expected failure but passed");
                    } else {
                        println!("⚠️ expected failure: {reason}");
                    }
                } else if let Err(err) = &result {
                    println!("⛔ failed: {err:?}");
                    failed += 1;
                } else {
                    println!("✅ passed");
                };
                total += 1;
            )*
            (total, failed)
        }
    };
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    simple_logger::init_with_level(log::Level::Error).expect("logger configuration is valid");

    let mut args = std::env::args_os();
    let cmd = args.next().expect("Argument zero must be defined");
    let profile_path = args
        .next()
        .context(format!("Usage: {} PROFILE", cmd.to_string_lossy()))?;

    println!("🗓️ Running tests for: {}", profile_path.to_string_lossy());
    let profile = Profile::load(&profile_path)?;
    let test_data = TestData::from_profile(profile).await?;

    let (total, failed) = run_tests!(
        &test_data,
        // caldav tests
        test_create_and_delete_collection,
        test_create_and_force_delete_collection,
        test_setting_and_getting_displayname,
        test_setting_and_getting_colour,
        test_create_and_delete_resource,
        test_create_and_fetch_resource,
        test_create_and_fetch_resource_with_weird_characters,
        test_fetch_missing,
        test_check_caldav_support,
        // carddav
        test_setting_and_getting_addressbook_displayname,
        test_check_carddav_support,
    );

    if failed > 0 {
        println!("⛔ {}/{} tests failed.\n", failed, total);
        std::process::exit(1);
    } else {
        println!("✅ {} tests passed.\n", total);
    }

    Ok(())
}

fn random_string(len: usize) -> String {
    thread_rng()
        .sample_iter(Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

async fn test_create_and_delete_collection(test_data: &TestData) -> anyhow::Result<()> {
    let orig_calendar_count = test_data.calendar_count().await?;

    let new_collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data
        .caldav
        .create_collection(&new_collection, CollectionType::Calendar)
        .await?;

    let new_calendar_count = test_data.calendar_count().await?;

    ensure!(orig_calendar_count + 1 == new_calendar_count);

    // Get the etag of the newly created calendar:
    // ASSERTION: this validates that a collection with a matching href was created.
    let calendars = test_data.caldav.find_calendars(None).await?;
    let etag = calendars
        .into_iter()
        .find(|collection| collection.href == new_collection)
        .context("created calendar was not returned when finding calendars")?
        .etag;

    // Try deleting with the wrong etag.
    test_data
        .caldav
        .delete(&new_collection, "wrong-etag")
        .await
        .unwrap_err();

    let Some(etag) = etag else { bail!("deletion is only supported on servers which provide etags") };

    // Delete the calendar
    test_data.caldav.delete(new_collection, etag).await?;

    let third_calendar_count = test_data.calendar_count().await?;
    ensure!(orig_calendar_count == third_calendar_count);

    Ok(())
}

async fn test_create_and_force_delete_collection(test_data: &TestData) -> anyhow::Result<()> {
    let orig_calendar_count = test_data.calendar_count().await?;

    let new_collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data
        .caldav
        .create_collection(&new_collection, CollectionType::Calendar)
        .await?;

    let after_creationg_calendar_count = test_data.calendar_count().await?;
    ensure!(orig_calendar_count + 1 == after_creationg_calendar_count);

    // Force-delete the collection
    test_data.caldav.force_delete(&new_collection).await?;

    let after_deletion_calendar_count = test_data.calendar_count().await?;
    ensure!(orig_calendar_count == after_deletion_calendar_count);

    Ok(())
}

async fn test_setting_and_getting_displayname(test_data: &TestData) -> anyhow::Result<()> {
    let new_collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data
        .caldav
        .create_collection(&new_collection, CollectionType::Calendar)
        .await?;

    let first_name = "panda-events";
    test_data
        .caldav
        .set_collection_displayname(&new_collection, Some(first_name))
        .await
        .context("setting collection displayname")?;

    let value = test_data
        .caldav
        .get_collection_displayname(&new_collection)
        .await
        .context("getting collection displayname")?;

    ensure!(value == Some(String::from(first_name)));

    let new_name = "🔥🔥🔥<lol>";
    test_data
        .caldav
        .set_collection_displayname(&new_collection, Some(new_name))
        .await
        .context("setting collection displayname")?;

    let value = test_data
        .caldav
        .get_collection_displayname(&new_collection)
        .await
        .context("getting collection displayname")?;

    ensure!(value == Some(String::from(new_name)));

    test_data.caldav.force_delete(&new_collection).await?;

    Ok(())
}

async fn test_setting_and_getting_colour(test_data: &TestData) -> anyhow::Result<()> {
    let new_collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data
        .caldav
        .create_collection(&new_collection, CollectionType::Calendar)
        .await?;

    let colour = "#ff00ff";
    test_data
        .caldav
        .set_calendar_colour(&new_collection, Some(colour))
        .await
        .context("setting collection colour")?;

    let value = test_data
        .caldav
        .get_calendar_colour(&new_collection)
        .await
        .context("getting collection colour")?;

    match value {
        Some(c) => ensure!(c.eq_ignore_ascii_case(colour) || c.eq_ignore_ascii_case("#FF00FFFF")),
        None => bail!("Set a colour but then got colour None"),
    }

    test_data.caldav.force_delete(&new_collection).await?;

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

async fn test_create_and_delete_resource(test_data: &TestData) -> anyhow::Result<()> {
    let collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data
        .caldav
        .create_collection(&collection, CollectionType::Calendar)
        .await?;

    let resource = format!("{}{}.ics", collection, &random_string(12));
    let content = minimal_icalendar()?;

    test_data
        .caldav
        .create_resource(&resource, content.clone(), mime_types::CALENDAR)
        .await?;

    let items = test_data.caldav.list_resources(&collection).await?;
    ensure!(items.len() == 1);

    let updated_entry = String::from_utf8(content)?
        .replace("hello", "goodbye")
        .as_bytes()
        .to_vec();

    // ASSERTION: deleting with a wrong etag fails.
    test_data
        .caldav
        .delete(&resource, "wrong-lol")
        .await
        .unwrap_err();

    // ASSERTION: creating conflicting resource fails.
    test_data
        .caldav
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
    match test_data
        .caldav
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
    test_data
        .caldav
        .update_resource(&resource, updated_entry, &etag, mime_types::CALENDAR)
        .await?;

    // ASSERTION: deleting with outdated etag fails
    test_data.caldav.delete(&resource, &etag).await.unwrap_err();

    let items = test_data.caldav.list_resources(&collection).await?;
    ensure!(items.len() == 1);

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
    test_data.caldav.delete(&resource, &etag).await?;

    let items = test_data.caldav.list_resources(&collection).await?;
    ensure!(items.len() == 0);
    Ok(())
}

async fn test_create_and_fetch_resource(test_data: &TestData) -> anyhow::Result<()> {
    let collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data
        .caldav
        .create_collection(&collection, CollectionType::Calendar)
        .await?;

    let resource = format!("{}{}.ics", collection, &random_string(12));
    test_data
        .caldav
        .create_resource(&resource, minimal_icalendar()?, mime_types::CALENDAR)
        .await?;

    let items = test_data.caldav.list_resources(&collection).await?;
    ensure!(items.len() == 1);

    let fetched = test_data
        .caldav
        .get_resources(&collection, &[&items[0].href])
        .await?;
    ensure!(fetched.len() == 1);
    assert_eq!(fetched[0].href, resource);

    Ok(())
}

async fn test_create_and_fetch_resource_with_weird_characters(
    test_data: &TestData,
) -> anyhow::Result<()> {
    let collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data
        .caldav
        .create_collection(&collection, CollectionType::Calendar)
        .await?;

    let mut count = 0;
    for symbol in ":?# []@!$&'()*+,;=<>".chars() {
        let resource = format!("{}weird-{}-{}.ics", collection, symbol, &random_string(6));
        test_data
            .caldav
            .create_resource(&resource, minimal_icalendar()?, mime_types::CALENDAR)
            .await
            .context(format!("failed to create resource with '{symbol}'"))?;
        count += 1;

        let items = test_data
            .caldav
            .list_resources(&collection)
            .await
            .context(format!("failed listing resource (when testing '{symbol}')"))?;
        ensure!(items.len() == count);
        ensure!(
            items.iter().any(|i| i.href == resource),
            format!("created item must be present when listing (testing '{symbol}')")
        );

        let fetched = test_data
            .caldav
            .get_resources(&collection, &[&resource])
            .await
            .context(format!("failed to get resource with '{symbol}'"))?;
        ensure!(fetched.len() == 1);
        assert_eq!(fetched[0].href, resource);
    }

    // FIXME: some servers will fail here due to tampering PRODID
    // FIXME: order of lines may vary but items are still equivalent.
    // ensure!(fetched[0].data, String::from_utf8(minimal_icalendar()?)?);
    Ok(())
}

async fn test_fetch_missing(test_data: &TestData) -> anyhow::Result<()> {
    let collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data
        .caldav
        .create_collection(&collection, CollectionType::Calendar)
        .await?;

    let resource = format!("{}{}.ics", collection, &random_string(12));
    test_data
        .caldav
        .create_resource(&resource, minimal_icalendar()?, mime_types::CALENDAR)
        .await?;

    let missing = format!("{}{}.ics", collection, &random_string(8));
    let fetched = test_data
        .caldav
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

async fn test_check_caldav_support(test_data: &TestData) -> anyhow::Result<()> {
    test_data
        .caldav
        .check_support(test_data.caldav.context_path())
        .await?;

    Ok(())
}

// CARDDAV ====================================================================

async fn test_setting_and_getting_addressbook_displayname(
    test_data: &TestData,
) -> anyhow::Result<()> {
    let new_collection = format!(
        "{}{}/",
        test_data.address_home_set.path(),
        &random_string(16)
    );
    test_data
        .carddav
        .create_collection(&new_collection, CollectionType::AddressBook)
        .await?;

    let first_name = "panda-events";
    test_data
        .carddav
        .set_collection_displayname(&new_collection, Some(first_name))
        .await
        .context("setting collection displayname")?;

    let value = test_data
        .carddav
        .get_collection_displayname(&new_collection)
        .await
        .context("getting collection displayname")?;

    ensure!(value == Some(String::from(first_name)));

    let new_name = "🔥🔥🔥<lol>";
    test_data
        .carddav
        .set_collection_displayname(&new_collection, Some(new_name))
        .await
        .context("setting collection displayname")?;

    let value = test_data
        .carddav
        .get_collection_displayname(&new_collection)
        .await
        .context("getting collection displayname")?;

    ensure!(value == Some(String::from(new_name)));

    test_data.carddav.force_delete(&new_collection).await?;

    Ok(())
}

async fn test_check_carddav_support(test_data: &TestData) -> anyhow::Result<()> {
    test_data
        .carddav
        .check_support(test_data.carddav.context_path())
        .await?;

    Ok(())
}
