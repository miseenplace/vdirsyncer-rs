//! Wrappers for using storages in read-only mode.
//!
//! These wrappers wrap around a normal [`Storage`] instance, but return [`ReadOnlyFilesystem`] for
//! any write operations.
//!
//! [`ReadOnlyFilesystem`]: std::io::ErrorKind::ReadOnlyFilesystem

use async_trait::async_trait;

use crate::base::Collection;
use crate::base::Storage;
use std::io;
use std::io::Result;

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
/// let orig = FilesystemDefinition {
///     path: PathBuf::from("/path/to/storage/"),
///     extension: String::from("ics"),
/// }.storage().unwrap();
///
/// let read_only = ReadOnlyStorage::from(orig);
/// ```
pub struct ReadOnlyStorage {
    inner: Box<dyn Storage>,
}

/// A wrapper around a [`Collection`] that disallows any write operations.
pub struct ReadOnlyCollection {
    inner: Box<dyn Collection>,
}

#[async_trait]
impl Storage for ReadOnlyStorage {
    async fn check(&self) -> Result<()> {
        self.inner.check().await
    }

    async fn discover_collections(&self) -> Result<Vec<Box<dyn Collection>>> {
        self.inner.discover_collections().await.map(|v| {
            v.into_iter()
                .map(|c| {
                    let b: Box<dyn Collection> = Box::from(ReadOnlyCollection::from(c));
                    b
                })
                .collect()
        })
    }

    async fn create_collection(&mut self, _href: &str) -> Result<Box<dyn Collection>> {
        Err(io::ErrorKind::ReadOnlyFilesystem.into())
    }

    async fn destroy_collection(&mut self, _href: &str) -> Result<()> {
        Err(io::ErrorKind::ReadOnlyFilesystem.into())
    }

    fn open_collection(&self, href: &str) -> Result<Box<dyn Collection>> {
        self.inner.open_collection(href).map(|c| {
            let b: Box<dyn Collection> = Box::from(ReadOnlyCollection::from(c));
            b
        })
    }
}

#[async_trait]
impl Collection for ReadOnlyCollection {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn href(&self) -> &str {
        self.inner.href()
    }

    async fn list(&self) -> Result<Vec<crate::base::ItemRef>> {
        self.inner.list().await
    }

    async fn get(&self, href: &str) -> Result<(crate::base::Item, crate::base::Etag)> {
        self.inner.get(href).await
    }

    async fn get_many(
        &self,
        hrefs: &[&str],
    ) -> Result<Vec<(crate::base::Href, crate::base::Item, crate::base::Etag)>> {
        self.inner.get_many(hrefs).await
    }

    async fn get_all(
        &self,
    ) -> Result<Vec<(crate::base::Href, crate::base::Item, crate::base::Etag)>> {
        self.inner.get_all().await
    }

    async fn add(&mut self, _: &crate::base::Item) -> Result<crate::base::ItemRef> {
        Err(io::ErrorKind::ReadOnlyFilesystem.into())
    }

    async fn update(
        &mut self,
        _: &str,
        _: &str,
        _: &crate::base::Item,
    ) -> Result<crate::base::Etag> {
        Err(io::ErrorKind::ReadOnlyFilesystem.into())
    }

    async fn set_meta(&mut self, _: crate::base::MetadataKind, _: &str) -> Result<()> {
        Err(io::ErrorKind::ReadOnlyFilesystem.into())
    }

    async fn get_meta(&self, meta: crate::base::MetadataKind) -> Result<Option<String>> {
        self.inner.get_meta(meta).await
    }
}

impl From<Box<dyn Storage>> for ReadOnlyStorage {
    fn from(value: Box<dyn Storage>) -> Self {
        Self { inner: value }
    }
}

impl From<Box<dyn Collection>> for ReadOnlyCollection {
    fn from(value: Box<dyn Collection>) -> Self {
        Self { inner: value }
    }
}
