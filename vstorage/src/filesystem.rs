//! Implements reading/writing entries from a local filesystem [`vdir`].
//!
//! [`vdir`]: https://vdirsyncer.pimutils.org/en/stable/vdir.html
use async_std::{
    fs::{
        create_dir, metadata, read_dir, read_to_string, remove_dir_all, DirEntry, File, OpenOptions,
    },
    io::WriteExt,
    path::{Path, PathBuf},
    stream::{Stream, StreamExt},
};
use std::rc::Rc;
use std::{
    fs::Metadata,
    io::{Error, ErrorKind, Result},
    os::unix::prelude::MetadataExt,
};
use url::Url;

use crate::base::{Collection, Etag, Item, ItemRef, MetadataKind, Storage};

// TODO: atomic writes

/// A filesystem directory containing zero or more directories.
///
/// Each child directory is treated as `FilesystemCollection`]. Nested subdirectories are strictly
/// not supported.
pub struct FilesystemStorage {
    path: PathBuf,
    read_only: bool,
    metadata: Rc<FilesystemMetadata>,
}

impl Storage for FilesystemStorage {
    type Metadata = FilesystemMetadata;
    type Collection = FilesystemCollection;

    fn new(url: &Url, metadata: FilesystemMetadata, read_only: bool) -> Result<Self> {
        Ok(FilesystemStorage {
            path: path_from_url(url)?,
            read_only,
            metadata: Rc::from(metadata),
        })
    }

    async fn check(&self) -> Result<()> {
        let meta = metadata(&self.path).await?;

        if meta.is_dir() {
            Ok(())
        } else {
            Err(Error::from(ErrorKind::NotADirectory))
        }
    }

    async fn discover_collections(&self) -> Result<Vec<FilesystemCollection>> {
        let mut entries = read_dir(&self.path).await?;

        let mut collections = Vec::new();
        while let Some(entry) = entries.next().await {
            let entry = entry?;

            if !entry.metadata().await?.is_dir() {
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
                metadata: self.metadata.clone(),
            });
        }

        Ok(collections)
    }

    async fn create_collection(&mut self, href: &str) -> Result<FilesystemCollection> {
        let path = self.join_collection_href(href)?;
        create_dir(&path).await?;

        Ok(FilesystemCollection {
            dir_name: href.to_owned(),
            path,
            metadata: self.metadata.clone(),
        })
    }

    async fn destroy_collection(&mut self, href: &str) -> Result<()> {
        let path = self.join_collection_href(href)?;
        remove_dir_all(path).await
    }
}

impl FilesystemStorage {
    // Joins an href to the collection's path.
    //
    // Errors if the resulting path is not a child of the storage's directory.
    fn join_collection_href(&self, href: &str) -> Result<PathBuf> {
        let path = self.path.join(href);
        if path.parent() != Some(&self.path) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "directory is not child of storage directory",
            ));
        };

        Ok(path)
    }
}

/// Metadata for a storage instance.
pub struct FilesystemMetadata {
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
    metadata: Rc<FilesystemMetadata>,
}

impl Collection for FilesystemCollection {
    async fn list(&self) -> Result<Vec<ItemRef>> {
        let mut read_dir = read_dir(&self.path).await?;

        let mut items = Vec::with_capacity(read_dir.size_hint().0);
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
        let href = format!("{}.{}", href, self.metadata.extension);

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
}

fn path_from_url(url: &Url) -> Result<PathBuf> {
    let path = url
        .to_file_path()
        .map_err(|_| Error::from(ErrorKind::InvalidInput))?;

    Ok(path.into())
}

async fn etag_for_path<P: AsRef<Path>>(path: &Path) -> Result<Etag> {
    let metadata = path.metadata().await?;
    Ok(etag_for_metadata(&metadata))
}

async fn etag_for_direntry(dir_entry: &DirEntry) -> Result<Etag> {
    let metadata = dir_entry.metadata().await?;
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
