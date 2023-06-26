//! Traits and common implementations shared by different storages.
//!
//! When writing code that should deal with different storage implementations, these traits should
//! be used as input / outputs, rather than concrete per-store types.
//!
//! See [`Storage`] as an entry point to this module.

use async_trait::async_trait;

use crate::{Etag, Href, Result};

/// Implementation-specific storage definition.
///
/// This type carries any configuration required to define a storage instances. This include
/// this like URL or TLS for network-based storages, or path and file extensions for filesystem
/// based storages.
#[async_trait]
pub trait Definition<I: Item>: Sync + Send + std::fmt::Debug {
    /// Creates a new storage instance for this definition.
    ///
    /// # Errors
    ///
    /// Errors are implementation-dependant; see implementations for details.
    async fn storage(self) -> Result<Box<dyn Storage<I>>>;
}

/// A storage is the highest level abstraction where items can be stored. It can be a remote CalDav
/// account, a local filesystem, etc.
///
/// Each storage may contain one or more [`Collection`]s (e.g.: calendars or address books).
///
/// The specific type of item that a storage can hold is defined by the `I` generic parameter.
/// E.g.: a CalDav storage can hold icalendar items. Only items with the same kind of item can be
/// synchronised (e.g.: it it nos possible to synchronise `Storage<VCardItem>` with
/// `Storage<IcsItem>`
///
/// # Note for implementors
///
/// The auto-generated documentation for this trait is rather hard to read due to the usage of
/// `#[async_trait]`. You might want to consider clicking on the `source` link on the right and
/// reading the documentation from the raw code for this trait.
#[async_trait]
pub trait Storage<I: Item>: Sync + Send {
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
    async fn get_item(&self, collection: &Collection, href: &str) -> Result<(I, Etag)>;

    /// Fetch multiple items.
    ///
    /// Similar to [`Storage::get_item`], but optimised to minimise the amount of IO required.
    /// Duplicate `href`s will be ignored.
    async fn get_many_items(
        &self,
        collection: &Collection,
        hrefs: &[&str],
    ) -> Result<Vec<(Href, I, Etag)>>;

    /// Fetch all items from a given collection.
    // TODO: provide a generic implementation.
    async fn get_all_items(&self, collection: &Collection) -> Result<Vec<(Href, I, Etag)>>;

    /// Saves a new item into a given collection
    async fn add_item(&mut self, collection: &Collection, item: &I) -> Result<ItemRef>;

    /// Updates an existing item in a given collection.
    async fn update_item(
        &mut self,
        collection: &Collection,
        href: &str,
        etag: &Etag,
        item: &I,
    ) -> Result<Etag>;

    async fn delete_item(&mut self, collection: &Collection, href: &str, etag: &Etag)
        -> Result<()>;

    /// A name that does not change for this collection.
    ///
    /// Usually this is based off the last component of the href, but may be different for storages
    /// where this does not make sense.
    ///
    /// When synchronising, collections with the same name will be mapped to each other.
    fn collection_id(&self, collection: &Collection) -> Result<String>;

    // XXX: collections should have non-pub cache of UID->hrefs
    // XXX: can this be implemented for Collection?
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
// TODO: meta-kind should be generic over the ContentType(Cal, Card, IMAP, etc).
pub enum MetadataKind {
    /// A user-friendly name for a collection.
    /// It is recommended to show this name in user interfaces.
    DisplayName,
    /// Collections may have colours, and various clients will respect this when display the
    /// collection itself or items from the collection (e.g.: calendars may show calendar entries
    /// from a collection using this colour as highlight).
    Colour,
}

/// Types of items that can be held in collections.
///
/// Storages can contain items of a concrete type implementing this trait. This trait defines how
/// to extract the basic information that is requires to synchronise storages. Additional parsing
/// is out of scope here and should be done by inspecting the raw data inside an item via
/// [`Item::as_str`].
pub trait Item: Sync + Send + std::fmt::Debug
where
    Self: From<String>,
{
    /// Parse the item and return a unique identifier for it.
    ///
    /// The UID does not change when the item is modified. The UID must remain the same when the
    /// item is copied across storages and storage types.
    #[must_use]
    fn uid(&self) -> Option<String>;

    /// Return the hash of this item.
    ///
    /// Implementations may normalise content before hashing to ensure that two equivalent items
    /// return the same hash.
    #[must_use]
    fn hash(&self) -> String;

    /// A unique identifier for this item. Is either the UID (if any), or the hash of its contents.
    #[must_use]
    fn ident(&self) -> String;

    /// Returns a new copy of this Item with the supplied UID.
    #[must_use]
    fn with_uid(&self, new_uid: &str) -> Self;

    #[must_use]
    /// Returns the raw contents of this item.
    fn as_str(&self) -> &str;
}

/// Immutable wrapper around a `VCALENDAR` or `VCARD`.
///
/// Note that this is not a proper validating parser for icalendar or vcard; it's a very simple
/// one with the sole purpose of extracing a UID. Proper parsing of components is out of scope,
/// since we want to enable operating on potentially invalid items too.
#[derive(Debug)]
pub struct IcsItem {
    // TODO: make this Vec<u8> instead?
    raw: String,
}

impl Item for IcsItem {
    /// Returns a unique identifier for this item.
    ///
    /// The UID does not change when the item is modified. The UID must remain the same when the
    /// item is copied across storages and storage types.
    #[must_use]
    fn uid(&self) -> Option<String> {
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
    /// [`Etag`]: crate::Etag
    #[must_use]
    fn hash(&self) -> String {
        // TODO: Need to keep in mind that:
        //  - Timezones may be renamed and that has no meaning.
        //  - Some props may be re-sorted, but the Item is still the same.
        //
        //  See vdirsyncer's vobject.py for details on this.
        crate::util::hash(&self.raw)
    }

    /// A unique identifier for this item. Is either the UID (if any), or the hash of its contents.
    #[must_use]
    fn ident(&self) -> String {
        self.uid().unwrap_or_else(|| self.hash())
    }

    /// Returns a new copy of this Item with the supplied UID.
    ///
    /// # Panics
    ///
    /// This function is not yet implemented.
    #[must_use]
    fn with_uid(&self, _new_uid: &str) -> Self {
        // The logic in vdirsyncer/vobject.py::Item.with_uid seems pretty solid.
        // TODO: this really needs to be done, although its absence only blocks syncing broken items.
        todo!()
    }

    #[inline]
    #[must_use]
    /// Returns the raw contents of this item.
    fn as_str(&self) -> &str {
        &self.raw
    }
}

impl From<String> for IcsItem {
    fn from(value: String) -> Self {
        IcsItem { raw: value }
    }
}

#[cfg(test)]
mod tests {
    // Note: Some of these examples are NOT valid vcards.
    // vdirsyncer is expected to handle invalid input gracefully and sync it as-is,
    // so this is not really a problem.

    use crate::base::{IcsItem, Item, Storage};

    fn item_from_raw(raw: String) -> IcsItem {
        IcsItem { raw }
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
        fn dummy(_: Box<dyn Storage<IcsItem>>) {}
    }
}
