use reqwest::StatusCode;
use std::{
    io::{Error, ErrorKind, Result},
    sync::Arc,
};
use url::Url;

use crate::{
    base::{Collection, Etag, Item, ItemRef, MetadataKind, Storage},
    simple_component::Component,
};

pub struct WebCalStorage {
    url: Arc<Url>,
    collection_name: String,
    client: reqwest::Client,
}

pub struct WebCalDefinition {
    url: Url,
    collection_name: String,
}

impl Storage for WebCalStorage {
    type Definition = WebCalDefinition;

    type Collection = WebCalCollection;

    /// Create a new storage instance.
    ///
    ///
    /// Unlike other [`Storage`] implementations, this one allows only a single collection.
    ///
    /// The URL's scheme MUST be one of `http` or `https`.
    fn new(definition: Self::Definition, read_only: bool) -> Result<Self> {
        if !read_only {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "webcal only support read-only storages",
            ));
        }
        // NOTE: It would be nice to support `webcal://` here, but the Url crate won't allow
        // changing the scheme of such a Url.
        if !["http", "https"].contains(&definition.url.scheme()) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "URL scheme must be http or https",
            ));
        };
        Ok(WebCalStorage {
            url: Arc::new(definition.url),
            collection_name: definition.collection_name,
            client: reqwest::Client::new(),
        })
    }

    async fn check(&self) -> Result<()> {
        // TODO: Should map status codes to io::Error. if 404 -> NotFound, etc.
        let raw = fetch_raw(&self.client, &self.url).await?;

        if !raw.starts_with("BEGIN:VCALENDAR") {
            return Err(Error::new(
                ErrorKind::Other,
                "response for URL doesn't look like a calendar",
            ));
        }
        Ok(())
    }

    async fn discover_collections(&self) -> Result<Vec<Self::Collection>> {
        Ok(vec![WebCalCollection {
            id: self.collection_name.to_owned(),
            url: self.url.clone(),
            client: self.client.clone(),
        }])
    }

    async fn create_collection(&mut self, _: &str) -> Result<Self::Collection> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "creating collections via webdav is not supported",
        ))
    }

    async fn destroy_collection(&mut self, _: &str) -> Result<()> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "creating collections via webdav is not supported",
        ))
    }

    fn open_collection(&self, href: &str) -> Result<Self::Collection> {
        if href != self.collection_name {
            return Err(Error::new(
                ErrorKind::NotFound,
                format!("this storage only contains the '{}' collection", href),
            ));
        }
        Ok(WebCalCollection {
            id: self.collection_name.to_owned(),
            url: self.url.clone(),
            client: self.client.clone(),
        })
    }
}

#[derive(Debug)]
pub struct WebCalCollection {
    id: String,
    url: Arc<Url>,
    client: reqwest::Client,
}

impl PartialEq for &WebCalCollection {
    fn eq(&self, other: &Self) -> bool {
        (&self.id, &self.url).eq(&(&other.id, &other.url))
    }
}

impl Collection for WebCalCollection {
    async fn list(&self) -> Result<Vec<ItemRef>> {
        let raw = fetch_raw(&self.client, &self.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let calendar = Component::parse(raw);
        let refs = calendar
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .subcomponents
            .iter()
            .map(|c| ItemRef {
                href: c.uid(),
                etag: crate::util::hash(c.raw()),
            })
            .collect();

        Ok(refs)
    }

    /// Returns a single item from the collection.
    ///
    /// Note that, due to the nature of webcal, the whole collection needs to be retrieved. It is
    /// strongly recommended to use [`get_many`] instead.
    ///
    /// [`get_many`]: crate::base::Collection::get_many
    async fn get(&self, href: &str) -> Result<(Item, Etag)> {
        let raw = fetch_raw(&self.client, &self.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let calendar = Component::parse(raw);
        let components = calendar
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .subcomponents;
        let component = components
            .iter()
            .find(|c| c.uid() == href)
            .ok_or_else(|| Error::from(ErrorKind::NotFound))?;

        let raw = component.raw();
        let hash = crate::util::hash(&raw);

        Ok((Item { raw }, hash))
    }

    async fn get_many(&self, hrefs: &[&str]) -> Result<Vec<(Item, Etag)>> {
        let raw = fetch_raw(&self.client, &self.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let calendar = Component::parse(raw);
        let mut components = calendar
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .subcomponents;

        // TODO: we need to fail if an href is missing from upstream.
        // Although this can be done externally to this method? I'm not sure the API should
        // guarantee it, since upstream APIs don't seem to do so.
        // This needs to be though about. A lot.
        components.retain(|c| hrefs.contains(&c.uid().as_ref()));

        components
            .iter()
            .map(|c| {
                let raw = c.raw();
                let hash = crate::util::hash(&raw);

                Ok((Item { raw }, hash))
            })
            .collect()
    }

    async fn add(&mut self, _: &Item) -> Result<ItemRef> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "creating collections via webdav is not supported",
        ))
    }

    async fn update(&mut self, _: &str, _: &str, _: &Item) -> Result<Etag> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "updating items via webdav is not supported",
        ))
    }

    async fn set_meta(&mut self, _: MetadataKind, _: &str) -> Result<()> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "setting metadata via webdav is not supported",
        ))
    }

    async fn get_meta(&self, _: MetadataKind) -> Result<String> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "getting metadata via webdav is not supported",
        ))
    }

    fn id(&self) -> &str {
        return self.id.as_str();
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
        let storage = WebCalStorage::new(metdata, true).unwrap();
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
