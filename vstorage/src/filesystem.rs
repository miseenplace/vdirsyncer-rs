//! Implements reading/writing entries from a local filesystem [`vdir`].
//!
//! [`vdir`]: https://vdirsyncer.pimutils.org/en/stable/vdir.html
#![allow(clippy::module_name_repetitions)]

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{
    fs::Metadata,
    io::{Error, ErrorKind, Result},
    os::unix::prelude::MetadataExt,
};
use tokio::fs::{
    create_dir, metadata, read_dir, read_to_string, remove_dir_all, DirEntry, File, OpenOptions,
};
use tokio::io::AsyncWriteExt;
use tokio_stream::wrappers::ReadDirStream;
use tokio_stream::StreamExt;

use crate::base::{Collection, Definition, Etag, Href, Item, ItemRef, MetadataKind, Storage};

// TODO: atomic writes

/// A filesystem directory containing zero or more directories.
///
/// Each child directory is treated as `FilesystemCollection`]. Nested subdirectories are strictly
/// not supported.
pub struct FilesystemStorage {
    definition: Arc<FilesystemDefinition>,
}

#[async_trait]
impl Storage for FilesystemStorage {
    async fn check(&self) -> Result<()> {
        let meta = metadata(&self.definition.path).await?;

        if meta.is_dir() {
            Ok(())
        } else {
            Err(Error::from(ErrorKind::NotADirectory))
        }
    }

    async fn discover_collections(&self) -> Result<Vec<Box<dyn Collection>>> {
        let mut entries = read_dir(&self.definition.path).await?;

        let mut collections = Vec::<Box<dyn Collection>>::new();
        while let Ok(entry) = entries.next_entry().await {
            let entry = entry.unwrap();

            if !metadata(entry.path()).await?.is_dir() {
                continue;
            }
            let dir_name = entry
                .file_name()
                .to_str()
                .ok_or_else(|| {
                    Error::new(ErrorKind::InvalidFilename, "collection name is not utf8")
                })?
                .to_owned();

            collections.push(Box::from(FilesystemCollection {
                dir_name,
                path: entry.path(),
                definition: self.definition.clone(),
            }));
        }

        Ok(collections)
    }

    async fn create_collection(&mut self, href: &str) -> Result<Box<dyn Collection>> {
        let path = self.join_collection_href(href)?;
        create_dir(&path).await?;

        Ok(Box::from(FilesystemCollection {
            dir_name: href.to_owned(),
            path,
            definition: self.definition.clone(),
        }))
    }

    async fn destroy_collection(&mut self, href: &str) -> Result<()> {
        let path = self.join_collection_href(href)?;
        remove_dir_all(path).await
    }

    fn open_collection(&self, href: &str) -> Result<Box<dyn Collection>> {
        let path = self.join_collection_href(href)?;

        Ok(Box::from(FilesystemCollection {
            dir_name: href.to_owned(),
            path,
            definition: self.definition.clone(),
        }))
    }
}

impl FilesystemStorage {
    // Joins an href to the collection's path.
    //
    // # Errors
    //
    // If the resulting path is not a child of the storage's directory.
    fn join_collection_href(&self, href: &str) -> Result<PathBuf> {
        let path = self.definition.path.join(href);
        if path.parent() != Some(&self.definition.path) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "directory is not child of storage directory",
            ));
        };

        Ok(path)
    }
}

/// Definition for a storage instance.
pub struct FilesystemDefinition {
    pub path: PathBuf,
    /// Filename extension for items in a storage. Files with matching extension are treated a
    /// items for a collection, and all other files are ignored.
    pub extension: String,
}

#[async_trait]
impl Definition for FilesystemDefinition {
    async fn storage(self) -> Result<Box<dyn Storage>> {
        Ok(Box::from(FilesystemStorage {
            definition: Arc::from(self),
        }))
    }
}

/// A collection backed by a filesystem directory.
///
/// See documentation for [`vdir`](https://vdirsyncer.pimutils.org/en/stable/vdir.html) for
/// details.
pub struct FilesystemCollection {
    dir_name: String,
    path: PathBuf,
    definition: Arc<FilesystemDefinition>,
}

#[async_trait]
impl Collection for FilesystemCollection {
    async fn list(&self) -> Result<Vec<ItemRef>> {
        let mut read_dir = ReadDirStream::new(read_dir(&self.path).await?);

        let mut items = Vec::new();
        while let Some(entry) = read_dir.next().await {
            let entry = entry?;
            let href = entry
                .file_name()
                .to_str()
                .ok_or_else(|| Error::new(ErrorKind::InvalidData, "Filename is not valid UTF-8"))?
                .into();
            let etag = etag_for_direntry(&entry).await?;
            let item = ItemRef { href, etag };
            items.push(item);
        }

        Ok(items)
    }

    async fn get(&self, href: &str) -> Result<(Item, Etag)> {
        let path = self.path.join(href);
        let meta = metadata(&path).await?;

        let item = Item::from(read_to_string(&path).await?);
        let etag = etag_for_metadata(&meta);

        Ok((item, etag))
    }

    async fn get_many(&self, hrefs: &[&str]) -> Result<Vec<(Href, Item, Etag)>> {
        // No specialisation for this type; it's fast enough for now.
        let mut items = Vec::with_capacity(hrefs.len());
        for href in hrefs {
            let (item, etag) = self.get(href).await?;
            items.push((String::from(*href), item, etag));
        }
        Ok(items)
    }

    async fn get_all(&self) -> Result<Vec<(Href, Item, Etag)>> {
        let mut read_dir = read_dir(&self.path).await?;

        let mut items = Vec::new();
        while let Ok(entry) = read_dir.next_entry().await {
            let entry = entry.unwrap();
            let href: String = entry
                .file_name()
                .to_str()
                .ok_or_else(|| Error::new(ErrorKind::InvalidData, "Filename is not valid UTF-8"))?
                .into();
            let etag = etag_for_direntry(&entry).await?;
            let item = Item::from(read_to_string(&href).await?);
            items.push((href, item, etag));
        }

        Ok(items)
    }

    async fn set_meta(&mut self, meta: MetadataKind, value: &str) -> Result<()> {
        let filename = filename_for_collection_meta(meta);

        let path = self.path.join(filename);
        let mut file = File::create(path).await?;

        file.write_all(value.as_bytes()).await?;
        Ok(())
    }

    async fn get_meta(&self, meta: MetadataKind) -> Result<Option<String>> {
        let filename = filename_for_collection_meta(meta);

        let path = self.path.join(filename);
        let value = match read_to_string(path).await {
            Ok(data) => data,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };

        Ok(Some(value))
    }

    async fn add(&mut self, item: &Item) -> Result<ItemRef> {
        // TODO: We only need to remove a few "illegal" characters, so this is a bit too strict.
        let basename = item
            .ident()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>();
        let href = format!("{}.{}", basename, self.definition.extension);

        let filename = self.path.join(&href);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&filename)
            .await?;
        file.write_all(item.as_str().as_bytes()).await?;

        let item_ref = ItemRef {
            href,
            etag: etag_for_path::<PathBuf>(&filename).await?,
        };
        Ok(item_ref)
    }

    async fn update(&mut self, href: &str, etag: &str, item: &Item) -> Result<Etag> {
        let filename = self.path.join(href);

        let actual_etag = etag_for_path::<PathBuf>(&filename).await?;
        if etag != actual_etag {
            return Err(Error::new(ErrorKind::InvalidData, "wrong etag"));
        }

        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(false)
            .open(&filename)
            .await?;
        file.write_all(item.as_str().as_bytes()).await?;

        let etag = etag_for_path::<PathBuf>(&filename).await?;
        Ok(etag)
    }

    fn id(&self) -> &str {
        &self.dir_name
    }

    fn href(&self) -> &str {
        &self.dir_name
    }
}

async fn etag_for_path<P: AsRef<Path>>(path: &Path) -> Result<Etag> {
    let metadata = metadata(path).await?;
    Ok(etag_for_metadata(&metadata))
}

async fn etag_for_direntry(dir_entry: &DirEntry) -> Result<Etag> {
    let metadata = metadata(dir_entry.path()).await?;
    Ok(etag_for_metadata(&metadata))
}

fn etag_for_metadata(metadata: &Metadata) -> Etag {
    format!("{};{}", metadata.mtime(), metadata.ino())
}

fn filename_for_collection_meta(kind: MetadataKind) -> &'static str {
    match kind {
        MetadataKind::DisplayName => "displayname",
        MetadataKind::Colour => "color",
    }
}

#[cfg(test)]
mod tests {
    use super::FilesystemDefinition;
    use crate::base::Definition;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_missing_displayname() {
        let dir = tempdir().unwrap();
        let definition = FilesystemDefinition {
            path: dir.path().to_path_buf(),
            extension: "ics".to_string(),
        };

        let mut storage = definition.storage().await.unwrap();
        let collection = storage.create_collection("test").await.unwrap();
        let displayname = collection
            .get_meta(crate::base::MetadataKind::DisplayName)
            .await
            .unwrap();

        assert!(displayname.is_none())
    }

    // #[test]
    // fn test_write_read_meta_name() {
    //     todo!();
    // }

    // TODO: test writing and then checking the file
    // TODO: test writing a file and then getting
}
