//! Traits and common implementations shared by different storages.
//!
//! When writing code that should deal with different storage implementations, these traits should
//! be used as input / outputs, rather than concrete per-store types.
//!
//! See [`Storage`] as an entry point to this module.

use sha2::{Digest, Sha256};
use std::io::Result;

use url::Url;

/// An identifier for a specific version of a resource.
///
/// Etags are bound to a specific storage. A storage SHOULD return the same `Etag` for an item as
/// long has not been modified. The `Etag` MUST change if the item has been modified.
///
/// This is inspired on the [HTTP header of the same name][MDN].
///
/// [MDN]: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/ETag
pub type Etag = String;

/// A storage is a the highest level abstraction where items can be stored. It can be a remote
/// CalDav server, a local filesystem, etc.
///
/// Storages may have one or more [`Collection`]s (e.g.: calendars).
pub trait Storage: Sized + Sync + Send {
    // XXX TODO FIXME: keep in mind item types
    // instances has one item type (e.g.: calendar only has todos)

    /// Implementation-specific metadata.
    ///
    /// This type carries configuration for storage instances, like TLS configuration for
    /// network-based storages, or file extensions for filesystem based storages.
    type Metadata; // TODO: should be serde::Serialize?

    /// Concrete collection type for this storage implementation.
    type Collection: Collection; // ??????

    /// Creates a new storage instance based on the given URL.
    fn new(url: &Url, metadata: Self::Metadata, read_only: bool) -> Result<Self>;

    /// Checks that the storage works. This includes validating credentials, and reachability.
    async fn check(&self) -> Result<()>;

    /// Finds existing collections for this storage.
    async fn discover_collections(&self) -> Result<Vec<Self::Collection>>;

    /// Creates a new collection.
    async fn create_collection(&mut self, href: &str) -> Result<Self::Collection>;

    /// Deletes an existing collection.
    async fn destroy_collection(&mut self, href: &str) -> Result<()>;

    /// Open an existing collection.
    ///
    /// This method DOES NOT check the existence of the collection. If existence needs to be
    /// verified, use [`discover_collections`] to enumerate all collections instead.
    ///
    /// [`discover_collections`]: Self::discover_collections
    fn open_collection(&self, href: &str) -> Result<Self::Collection>;

    // NOTE: to sync a single-collection storage to a multi-collection storage #name=XXXX to the URL.
    //       (e.g.: a webical calendar to a local storage)
}

/// A collection may be an "addressbook" or a "calendar".
///
/// The type of items contained is restricted by the underlying implementation.
///
/// Collections never cache data locally. For reading items in bulk, prefer [`get_many`].
///
/// [`get_many`]: Self::get_many
pub trait Collection: Sync + Send {
    /// A unique identifier for this collection.
    ///
    /// Href should not change over time, so should be associated with an immutable property of the
    /// collection (e.g.: a relative URL path, or a directory's filename).
    ///
    /// # Note for implementers
    ///
    /// It the underlying implementation has native immutable IDs for collections, that should
    /// always be preferred.
    fn id(&self) -> &str;

    /// The path to this collection inside the storage.
    ///
    /// This value can be used with [`Storage::open_collection`] to later access this same
    /// collection.
    ///
    /// The exact meaning of this value is storage-specific, but should be remain consistent.
    fn href(&self) -> &str;

    /// Enumerates items in this collection.
    async fn list(&self) -> Result<Vec<ItemRef>>;

    /// Fetch a single item.
    async fn get(&self, href: &str) -> Result<(Item, Etag)>;

    /// Fetch multiple items. Similar to [`Collection::get`], but optimised to minimise the amount of IO
    /// required. Duplicate `href`s will be ignored.
    // XXX: This API is kinda bad. It's very all or nothing. If an individual item has issues, the whole query fails.
    async fn get_many(&self, hrefs: &[&str]) -> Result<Vec<(Item, Etag)>>;

    /// Saves a new item into the collection
    async fn add(&mut self, item: &Item) -> Result<ItemRef>;

    /// Updates an existing item in the collection.
    async fn update(&mut self, href: &str, etag: &str, item: &Item) -> Result<Etag>;

    /// Sets the value of a property for this collection.
    async fn set_meta(&mut self, meta: MetadataKind, value: &str) -> Result<()>;

    /// Returns the value of a property for this collection.
    async fn get_meta(&self, meta: MetadataKind) -> Result<String>;

    // collections should have non-pub cache of UID->hrefs
    // can this be implemented for Collection?
}

/// A reference to an [`Item`] inside a collection.
pub struct ItemRef {
    pub href: String, // XXX: Am I sure this can only be utf8?
    pub etag: Etag,
}

/// Metadata types supported by storages.
///
/// See also [`Collection::set_meta`]
#[non_exhaustive]
#[derive(Copy, Clone)]
pub enum MetadataKind {
    /// A user-friendly name for a collection.
    /// It is recommended to show this name in user interfaces.
    DisplayName,
    /// Collections may have colours, and various clients will respect this when display the
    /// collection itself or items from the collection (e.g.: calendars may show calendar entries
    /// from a collection using this colour as highlight).
    Colour,
}

/// Immutable wrapper around a `VCALENDAR` or `VCARD`.
///
/// Note that this is not a proper validating parser for icalendar or vcard; it's a very simple
/// one with the sole purpose of extracing a UID. Proper parsing of components is out of scope,
/// since we want to sync potentially invalid items too.
#[derive(Debug)]
pub struct Item {
    pub(crate) raw: String,
}

impl Item {
    /// Returns a unique identifier for this item.
    ///
    /// The UID does not change when the item is modified. The UID must remain the same when the
    /// item is copied across storages and storage types.
    #[must_use]
    pub fn uid(&self) -> Option<String> {
        let mut lines = self.raw.split_terminator("\r\n");
        let mut uid = lines
            .find_map(|line| line.strip_prefix("UID:"))
            .map(String::from)?;

        // If the following lines start with a space or tab, they're a continuation of the UID.
        // See: https://www.rfc-editor.org/rfc/rfc5545#section-3.1
        lines
            .map_while(|line| line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')))
            .for_each(|part| uid.push_str(part));

        Some(uid)
    }

    /// Returns the hash of the raw content.
    /// This is usable for etags (and status file).
    #[must_use]
    fn hash(&self) -> String {
        // TODO: Need to keep in mind that:
        //  - Timezones may be renamed and that has no meaning.
        //  - Some props may be re-sorted, but the Item is still the same.
        //
        //  See vdirsyncer's vobject.py for details on this.
        format!("{:X}", Sha256::digest(&(self.raw)))
    }

    /// A unique identifier for this item. Is either the UID (if any), or the hash of its contents.
    #[must_use]
    pub fn ident(&self) -> String {
        self.uid().unwrap_or_else(|| self.hash())
    }

    /// Returns a new copy of this Item with the supplied UID.
    #[must_use]
    pub fn with_uid(&self, _new_uid: String) -> Self {
        // The logic in vdirsyncer/vobject.py::Item.with_uid seems pretty solid.
        // TODO: this really needs to be done, although its absence only blocks syncing broken items.
        todo!()
    }

    #[must_use]
    /// Returns the raw contents of this item.
    pub fn raw(&self) -> &str {
        &self.raw
    }
}

#[cfg(test)]
mod tests {
    // Note: Some of these examples are NOT valid vcards.
    // vdirsyncer is expected to handle invalid input gracefully and sync it as-is,
    // so this is not really a problem.

    use super::Item;

    fn item_from_raw(raw: String) -> Item {
        Item { raw }
    }

    #[test]
    fn test_single_line_uid() {
        let raw = ["BEGIN:VCARD", "UID:hello", "END:VCARD"].join("\r\n");
        let item = item_from_raw(raw);
        assert_eq!(item.uid(), Some(String::from("hello")));
        assert_eq!(item.ident(), String::from("hello"));

        let raw = ["BEGIN:VCARD", "UID:hel", "lo", "END:VCARD"].join("\r\n");
        let item = item_from_raw(raw);
        assert_eq!(item.uid(), Some(String::from("hel")));
        assert_eq!(item.ident(), String::from("hel"));

        let raw = [
            "BEGIN:VCARD",
            "UID:hello",
            "REV:20210307T195614Z\tthere",
            "END:VCARD",
        ]
        .join("\r\n");
        let item = item_from_raw(raw);
        assert_eq!(item.uid(), Some(String::from("hello")));
        assert_eq!(item.ident(), String::from("hello"));
    }

    #[test]
    fn test_multi_line_uid() {
        let raw = ["BEGIN:VCARD", "UID:hello", "\tthere", "END:VCARD"].join("\r\n");
        let item = item_from_raw(raw);
        assert_eq!(item.uid(), Some(String::from("hellothere")));
        assert_eq!(item.ident(), String::from("hellothere"));

        let raw = [
            "BEGIN:VCARD",
            "UID:hello",
            "\tthere",
            "REV:20210307T195614Z",
            "\tnope",
            "END:VCARD",
        ]
        .join("\r\n");
        let item = item_from_raw(raw);
        assert_eq!(item.uid(), Some(String::from("hellothere")));
        assert_eq!(item.ident(), String::from("hellothere"));
    }

    #[test]
    fn test_missing_uid() {
        let raw = [
            "BEGIN:VCARD",
            "UIDX:hello",
            "REV:20210307T195614Z\tthere",
            "END:VCARD",
        ]
        .join("\r\n");
        let item = item_from_raw(raw);
        assert_eq!(item.uid(), None);
        assert_eq!(
            item.ident(),
            "23A1B4246052E5BBB7AED65EDD759EBB03EF314DB055C109716D0301F9AC8E19"
        );
    }
}
