//! A [`CalDavStorage`] is a single caldav repository, as specified in rfc4791.

use std::sync::Arc;

use async_trait::async_trait;
use http::Uri;
use libdav::dav::mime_types;
use libdav::CalDavClient;
use libdav::{auth::Auth, dav::CollectionType};

use crate::base::{Collection, Definition, Etag, Href, Item, ItemRef, MetadataKind, Storage};
use crate::Result;
use crate::{Error, ErrorKind};

pub struct CalDavDefinition {
    pub url: Uri,
    pub auth: Auth,
}

impl From<libdav::BootstrapError> for Error {
    fn from(value: libdav::BootstrapError) -> Self {
        // TODO: not implemented
        Error::new(ErrorKind::Uncategorised, value)
    }
}

impl From<libdav::dav::DavError> for Error {
    fn from(value: libdav::dav::DavError) -> Self {
        // TODO: not implemented
        Error::new(ErrorKind::Uncategorised, value)
    }
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
        let uri = &self
            .client
            .calendar_home_set
            .as_ref()
            .unwrap_or(self.client.context_path());
        self.client
            .check_support(uri)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))
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
        let uri = self
            .client
            .calendar_home_set
            .as_ref()
            .unwrap_or(self.client.context_path());
        let x = self
            .client
            .find_calendars(uri)
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

    async fn create_collection(&mut self, href: &str) -> Result<Box<dyn Collection>> {
        self.client
            .create_collection(href, CollectionType::Calendar)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;
        Ok(Box::from(CalDavCollection {
            href: href.to_string(),
            client: self.client.clone(),
        }))
    }

    /// Deletes a caldav collection.
    ///
    /// This method does multiple network calls to ensure that the collection is empty. If the
    /// server property supports `Etag` (it MUST as per the spec), this method guarantees that the
    /// collection is empty when deleting it.
    ///
    /// If the server is not compliant and does not support Etags, possible race conditions could
    /// occur and if calendar components are added to the collection at the same time, they may be
    /// deleted.
    async fn destroy_collection(&mut self, href: &str) -> Result<()> {
        let mut results = self
            .client
            .get_resources(href, &[href])
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;

        if results.len() != 1 {
            return Err(ErrorKind::InvalidData.into());
        }

        let item = results.pop().expect("results has exactly one item");
        if item.href != href {
            return Err(Error::new(
                ErrorKind::Uncategorised,
                format!("Requested href: {}, got: {}", href, item.href,),
            ));
        }

        let etag = item
            .content
            .map_err(|e| Error::new(ErrorKind::Uncategorised, format!("Got status code: {e}")))?
            .etag;
        // TODO: specific error kind type for MissingEtag?

        // TODO: if no etag -> use force deletion (and warn)
        let collection = CalDavCollection {
            client: self.client.clone(),
            href: href.to_string(),
        };

        // TODO: verify that the collection is actually a calendar collection?
        // This could be done by using discover above.
        let items = collection.list().await?;
        if !items.is_empty() {
            return Err(ErrorKind::CollectionNotEmpty.into());
        }

        self.client
            .delete(href, etag)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;
        Ok(())
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
        self.href
            .strip_suffix('/')
            .unwrap_or("")
            .rsplit('/')
            .next()
            .expect("rsplit always yields at least one result")
    }

    fn href(&self) -> &str {
        &self.href
    }

    async fn list(&self) -> Result<Vec<ItemRef>> {
        let response = self.client.list_resources(&self.href).await?;
        let mut items = Vec::with_capacity(response.len());
        for r in response {
            items.push(ItemRef {
                href: r.href,
                etag: r.details.etag.ok_or(Error::from(ErrorKind::InvalidData))?,
            });
        }
        Ok(items)
    }

    async fn get(&self, href: &str) -> Result<(Item, Etag)> {
        let mut results = self
            .client
            .get_resources(&self.href, &[href])
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;

        if results.len() != 1 {
            return Err(ErrorKind::InvalidData.into());
        }

        let item = results.pop().expect("results has exactly one item");
        if item.href != href {
            return Err(Error::new(
                ErrorKind::Uncategorised,
                format!("Requested href: {}, got: {}", href, item.href,),
            ));
        }

        let content = item
            .content
            .map_err(|e| Error::new(ErrorKind::Uncategorised, format!("Got status code: {e}")))?;

        Ok((Item::from(content.data), content.etag))
    }

    async fn get_many(&self, hrefs: &[&str]) -> Result<Vec<(Href, Item, Etag)>> {
        Ok(self
            .client
            .get_resources(&self.href, hrefs)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?
            .into_iter()
            .map(|r| {
                let content = r.content.unwrap();
                (r.href, Item::from(content.data), content.etag)
            })
            .collect())
    }

    async fn get_all(&self) -> Result<Vec<(Href, Item, Etag)>> {
        let list = self.list().await?;
        let hrefs = list.iter().map(|i| i.href.as_str()).collect::<Vec<_>>();
        self.get_many(&hrefs).await
    }

    async fn add(&mut self, item: &Item) -> Result<ItemRef> {
        let href = item.ident();
        self.client
            // FIXME: should not copy data here?
            .create_resource(
                &href,
                item.as_str().as_bytes().to_vec(),
                mime_types::CALENDAR,
            )
            .await
            .map(|opt| opt.ok_or(Error::new(ErrorKind::InvalidData, "No Etag in response")))?
            .map(|etag| ItemRef { href, etag })
    }

    async fn update(&mut self, href: &str, etag: &str, item: &Item) -> Result<Etag> {
        self.client
            .update_resource(
                href,
                item.as_str().as_bytes().to_vec(),
                etag,
                mime_types::CALENDAR,
            )
            .await
            .map(|opt| opt.ok_or(Error::new(ErrorKind::InvalidData, "No Etag in response")))?
    }

    /// # Panics
    ///
    /// This function is not implemented.
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
