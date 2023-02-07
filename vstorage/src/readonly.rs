//! Wrappers for using storages in read-only mode.
//!
//! These wrappers wrap around a normal [`Storage`] instance, but return [`ReadOnlyFilesystem`] for
//! any write operations.
//!
//! [`ReadOnlyFilesystem`]: std::io::ErrorKind::ReadOnlyFilesystem

use crate::base::Collection;
use crate::base::Storage;
use std::io;
use std::io::Result;

/// A wrapper around a [`Storage`] that disallows any write operations.
///
/// Aside from the [`ReadOnlyStorage::new`] method, `from` can be used to create these:
///
/// ```
/// # use vstorage::filesystem::FilesystemStorage;
/// # use crate::vstorage::base::Storage;
/// # use vstorage::filesystem::FilesystemDefinition;
/// # use std::path::PathBuf;
/// # use vstorage::readonly::ReadOnlyStorage;
/// let orig = FilesystemStorage::new(FilesystemDefinition {
///     path: PathBuf::from("/path/to/storage/"),
///     extension: String::from("ics"),
/// }).unwrap();
///
/// let read_only = ReadOnlyStorage::from(orig);
/// ```
pub struct ReadOnlyStorage<S: Storage> {
    inner: S,
}

/// A wrapper around a [`Collection`] that disallows any write operations.
pub struct ReadOnlyCollection<C: Collection> {
    inner: C,
}

impl<S: Storage> Storage for ReadOnlyStorage<S> {
    type Definition = S::Definition;

    type Collection = ReadOnlyCollection<S::Collection>;

    fn new(definition: Self::Definition) -> Result<Self> {
        Ok(Self {
            inner: S::new(definition)?,
        })
    }

    async fn check(&self) -> Result<()> {
        self.inner.check().await
    }

    async fn discover_collections(&self) -> Result<Vec<Self::Collection>> {
        self.inner
            .discover_collections()
            .await
            .map(|v| v.into_iter().map(S::Collection::into).collect())
    }

    async fn create_collection(&mut self, _href: &str) -> Result<Self::Collection> {
        Err(io::ErrorKind::ReadOnlyFilesystem.into())
    }

    async fn destroy_collection(&mut self, _href: &str) -> Result<()> {
        Err(io::ErrorKind::ReadOnlyFilesystem.into())
    }

    fn open_collection(&self, href: &str) -> Result<Self::Collection> {
        self.inner
            .open_collection(href)
            .map(|c| ReadOnlyCollection { inner: c })
    }
}

impl<C: Collection> Collection for ReadOnlyCollection<C> {
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

    async fn get_meta(&self, meta: crate::base::MetadataKind) -> Result<String> {
        self.inner.get_meta(meta).await
    }
}

impl<S: Storage> From<S> for ReadOnlyStorage<S> {
    fn from(value: S) -> Self {
        Self { inner: value }
    }
}

impl<C: Collection> From<C> for ReadOnlyCollection<C> {
    fn from(value: C) -> Self {
        Self { inner: value }
    }
}
