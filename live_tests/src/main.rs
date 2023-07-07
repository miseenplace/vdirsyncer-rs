// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use anyhow::Context;
use http::Uri;
use libdav::{auth::Auth, CalDavClient, CardDavClient};
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use std::{fs::File, io::Read, path::Path};

mod caldav;
mod carddav;

/// A profile for a test server
///
/// Profiles are expected to be defined in files which specify details for connecting
/// to the server and exceptions to rules for tests (e.g.: expected failures).
#[derive(serde::Deserialize, Debug, Clone)]
struct Profile {
    host: String,
    username: String,
    password: String,
    /// The name of the server implementation.
    server: String,
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

fn process_result(
    test_data: &TestData,
    test_name: &str,
    result: anyhow::Result<()>,
    total: &mut u32,
    failed: &mut u32,
) {
    print!("- {test_name}: ");
    if let Some(expected_failure) = EXPECTED_FAILURES
        .iter()
        .find(|x| x.server == test_data.profile.server.as_str() && x.test == test_name)
    {
        if result.is_ok() {
            println!("⛔ expected failure but passed");
            *failed += 1;
        } else {
            println!("⚠️ expected failure: {}", expected_failure.reason);
        }
    } else if let Err(err) = &result {
        println!("⛔ failed: {err:?}");
        *failed += 1;
    } else {
        println!("✅ passed");
    };
    *total += 1;
}

macro_rules! run_tests {
    ($test_data:expr, $($test:expr,)*) => {
        {
            let mut total = 0;
            let mut failed = 0;
            $(
                let name = stringify!($test);
                let result = $test($test_data).await;
                process_result($test_data, name, result, &mut total, &mut failed);
            )*
            (total, failed)
        }
    };
}

struct ExpectedFailure {
    server: &'static str,
    test: &'static str,
    reason: &'static str,
}

/// A list of tests that are known to fail on specific servers.
///
/// An `xfail` proc macro would be nice, but it seems like an overkill for just a single project.
const EXPECTED_FAILURES: &[ExpectedFailure] = &[
    // Baikal
    ExpectedFailure {
        server: "baikal",
        test: "caldav::test_create_and_delete_collection",
        reason: "https://github.com/sabre-io/Baikal/issues/1182",
    },
    // Cyrus-IMAP
    ExpectedFailure {
        server: "cyrus-imap",
        test: "caldav::test_create_and_delete_collection",
        reason: "precondition failed (unreported)",
    },
    ExpectedFailure {
        server: "cyrus-imap",
        test: "caldav::test_check_caldav_support",
        reason: "server does not adviertise caldav support (unreported)",
    },
    ExpectedFailure {
        server: "cyrus-imap",
        test: "caldav::test_setting_and_getting_colour",
        reason: "https://github.com/cyrusimap/cyrus-imapd/issues/4489",
    },
    ExpectedFailure {
        server: "cyrus-imap",
        test: "carddav::test_check_carddav_support",
        reason: "server does not adviertise caldav support (unreported)",
    },
    // Nextcloud
    ExpectedFailure {
        server: "nextcloud",
        test: "caldav::test_create_and_delete_collection",
        reason: "server does not return etags (unreported)",
    },
    ExpectedFailure {
        server: "nextcloud",
        test: "caldav::test_check_caldav_support",
        reason: "https://github.com/nextcloud/server/issues/37374",
    },
    ExpectedFailure {
        server: "nextcloud",
        test: "carddav::test_check_carddav_support",
        reason: "server does not adviertise caldav support (unreported)",
    },
    // Xandikos
    ExpectedFailure {
        server: "xandikos",
        test: "caldav::test_create_and_fetch_resource_with_weird_characters",
        reason: "https://github.com/jelmer/xandikos/issues/253",
    },
];

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
        caldav::test_create_and_delete_collection,
        caldav::test_create_and_force_delete_collection,
        caldav::test_setting_and_getting_displayname,
        caldav::test_setting_and_getting_colour,
        caldav::test_create_and_delete_resource,
        caldav::test_create_and_fetch_resource,
        caldav::test_create_and_fetch_resource_with_weird_characters,
        caldav::test_create_and_fetch_resource_with_non_ascii_data,
        caldav::test_fetch_missing,
        caldav::test_check_caldav_support,
        carddav::test_setting_and_getting_addressbook_displayname,
        carddav::test_check_carddav_support,
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
