//! Traits and common implementations shared by different storages.
//!
//! When writing code that should deal with different storage implementations, these traits should
//! be used as input / outputs, rather than concrete per-store types.
//!
//! See [`Storage`] as an entry point to this module.

use crate::Result;

use async_trait::async_trait;

/// An identifier for a specific version of a resource.
///
/// Etags are bound to a specific storage. A storage SHOULD return the same `Etag` for an item as
/// long has not been modified. The `Etag` MUST change if the item has been modified.
///
/// This is inspired on the [HTTP header of the same name][MDN].
///
/// [MDN]: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/ETag
pub type Etag = String;

/// The path to the item inside the collection.
///
/// For example, for carddav collections this is the path of the entry inside the collection. For
/// Filesystem, this the file's relative path, etc. `Href`s MUST be valid UTF-8 sequences.
///
/// Whether an `href` is relative to a collection or absolute is storage dependant. As such, this
/// should be treated as an opaque string by consumers of this library.
pub type Href = String;

/// Implementation-specific storage definition.
///
/// This type carries any configuration required to define a storage instances. This include
/// this like URL or TLS for network-based storages, or path and file extensions for filesystem
/// based storages.
#[async_trait]
pub trait Definition: Sync + Send {
    /// Creates a new storage instance for this definition.
    ///
    /// # Errors
    ///
    /// Errors are implementation-dependant; see implementations for details.
    async fn storage(self) -> Result<Box<dyn Storage>>;
}

/// A storage is the highest level abstraction where items can be stored. It can be a remote CalDav
/// account, a local filesystem, etc.
///
/// Each storage may contain one or more [`Collection`]s (e.g.: calendars or address books).
#[async_trait]
pub trait Storage: Sync + Send {
    // TODO: Will eventually need to support non-icalendar things here.
    // TODO: Some calendar instances only allow a single item type (e.g.: events but not todos).

    /// Checks that the storage works. This includes validating credentials, and reachability.
    async fn check(&self) -> Result<()>;

    /// Finds existing collections for this storage.
    async fn discover_collections(&self) -> Result<Vec<Collection>>;

    /// Creates a new collection.
    async fn create_collection(&mut self, href: &str) -> Result<Collection>;

    /// Deletes an existing collection.
    ///
    /// A collection must be empty for deletion to succeed.
    async fn destroy_collection(&mut self, href: &str) -> Result<()>;

    /// Open an existing collection.
    ///
    /// This method DOES NOT check the existence of the collection.
    fn open_collection(&self, href: &str) -> Result<Collection>;

    /// Returns the value of a property for a given collection.
    async fn get_collection_meta(
        &self,
        collection: &Collection,
        meta: MetadataKind,
    ) -> Result<Option<String>>;

    /// Sets the value of a property for a given collection.
    async fn set_collection_meta(
        &mut self,
        collection: &Collection,
        meta: MetadataKind,
        value: &str,
    ) -> Result<()>;

    /// Enumerates items in a given collection.
    async fn list_items(&self, collection: &Collection) -> Result<Vec<ItemRef>>;

    /// Fetch a single item from given collection.
    async fn get_item(&self, collection: &Collection, href: &str) -> Result<(Item, Etag)>;

    /// Fetch multiple items.
    ///
    /// Similar to [`Storage::get_item`], but optimised to minimise the amount of IO required.
    /// Duplicate `href`s will be ignored.
    async fn get_many_items(
        &self,
        collection: &Collection,
        hrefs: &[&str],
    ) -> Result<Vec<(Href, Item, Etag)>>;

    /// Fetch all items from a given collection.
    // TODO: provide a generic implementation.
    async fn get_all_items(&self, collection: &Collection) -> Result<Vec<(Href, Item, Etag)>>;

    /// Saves a new item into a given collection
    async fn add_item(&mut self, collection: &Collection, item: &Item) -> Result<ItemRef>;

    /// Updates an existing item in a given collection.
    async fn update_item(
        &mut self,
        collection: &Collection,
        href: &str,
        etag: &str,
        item: &Item,
    ) -> Result<Etag>;

    async fn delete_item(&mut self, collection: &Collection, href: &str, etag: &str) -> Result<()>;

    // collections should have non-pub cache of UID->hrefs
    // can this be implemented for Collection?
}

/// A collection may, for example, be an address book or a calendar.
///
/// The type of items contained is restricted by the underlying implementation. Collections contain
/// zero or more items (e.g.: an address book contains events). Each item is addressed by an
/// [`Href`].
///
/// Collections never cache data locally. For reading items in bulk, prefer
/// [`Storage::get_many_items`].
pub struct Collection {
    href: String,
}

impl Collection {
    /// The path to this collection inside the storage.
    ///
    /// This value can be used with [`Storage::open_collection`] to later access this same
    /// collection.
    ///
    /// Href should not change over time, so should be associated with an immutable property of the
    /// collection (e.g.: a relative URL path, or a directory's filename).
    ///
    /// The exact meaning of this value is storage-specific, but should be remain consistent with a
    /// storage.
    #[must_use]
    pub fn href(&self) -> &str {
        &self.href
    }

    pub(crate) fn new(href: String) -> Collection {
        Collection { href }
    }
}

/// A reference to an [`Item`] inside a collection.
pub struct ItemRef {
    pub href: String, // TODO: This should be parametrized, or I should document the restriction.
    pub etag: Etag,
}

/// Metadata types supported by storages.
///
/// See also [`Storage::get_collection_meta`] and [`Storage::set_collection_meta`].
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
/// since we want to enable operating on potentially invalid items too.
#[derive(Debug)]
pub struct Item {
    raw: String,
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
    ///
    /// This does some minimal normalisations of the items before hashing:
    ///
    /// - Ignores the `PROPID` field. Two item where only this field varies are
    ///   considered equivalent.
    ///
    /// This is used as a fallback when a storage backend doesn't provide [`Etag`] values, or when
    /// an item is missing its `UID`.
    ///
    /// [`util::hash`]: crate::util::hash
    /// [`Etag`]: crate::base::Etag
    #[must_use]
    pub fn hash(&self) -> String {
        // TODO: Need to keep in mind that:
        //  - Timezones may be renamed and that has no meaning.
        //  - Some props may be re-sorted, but the Item is still the same.
        //
        //  See vdirsyncer's vobject.py for details on this.
        crate::util::hash(&self.raw)
    }

    /// A unique identifier for this item. Is either the UID (if any), or the hash of its contents.
    #[must_use]
    pub fn ident(&self) -> String {
        self.uid().unwrap_or_else(|| self.hash())
    }

    /// Returns a new copy of this Item with the supplied UID.
    ///
    /// # Panics
    ///
    /// This function is not yet implemented.
    #[must_use]
    pub fn with_uid(&self, _new_uid: &str) -> Self {
        // The logic in vdirsyncer/vobject.py::Item.with_uid seems pretty solid.
        // TODO: this really needs to be done, although its absence only blocks syncing broken items.
        todo!()
    }

    #[inline]
    #[must_use]
    /// Returns the raw contents of this item.
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl<S: AsRef<str>> From<S> for Item {
    fn from(value: S) -> Item {
        Item {
            raw: value.as_ref().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    // Note: Some of these examples are NOT valid vcards.
    // vdirsyncer is expected to handle invalid input gracefully and sync it as-is,
    // so this is not really a problem.

    use super::Item;
    use crate::base::Storage;

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

    #[test]
    fn test_storage_is_object_safe() {
        #[allow(dead_code)]
        fn dummy(_: Box<dyn Storage>) {}
    }
}
