use anyhow::{ensure, Context};
use libdav::dav::CollectionType;

use crate::{random_string, TestData};

pub(crate) async fn test_setting_and_getting_addressbook_displayname(
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

pub(crate) async fn test_check_carddav_support(test_data: &TestData) -> anyhow::Result<()> {
    test_data
        .carddav
        .check_support(test_data.carddav.context_path())
        .await?;

    Ok(())
}
