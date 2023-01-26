//! Implements reading/writing entries from a local filesystem [`vdir`].
//!
//! [`vdir`]: https://vdirsyncer.pimutils.org/en/stable/vdir.html
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

use crate::base::{Collection, Etag, Item, ItemRef, MetadataKind, Storage};

// TODO: atomic writes

/// A filesystem directory containing zero or more directories.
///
/// Each child directory is treated as `FilesystemCollection`]. Nested subdirectories are strictly
/// not supported.
pub struct FilesystemStorage {
    definition: Arc<FilesystemDefinition>,
    read_only: bool,
}

impl Storage for FilesystemStorage {
    type Definition = FilesystemDefinition;
    type Collection = FilesystemCollection;

    fn new(definition: FilesystemDefinition, read_only: bool) -> Result<Self> {
        Ok(FilesystemStorage {
            definition: Arc::from(definition),
            read_only,
        })
    }

    async fn check(&self) -> Result<()> {
        let meta = metadata(&self.definition.path).await?;

        if meta.is_dir() {
            Ok(())
        } else {
            Err(Error::from(ErrorKind::NotADirectory))
        }
    }

    async fn discover_collections(&self) -> Result<Vec<FilesystemCollection>> {
        let mut entries = read_dir(&self.definition.path).await?;

        let mut collections = Vec::new();
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

            collections.push(FilesystemCollection {
                dir_name,
                path: entry.path(),
                definition: self.definition.clone(),
            });
        }

        Ok(collections)
    }

    async fn create_collection(&mut self, href: &str) -> Result<FilesystemCollection> {
        let collection = self.open_collection(href)?;
        create_dir(&collection.path).await?;
        Ok(collection)
    }

    async fn destroy_collection(&mut self, href: &str) -> Result<()> {
        let path = self.join_collection_href(href)?;
        remove_dir_all(path).await
    }

    fn open_collection(&self, href: &str) -> Result<Self::Collection> {
        let path = self.join_collection_href(href)?;

        Ok(FilesystemCollection {
            dir_name: href.to_owned(),
            path,
            definition: self.definition.clone(),
        })
    }
}

impl FilesystemStorage {
    // Joins an href to the collection's path.
    //
    // Errors if the resulting path is not a child of the storage's directory.
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

/// A collection backed by a filesystem directory.
///
/// See documentation for [`vdir`](https://vdirsyncer.pimutils.org/en/stable/vdir.html) for
/// details.
pub struct FilesystemCollection {
    dir_name: String,
    path: PathBuf,
    definition: Arc<FilesystemDefinition>,
}

impl Collection for FilesystemCollection {
    async fn list(&self) -> Result<Vec<ItemRef>> {
        let mut read_dir = read_dir(&self.path).await?;

        let mut items = Vec::new();
        while let Ok(entry) = read_dir.next_entry().await {
            let entry = entry.unwrap();
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

        let item = Item {
            raw: read_to_string(&path).await?,
        };
        let etag = etag_for_metadata(&meta);

        Ok((item, etag))
    }

    async fn get_many(&self, hrefs: &[&str]) -> Result<Vec<(Item, Etag)>> {
        // No specialisation for this type; it's fast enough for now.
        let mut items = Vec::with_capacity(hrefs.len());
        for href in hrefs {
            items.push(self.get(href).await?);
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

    async fn get_meta(&self, meta: MetadataKind) -> Result<String> {
        let filename = filename_for_collection_meta(meta);

        let path = self.path.join(filename);
        let value = read_to_string(path).await?;

        Ok(value)
    }

    async fn add(&mut self, item: &Item) -> Result<ItemRef> {
        let href = item.ident();
        // TODO: check that href is a valid href, else, use a uuid.
        let href = format!("{}.{}", href, self.definition.extension);

        let filename = self.path.join(&href);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&filename)
            .await?;
        file.write_all(item.raw.as_bytes()).await?;

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
        file.write_all(item.raw.as_bytes()).await?;

        let etag = etag_for_path::<PathBuf>(&filename).await?;
        Ok(etag)
    }

    fn id(&self) -> &str {
        self.dir_name.as_str()
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
    // TODO: helper to create storage in tmpdir (auto-delete on drop? Is there a dedicated helper?)

    // #[test]
    // fn test_write_read_meta_name() {
    //     todo!();
    // }

    // TODO: test writing and then checking the file
    // TODO: test writing a file and then getting
}
