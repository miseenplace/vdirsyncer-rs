//! Implements reading entries from a remote webcal resource.
//!
//! Webcal is a de-facto standard, and is basically a single icalendar file hosted via http(s).
//!
//! See the [Webcal wikipedia page](https://en.wikipedia.org/wiki/Webcal).
use reqwest::StatusCode;
use std::{
    io::{Error, ErrorKind, Result},
    sync::Arc,
};
use url::Url;

use crate::{
    base::{Collection, Etag, Href, Item, ItemRef, MetadataKind, Storage},
    simple_component::Component,
};

/// A storage which exposes items in remote icalendar resource.
///
/// A webcal storage contains exactly one collection, which contains all the entires found in the
/// remote resource. The name of this single collection must be specified via the
/// [`WebCalDefinition::collection_name`] property.
///
/// This storage is a bit of an odd one (since in reality, there's no concept of collections in
/// webcal. The extra abstraction layer is here merely to match the format of other storages.
pub struct WebCalStorage {
    definition: Arc<WebCalDefinition>,
    client: reqwest::Client,
}

/// Definition for a [`WebCalStorage`].
#[derive(Debug, PartialEq)]
pub struct WebCalDefinition {
    /// The URL of the remote icalendar resource. Must be HTTP or HTTPS.
    pub url: Url,
    /// The name to be given to the single collection available.
    pub collection_name: String,
}

impl Storage for WebCalStorage {
    type Definition = WebCalDefinition;

    type Collection = WebCalCollection;

    /// Create a new storage instance.
    ///
    /// Unlike other [`Storage`] implementations, this one allows only a single collection.
    fn new(definition: Self::Definition) -> Result<Self> {
        // NOTE: It would be nice to support `webcal://` here, but the Url crate won't allow
        // changing the scheme of such a Url.
        if !["http", "https"].contains(&definition.url.scheme()) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "URL scheme must be http or https",
            ));
        };
        Ok(WebCalStorage {
            definition: Arc::new(definition),
            client: reqwest::Client::new(),
        })
    }

    /// Checks that the remove resource exists and whether it looks like an icalendar resource.
    async fn check(&self) -> Result<()> {
        // TODO: Should map status codes to io::Error. if 404 -> NotFound, etc.
        let raw = fetch_raw(&self.client, &self.definition.url).await?;

        if !raw.starts_with("BEGIN:VCALENDAR") {
            return Err(Error::new(
                ErrorKind::Other,
                "response for URL doesn't look like a calendar",
            ));
        }
        Ok(())
    }

    /// Returns a single collection with the name specified in the definition.
    async fn discover_collections(&self) -> Result<Vec<Self::Collection>> {
        Ok(vec![WebCalCollection {
            definition: self.definition.clone(),
            client: self.client.clone(),
        }])
    }

    /// Unsupported for this storage type.
    async fn create_collection(&mut self, _: &str) -> Result<Self::Collection> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "creating collections via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn destroy_collection(&mut self, _: &str) -> Result<()> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "creating collections via webcal is not supported",
        ))
    }

    /// Usable only with the collection name specified in the definition. Any other name will
    /// return [`ErrorKind::NotFound`]
    fn open_collection(&self, href: &str) -> Result<Self::Collection> {
        if href != self.definition.collection_name {
            return Err(Error::new(
                ErrorKind::NotFound,
                format!("this storage only contains the '{href}' collection"),
            ));
        }
        Ok(WebCalCollection {
            definition: self.definition.clone(),
            client: self.client.clone(),
        })
    }
}

/// A collection of items in a webcal storage.
///
/// For this collection type, the `Href` is the UID of the entries. There is no other way to
/// address individual entries, so this is essentially the only choice.
///
/// The fact that `Href = UID` is a quirk specific to this storage type, and should not be relied
/// upon in general.
#[derive(Debug)]
pub struct WebCalCollection {
    definition: Arc<WebCalDefinition>,
    client: reqwest::Client,
}

impl PartialEq for &WebCalCollection {
    fn eq(&self, other: &Self) -> bool {
        self.definition.eq(&other.definition)
    }
}

impl Collection for WebCalCollection {
    /// Enumerates items in this collection.
    ///
    /// Note that, due to the nature of webcal, the whole collection needs to be retrieved. If some
    /// items need to be read as well, it is generally best to use [`WebCalCollection::get_all`] instead.
    async fn list(&self) -> Result<Vec<ItemRef>> {
        let raw = fetch_raw(&self.client, &self.definition.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let calendar = Component::parse(raw);
        let refs = calendar
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .subcomponents
            .iter()
            .map(|c| {
                let item = Item { raw: c.raw() };

                ItemRef {
                    href: item.ident(),
                    etag: crate::util::hash(c.raw()),
                }
            })
            .collect();

        Ok(refs)
    }

    /// Returns a single item from the collection.
    ///
    /// Note that, due to the nature of webcal, the whole collection needs to be retrieved. It is
    /// strongly recommended to use [`WebCalCollection::get_all`] instead.
    ///
    /// [`get_many`]: crate::base::Collection::get_many
    async fn get(&self, href: &str) -> Result<(Item, Etag)> {
        let raw = fetch_raw(&self.client, &self.definition.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let calendar = Component::parse(raw);
        let components = calendar
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .subcomponents;
        let item = components
            .iter()
            .find_map(|c| {
                let item = Item { raw: c.raw() };
                if item.ident() == href {
                    Some(item)
                } else {
                    None
                }
            })
            .ok_or_else(|| Error::from(ErrorKind::NotFound))?;

        let hash = crate::util::hash(&item.raw);
        Ok((item, hash))
    }

    /// Returns multiple items from the collection.
    ///
    /// Note that, due to the nature of webcal, the whole collection needs to be retrieved. It is
    /// generally best to use [`WebCalCollection::get_all`] instead.
    async fn get_many(&self, hrefs: &[&str]) -> Result<Vec<(Item, Etag)>> {
        let raw = fetch_raw(&self.client, &self.definition.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let calendar = Component::parse(raw);
        let components = calendar
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .subcomponents;

        components
            .iter()
            .filter_map(|c| {
                let raw = c.raw();
                let item = Item { raw };
                if hrefs.contains(&(item.ident().as_ref())) {
                    let hash = crate::util::hash(&item.raw);
                    Some(Ok((item, hash)))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Fetch all items in the collection.
    ///
    /// Performs a single HTTP(s) request to fetch all items.
    async fn get_all(&self) -> Result<Vec<(Href, Item, Etag)>> {
        let raw = fetch_raw(&self.client, &self.definition.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let calendar = Component::parse(raw);
        let components = calendar
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .subcomponents;

        components
            .iter()
            .map(|c| {
                let raw = c.raw();
                let hash = crate::util::hash(&raw);
                let item = Item { raw };

                Ok((item.ident(), item, hash))
            })
            .collect()
    }

    /// Unsupported for this storage type.
    async fn add(&mut self, _: &Item) -> Result<ItemRef> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "creating collections via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn update(&mut self, _: &str, _: &str, _: &Item) -> Result<Etag> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "updating items via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn set_meta(&mut self, _: MetadataKind, _: &str) -> Result<()> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "setting metadata via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn get_meta(&self, _: MetadataKind) -> Result<String> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "getting metadata via webcal is not supported",
        ))
    }

    fn id(&self) -> &str {
        self.definition.collection_name.as_str()
    }

    fn href(&self) -> &str {
        ""
    }
}

/// Helper method to fetch a URL and return its body as a String.
///
/// Be warned! This swallows headers (including `Etag`!).
#[inline]
async fn fetch_raw(client: &reqwest::Client, url: &Url) -> Result<String> {
    let response = client
        // TODO: upstream should impl IntoURL for &Url
        .get((*url).clone())
        .send()
        .await
        .map_err(|e| Error::new(ErrorKind::Other, e))?;

    if response.status() != StatusCode::OK {
        return Err(Error::new(ErrorKind::Other, "request did not return 200"));
    }

    let raw = response
        .text()
        .await
        .map_err(|e| Error::new(ErrorKind::Other, e))?;

    Ok(raw)
}

mod test {

    // FIXME: only run this test with a dedicated flag for networked test.
    // FIXME: use a webcal link hosted by me.
    // TODO: these are just validation tests and not suitable as a keeper.
    #[tokio::test]
    async fn test_dummy() {
        use super::WebCalStorage;
        use crate::base::Collection;
        use crate::base::Storage;
        use crate::webcal::WebCalDefinition;
        use url::Url;

        let metdata = WebCalDefinition {
            url: Url::parse("https://www.officeholidays.com/ics/netherlands").unwrap(),
            collection_name: "holidays".to_string(),
        };
        let storage = WebCalStorage::new(metdata).unwrap();
        storage.check().await.unwrap();
        let collection = &storage.open_collection("holidays").unwrap();
        let discovery = &storage.discover_collections().await.unwrap();

        assert_eq!(&collection, &discovery.first().unwrap());

        let item_refs = collection.list().await.unwrap();

        for item_ref in &item_refs {
            let (_item, etag) = collection.get(&item_ref.href).await.unwrap();
            // Might file if upstream file mutates between requests.
            assert_eq!(etag, item_ref.etag);
        }

        let hrefs: Vec<&str> = item_refs.iter().map(|r| r.href.as_ref()).collect();
        let many = collection.get_many(&hrefs.to_owned()).await.unwrap();

        assert_eq!(many.len(), hrefs.len());
        assert_eq!(many.len(), item_refs.len());
        // TODO: compare their contents and etags, though these should all match.
    }
}
