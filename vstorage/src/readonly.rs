//! Wrappers for using storages in read-only mode.
//!
//! These wrappers wrap around a normal [`Storage`] instance, but return [`ReadOnlyFilesystem`] for
//! any write operations.
//!
//! [`ReadOnlyFilesystem`]: std::io::ErrorKind::ReadOnlyFilesystem

use async_trait::async_trait;

use crate::base::Collection;
use crate::base::Storage;
use crate::ErrorKind;
use crate::Result;

/// A wrapper around a [`Storage`] that disallows any write operations.
///
/// # Example
///
/// ```
/// # use vstorage::filesystem::FilesystemStorage;
/// # use crate::vstorage::base::Storage;
/// # use vstorage::filesystem::FilesystemDefinition;
/// # use std::path::PathBuf;
/// # use vstorage::readonly::ReadOnlyStorage;
/// # use crate::vstorage::base::Definition;
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let orig = FilesystemDefinition {
///     path: PathBuf::from("/path/to/storage/"),
///     extension: String::from("ics"),
/// }.storage().await.unwrap();
///
/// let read_only = ReadOnlyStorage::from(orig);
/// # })
/// ```
pub struct ReadOnlyStorage {
    inner: Box<dyn Storage>,
}

#[async_trait]
impl Storage for ReadOnlyStorage {
    async fn check(&self) -> Result<()> {
        self.inner.check().await
    }

    async fn discover_collections(&self) -> Result<Vec<Collection>> {
        self.inner.discover_collections().await
    }

    async fn create_collection(&mut self, _href: &str) -> Result<Collection> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn destroy_collection(&mut self, _href: &str) -> Result<()> {
        Err(ErrorKind::ReadOnly.into())
    }

    fn open_collection(&self, href: &str) -> Result<Collection> {
        self.inner.open_collection(href)
    }

    async fn list_items(&self, collection: &Collection) -> Result<Vec<crate::base::ItemRef>> {
        self.inner.list_items(collection).await
    }

    async fn get_item(
        &self,
        collection: &Collection,
        href: &str,
    ) -> Result<(crate::base::Item, crate::base::Etag)> {
        self.inner.get_item(collection, href).await
    }

    async fn get_many_items(
        &self,
        collection: &Collection,
        hrefs: &[&str],
    ) -> Result<Vec<(crate::base::Href, crate::base::Item, crate::base::Etag)>> {
        self.inner.get_many_items(collection, hrefs).await
    }

    async fn get_all_items(
        &self,
        collection: &Collection,
    ) -> Result<Vec<(crate::base::Href, crate::base::Item, crate::base::Etag)>> {
        self.inner.get_all_items(collection).await
    }

    async fn add_item(
        &mut self,
        _: &Collection,
        _: &crate::base::Item,
    ) -> Result<crate::base::ItemRef> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn update_item(
        &mut self,
        _: &Collection,
        _: &str,
        _: &str,
        _: &crate::base::Item,
    ) -> Result<crate::base::Etag> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn set_collection_meta(
        &mut self,
        _: &Collection,
        _: crate::base::MetadataKind,
        _: &str,
    ) -> Result<()> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn get_collection_meta(
        &self,
        collection: &Collection,
        meta: crate::base::MetadataKind,
    ) -> Result<Option<String>> {
        self.inner.get_collection_meta(collection, meta).await
    }
}

impl From<Box<dyn Storage>> for ReadOnlyStorage {
    fn from(value: Box<dyn Storage>) -> Self {
        Self { inner: value }
    }
}
