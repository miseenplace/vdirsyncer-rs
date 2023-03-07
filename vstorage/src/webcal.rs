//! Implements reading entries from a remote webcal resource.
//!
//! Webcal is a de-facto standard, and is basically a single icalendar file hosted via http(s).
//!
//! See the [Webcal wikipedia page](https://en.wikipedia.org/wiki/Webcal).
#![allow(clippy::module_name_repetitions)]

use async_trait::async_trait;
use http::{uri::Scheme, StatusCode, Uri};
use hyper::{client::HttpConnector, Client};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use std::{
    io::{Error, ErrorKind, Result},
    sync::Arc,
};

use crate::{
    base::{Collection, Definition, Etag, Href, Item, ItemRef, MetadataKind, Storage},
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
    inner: Arc<WebCalInner>,
}

/// Definition for a [`WebCalStorage`].
#[derive(Debug, PartialEq)]
pub struct WebCalDefinition {
    /// The URL of the remote icalendar resource. Must be HTTP or HTTPS.
    pub url: Uri,
    /// The name to be given to the single collection available.
    pub collection_name: String,
}

/// A holder of data shared across the storage and its collections.
struct WebCalInner {
    definition: Arc<WebCalDefinition>,
    http_client: Client<HttpsConnector<HttpConnector>>,
}

#[async_trait]
impl Definition for WebCalDefinition {
    /// Create a new storage instance.
    ///
    /// Unlike other [`Storage`] implementations, this one allows only a single collection.
    async fn storage(self) -> Result<Box<dyn Storage>> {
        match &self.url.scheme().map(Scheme::as_str) {
            Some("http") => {}
            Some("https") => {}
            // TODO: support webcal and webcals
            Some(_) => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "URL scheme must be http or https",
                ));
            }
            None => todo!(),
        }
        let https = HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_only()
            .enable_http1()
            .build();
        Ok(Box::from(WebCalStorage {
            inner: Arc::from(WebCalInner {
                definition: Arc::new(self),
                http_client: Client::builder().build(https),
            }),
        }))
    }
}

#[async_trait]
impl Storage for WebCalStorage {
    /// Checks that the remove resource exists and whether it looks like an icalendar resource.
    async fn check(&self) -> Result<()> {
        // TODO: Should map status codes to io::Error. if 404 -> NotFound, etc.
        let raw = fetch_raw(&self.inner.http_client, &self.inner.definition.url).await?;

        if !raw.starts_with("BEGIN:VCALENDAR") {
            return Err(Error::new(
                ErrorKind::Other,
                "response for URL doesn't look like a calendar",
            ));
        }
        Ok(())
    }

    /// Returns a single collection with the name specified in the definition.
    async fn discover_collections(&self) -> Result<Vec<Box<dyn Collection>>> {
        Ok(vec![Box::from(WebCalCollection {
            inner: self.inner.clone(),
        })])
    }

    /// Unsupported for this storage type.
    async fn create_collection(&mut self, _: &str) -> Result<Box<dyn Collection>> {
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
    fn open_collection(&self, href: &str) -> Result<Box<dyn Collection>> {
        if href != self.inner.definition.collection_name {
            return Err(Error::new(
                ErrorKind::NotFound,
                format!("this storage only contains the '{href}' collection"),
            ));
        }
        Ok(Box::from(WebCalCollection {
            inner: self.inner.clone(),
        }))
    }
}

/// A collection of items in a webcal storage.
///
/// For this collection type, the `Href` is the UID of the entries. There is no other way to
/// address individual entries, so this is essentially the only choice.
///
/// The fact that `Href = UID` is a quirk specific to this storage type, and should not be relied
/// upon in general.
pub struct WebCalCollection {
    inner: Arc<WebCalInner>,
}

impl PartialEq for &WebCalCollection {
    fn eq(&self, other: &Self) -> bool {
        self.inner.definition.eq(&other.inner.definition)
    }
}

#[async_trait]
impl Collection for WebCalCollection {
    /// Enumerates items in this collection.
    ///
    /// Note that, due to the nature of webcal, the whole collection needs to be retrieved. If some
    /// items need to be read as well, it is generally best to use [`WebCalCollection::get_all`] instead.
    async fn list(&self) -> Result<Vec<ItemRef>> {
        let raw = fetch_raw(&self.inner.http_client, &self.inner.definition.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let calendar = Component::parse(&raw);
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
        let raw = fetch_raw(&self.inner.http_client, &self.inner.definition.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let calendar = Component::parse(&raw);
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
    async fn get_many(&self, hrefs: &[&str]) -> Result<Vec<(Href, Item, Etag)>> {
        let raw = fetch_raw(&self.inner.http_client, &self.inner.definition.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let calendar = Component::parse(&raw);
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
                    Some(Ok((item.ident(), item, hash)))
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
        let raw = fetch_raw(&self.inner.http_client, &self.inner.definition.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let calendar = Component::parse(&raw);
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
    async fn get_meta(&self, _: MetadataKind) -> Result<Option<String>> {
        // TODO: return None?
        Err(Error::new(
            ErrorKind::Unsupported,
            "getting metadata via webcal is not supported",
        ))
    }

    fn id(&self) -> &str {
        self.inner.definition.collection_name.as_str()
    }

    fn href(&self) -> &str {
        ""
    }
}

/// Helper method to fetch a URL and return its body as a String.
///
/// Be warned! This swallows headers (including `Etag`!).
#[inline]
async fn fetch_raw(client: &Client<HttpsConnector<HttpConnector>>, url: &Uri) -> Result<String> {
    let response = client
        // TODO: upstream should impl IntoURL for &Uri
        .get(url.clone())
        .await
        .map_err(|e| Error::new(ErrorKind::Other, e))?;

    if response.status() != StatusCode::OK {
        return Err(Error::new(ErrorKind::Other, "request did not return 200"));
    }

    // TODO: handle non-UTF-8 data.
    hyper::body::to_bytes(response)
        .await
        .map_err(|e| Error::new(ErrorKind::Other, e))
        .map(|bytes| String::from_utf8(bytes.into()))?
        .map_err(|e| Error::new(ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod test {
    use http::Uri;

    use crate::base::Definition;

    // FIXME: only run this test with a dedicated flag for networked test.
    // FIXME: use a webcal link hosted by me.
    // TODO: these are just validation tests and not suitable as a keeper.
    #[tokio::test]
    async fn test_dummy() {
        use crate::webcal::WebCalDefinition;

        let metdata = WebCalDefinition {
            url: Uri::try_from("https://www.officeholidays.com/ics/netherlands").unwrap(),
            collection_name: "holidays".to_string(),
        };
        let storage = metdata.storage().await.unwrap();
        storage.check().await.unwrap();
        let collection = &storage.open_collection("holidays").unwrap();
        let discovery = &storage.discover_collections().await.unwrap();

        assert_eq!(&collection.id(), &discovery.first().unwrap().id());

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
