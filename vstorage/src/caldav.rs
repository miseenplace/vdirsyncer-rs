//! A [`CalDavStorage`] is a single caldav repository, as specified in rfc4791.

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
        let client = CalDavClient::builder()
            .with_uri(self.url)
            .with_auth(self.auth)
            .build()
            .auto_bootstrap()
            .await?;

        Ok(Box::from(CalDavStorage { client }))
    }
}

/// A storage backed by a caldav server.
///
/// A single storage represents a single server with a specific set of credentials.
pub struct CalDavStorage {
    client: CalDavClient,
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
    async fn discover_collections(&self) -> Result<Vec<Collection>> {
        let x = self
            .client
            .find_calendars(None)
            .await?
            .into_iter()
            .map(|collection| Collection::new(collection.href))
            .collect::<Vec<_>>();
        Ok(x)
    }

    async fn create_collection(&mut self, href: &str) -> Result<Collection> {
        self.client
            .create_collection(href, CollectionType::Calendar)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;
        Ok(Collection::new(href.to_string()))
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
        let collection = Collection::new(href.to_string());

        // TODO: verify that the collection is actually a calendar collection?
        // This could be done by using discover above.
        let items = self.list_items(&collection).await?;
        if !items.is_empty() {
            return Err(ErrorKind::CollectionNotEmpty.into());
        }

        self.client
            .delete(href, etag)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;
        Ok(())
    }

    fn open_collection(&self, href: &str) -> Result<Collection> {
        Ok(Collection::new(href.to_string()))
    }

    async fn list_items(&self, collection: &Collection) -> Result<Vec<ItemRef>> {
        let response = self.client.list_resources(collection.href()).await?;
        let mut items = Vec::with_capacity(response.len());
        for r in response {
            items.push(ItemRef {
                href: r.href,
                etag: r.details.etag.ok_or(Error::from(ErrorKind::InvalidData))?,
            });
        }
        Ok(items)
    }

    async fn get_item(&self, collection: &Collection, href: &str) -> Result<(Item, Etag)> {
        let mut results = self
            .client
            .get_resources(&collection.href(), &[href])
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

    async fn get_many_items(
        &self,
        collection: &Collection,
        hrefs: &[&str],
    ) -> Result<Vec<(Href, Item, Etag)>> {
        Ok(self
            .client
            .get_resources(&collection.href(), hrefs)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?
            .into_iter()
            .map(|r| {
                let content = r.content.unwrap();
                (r.href, Item::from(content.data), content.etag)
            })
            .collect())
    }

    async fn get_all_items(&self, collection: &Collection) -> Result<Vec<(Href, Item, Etag)>> {
        let list = self.list_items(collection).await?;
        let hrefs = list.iter().map(|i| i.href.as_str()).collect::<Vec<_>>();
        self.get_many_items(collection, &hrefs).await
    }

    async fn add_item(&mut self, collection: &Collection, item: &Item) -> Result<ItemRef> {
        let href = join_hrefs(collection.href(), &item.ident());
        // TODO: ident: .chars().filter(char::is_ascii_alphanumeric)

        self.client
            // FIXME: should not copy data here?
            .create_resource(
                &href,
                item.as_str().as_bytes().to_vec(),
                mime_types::CALENDAR,
            )
            .await
            // FIXME: etag may be missing. In such case, we should fetch it.
            .map(|opt| opt.ok_or(Error::new(ErrorKind::InvalidData, "No Etag in response")))?
            .map(|etag| ItemRef { href, etag })
    }

    async fn update_item(
        &mut self,
        _collection: &Collection,
        href: &str,
        etag: &str,
        item: &Item,
    ) -> Result<Etag> {
        // TODO: check that href is a sub-path of collection.href?
        self.client
            .update_resource(
                href,
                item.as_str().as_bytes().to_vec(),
                etag,
                mime_types::CALENDAR,
            )
            .await
            // FIXME: etag may be missing. In such case, we should fetch it.
            .map(|opt| opt.ok_or(Error::new(ErrorKind::InvalidData, "No Etag in response")))?
    }

    /// # Panics
    ///
    /// Setting colour is not implemented.
    async fn set_collection_meta(
        &mut self,
        collection: &Collection,
        meta: MetadataKind,
        value: &str,
    ) -> Result<()> {
        match meta {
            MetadataKind::DisplayName => {
                self.client
                    .set_collection_displayname(collection.href(), Some(value))
                    .await
            }
            MetadataKind::Colour => {
                self.client
                    .set_calendar_colour(collection.href(), Some(value))
                    .await
            }
        }
        .map_err(Error::from)
    }

    /// Read metadata from a collection.
    ///
    /// Metadata is fetched using the `PROPFIND` method under the hood. Some servers may not
    /// support some properties.
    ///
    /// # Errors
    ///
    /// If the underlying HTTP connection fails or if the server returns invalid data.
    async fn get_collection_meta(
        &self,
        collection: &Collection,
        meta: MetadataKind,
    ) -> Result<Option<String>> {
        let result = match meta {
            MetadataKind::DisplayName => {
                self.client
                    .get_collection_displayname(collection.href())
                    .await
            }
            MetadataKind::Colour => self.client.get_calendar_colour(collection.href()).await,
        };

        result.map_err(Error::from)
    }

    async fn delete_item(
        &mut self,
        _collection: &Collection,
        href: &str,
        etag: &str,
    ) -> Result<()> {
        // TODO: check that href is a sub-path of collection.href?
        self.client.delete(href, etag).await?;

        Ok(())
    }
}

fn join_hrefs(collection_href: &str, item_href: &str) -> String {
    if item_href.starts_with('/') {
        return item_href.to_string();
    }

    let mut href = collection_href
        .strip_suffix('/')
        .unwrap_or(collection_href)
        .to_string();
    href.push('/');
    href.push_str(item_href);
    href
}
