//! A [`CalDavStorage`] is a single caldav repository, as specified in rfc4791.
//!
//! XXX: WARNING: This module is VERY INCOMPLETE!

use std::{
    io::{Error, Result},
    sync::Arc,
};

use async_trait::async_trait;
use http::Uri;
use libdav::auth::Auth;
use libdav::CalDavClient;

use crate::base::{Collection, Definition, MetadataKind, Storage};

pub struct CalDavDefinition {
    pub url: Uri,
    pub auth: Auth,
}

#[async_trait]
impl Definition for CalDavDefinition {
    async fn storage(self) -> Result<Box<dyn Storage>> {
        let unwrapped_client = CalDavClient::builder()
            .with_uri(self.url)
            .with_auth(self.auth)
            .build()
            .auto_bootstrap()
            .await?;
        let client = Arc::from(unwrapped_client);

        Ok(Box::from(CalDavStorage { client }))
    }
}

/// A storage backed by a caldav server.
///
/// A single storage represents a single server with a specific set of credentials.
pub struct CalDavStorage {
    client: Arc<CalDavClient>,
}

#[async_trait]
impl Storage for CalDavStorage {
    async fn check(&self) -> Result<()> {
        // TODO: use https://www.rfc-editor.org/rfc/rfc4791#section-5.1
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
        let x = self
            .client
            .find_calendars(self.client.context_path())
            .await?
            .into_iter()
            .map(|(href, _etag)| {
                CalDavCollection {
                    href,
                    client: self.client.clone(),
                }
                .boxed()
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
        Ok(CalDavCollection {
            client: self.client.clone(),
            href: href.to_string(),
        }
        .boxed())
    }
}

/// A caldav collection
///
/// The "collection" concept from `vstorage` maps 1:1 with the "collection" concept in caldav.
pub struct CalDavCollection {
    client: Arc<CalDavClient>,
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
        let result = match meta {
            MetadataKind::DisplayName => self.client.get_collection_displayname(&self.href).await,
            MetadataKind::Colour => self.client.get_calendar_colour(&self.href).await,
        };

        result.map_err(Error::from)
    }
}
