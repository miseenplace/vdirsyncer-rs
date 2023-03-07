//! A CalDavStorage is a single CalDav repository, as specified in rfc4791.
//! XXX: WARNING: This module is VERY INCOMPLETE!

use std::{io::Result, sync::Arc};

use async_trait::async_trait;
use http::Uri;
use tokio::sync::RwLock;
use vcaldav::auth::Auth;
use vcaldav::CalDavClient;

use crate::base::{Collection, Definition, MetadataKind, Storage};

pub struct CalDavDefinition {
    pub url: Uri,
    pub auth: Auth,
}

#[async_trait]
impl Definition for CalDavDefinition {
    async fn storage(self) -> Result<Box<dyn Storage>> {
        let unwrapped_client = CalDavClient::auto_bootstrap(self.url, self.auth).await?;
        let client = Arc::from(RwLock::from(unwrapped_client));

        Ok(Box::from(CalDavStorage { client }))
    }
}

/// A storage backed by a CalDav server.
///
/// A single storage represents a single server with a specific set of credentials.
pub struct CalDavStorage {
    client: Arc<RwLock<CalDavClient>>,
}

// TODO: https://www.rfc-editor.org/rfc/rfc6764 states that we should cache principals and other
// data. But we don't have an API for storages to expose cachable data. Maybe we should?

#[async_trait]
impl Storage for CalDavStorage {
    async fn check(&self) -> Result<()> {
        todo!()
    }

    /// Finds existing collections for this storage.
    ///
    /// Will only return collections stored under the principal's home. In most common scenarios,
    /// this implies that only collections owned by the current user are found and not other
    /// collections.
    ///
    /// Collections outside the principal's home can still be found by providing an absolute path
    /// to [`CalDavStorage::open_collection`].
    async fn discover_collections(&self) -> Result<Vec<Box<dyn Collection>>> {
        let client = self.client.read().await;
        let x = client
            .find_calendars(client.context_path().clone())
            .await?
            .into_iter()
            .map(|href| {
                let collection: Box<dyn Collection> = Box::from(CalDavCollection {
                    href,
                    client: self.client.clone(),
                });
                collection
            })
            .collect::<Vec<_>>();
        Ok(x)
    }

    async fn create_collection(&mut self, _href: &str) -> Result<Box<dyn Collection>> {
        todo!()
    }

    async fn destroy_collection(&mut self, _href: &str) -> Result<()> {
        todo!()
    }

    fn open_collection(&self, href: &str) -> Result<Box<dyn Collection>> {
        let b: Box<dyn Collection> = Box::from(CalDavCollection {
            client: self.client.clone(),
            href: href.to_string(),
        });
        Ok(b)
    }
}

/// A CalDav collection
///
/// The "collection" concept from `vstorage` maps 1:1 with the "collection" concept in CalDav.
pub struct CalDavCollection {
    client: Arc<RwLock<CalDavClient>>,
    href: String,
}

#[async_trait]
impl Collection for CalDavCollection {
    fn id(&self) -> &str {
        &self.href
    }

    fn href(&self) -> &str {
        &self.href
    }

    async fn list(&self) -> Result<Vec<crate::base::ItemRef>> {
        todo!()
    }

    async fn get(&self, _href: &str) -> Result<(crate::base::Item, crate::base::Etag)> {
        todo!()
    }

    async fn get_many(
        &self,
        _hrefs: &[&str],
    ) -> Result<Vec<(crate::base::Href, crate::base::Item, crate::base::Etag)>> {
        todo!()
    }

    async fn get_all(
        &self,
    ) -> Result<Vec<(crate::base::Href, crate::base::Item, crate::base::Etag)>> {
        todo!()
    }

    async fn add(&mut self, _item: &crate::base::Item) -> Result<crate::base::ItemRef> {
        todo!()
    }

    async fn update(
        &mut self,
        _href: &str,
        _etag: &str,
        _item: &crate::base::Item,
    ) -> Result<crate::base::Etag> {
        todo!()
    }

    async fn set_meta(&mut self, _meta: MetadataKind, _value: &str) -> Result<()> {
        todo!()
    }

    /// Read metadata from a collection.
    ///
    /// Metadata is fetched using the `PROPFIND` method under the hood. Some servers may not
    /// support some properties.
    ///
    /// # Errors
    ///
    /// If the underlying HTTP connection fails or if the server returns invalid data.
    async fn get_meta(&self, meta: MetadataKind) -> Result<Option<String>> {
        let client = &self.client.read().await;

        let result = match meta {
            MetadataKind::DisplayName => client.get_calendar_displayname(&self.href).await,
            MetadataKind::Colour => client.get_calendar_colour(&self.href).await,
        };

        result.map_err(|e| e.into())
    }
}
