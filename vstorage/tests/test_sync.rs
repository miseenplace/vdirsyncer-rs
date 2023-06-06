use rand::{distributions::Alphanumeric, thread_rng, Rng};
use std::boxed::Box;
use std::{fmt::Write, path::PathBuf};
use vstorage::sync::plan::Plan;
use vstorage::{
    base::{Definition, IcsItem, Storage},
    filesystem::FilesystemDefinition,
    sync::{StoragePair, StorageState},
};

fn random_string(len: usize) -> String {
    thread_rng()
        .sample_iter(Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn minimal_icalendar(summary: &str) -> anyhow::Result<String> {
    let mut entry = String::new();
    let uid = random_string(12);

    entry.push_str("BEGIN:VCALENDAR\r\n");
    entry.push_str("VERSION:2.0\r\n");
    entry.push_str("PRODID:-//hacksw/handcal//NONSGML v1.0//EN\r\n");
    entry.push_str("BEGIN:VEVENT\r\n");
    write!(entry, "UID:{uid}\r\n")?;
    entry.push_str("DTSTAMP:19970610T172345Z\r\n");
    entry.push_str("DTSTART:19970714T170000Z\r\n");
    write!(entry, "SUMMARY:{summary}\r\n")?;
    entry.push_str("END:VEVENT\r\n");
    entry.push_str("END:VCALENDAR\r\n");

    Ok(entry.into())
}

async fn create_populated_storage(path: PathBuf) -> Box<dyn Storage<IcsItem>> {
    std::fs::create_dir(&path).unwrap();
    let def = FilesystemDefinition::<IcsItem>::new(path, "ics".into());
    let mut storage = def.storage().await.unwrap();

    let first = storage.create_collection("first-calendar").await.unwrap();
    let item = &minimal_icalendar("First calendar event one")
        .unwrap()
        .into();
    storage.add_item(&first, item).await.unwrap();

    let item = &minimal_icalendar("First calendar event two")
        .unwrap()
        .into();
    storage.add_item(&first, item).await.unwrap();
    drop(first);

    let second = storage.create_collection("second-calendar").await.unwrap();
    let item = &minimal_icalendar("Second calendar event one")
        .unwrap()
        .into();
    storage.add_item(&second, item).await.unwrap();

    let item = &minimal_icalendar("Second calendar event two")
        .unwrap()
        .into();
    storage.add_item(&second, item).await.unwrap();
    drop(second);

    let third = storage.create_collection("third-calendar").await.unwrap();
    let item = &minimal_icalendar("Third calendar event one")
        .unwrap()
        .into();
    storage.add_item(&third, item).await.unwrap();
    drop(third);

    storage
}

async fn create_empty_storage(path: PathBuf) -> Box<dyn Storage<IcsItem>> {
    std::fs::create_dir(&path).unwrap();
    let def = FilesystemDefinition::<IcsItem>::new(path, "ics".into());
    def.storage().await.unwrap()
}

#[tokio::test]
async fn test_sync_simple_case() {
    let populated_path = {
        let mut p = std::env::temp_dir();
        p.push(random_string(12));
        p
    };
    let empty_path = {
        let mut p = std::env::temp_dir();
        p.push(random_string(12));
        p
    };
    let mut populated = create_populated_storage(populated_path.clone()).await;
    let mut empty = create_empty_storage(empty_path.clone()).await;

    let names = vec!["first-calendar".to_string(), "second-calendar".to_string()];
    let empty_state = StorageState::empty();
    let mut pair = StoragePair::<IcsItem>::new(
        &mut *populated,
        &mut *empty,
        &empty_state,
        &empty_state,
        &names,
    )
    .await
    .unwrap();
    let plan = Plan::for_storage_pair(&pair);
    let result = plan.execute(&mut pair).await;
    assert_eq!(result.errors.len(), 0);

    let first = std::fs::read_dir(empty_path.join("first-calendar"))
        .unwrap()
        .map(|r| r.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(first.len(), 2);
    for item in first {
        let data = std::fs::read_to_string(item.path()).unwrap();
        let found = data.find("First calendar event");
        assert!(found.is_some());
    }

    let second = std::fs::read_dir(empty_path.join("second-calendar"))
        .unwrap()
        .map(|r| r.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(second.len(), 2);
    for item in second {
        let data = std::fs::read_to_string(item.path()).unwrap();
        let found = data.find("Second calendar event");
        assert!(found.is_some());
    }

    std::fs::remove_dir_all(populated_path).unwrap();
    std::fs::remove_dir_all(empty_path).unwrap();
}
