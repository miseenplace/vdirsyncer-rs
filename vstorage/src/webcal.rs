use reqwest::StatusCode;
use std::{
    io::{Error, ErrorKind, Result},
    rc::Rc,
};
use url::Url;

use crate::base::{Collection, Etag, Item, ItemRef, MetadataKind, Storage};

pub struct WebCalStorage {
    url: Rc<Url>,
    collection_name: String,
    client: reqwest::Client,
}

impl Storage for WebCalStorage {
    type Metadata = ();

    type Collection = WebCalCollection;

    /// Create a new storage instance.
    ///
    ///
    /// Unlike other [`Storage`] implementations, this one allows only a single collection. The id
    /// of the collection MUST be provided as a URL fragment (e.g.: `#my-collection`).
    ///
    /// The URL's scheme MUST be one of `http` or `https`.
    fn new(url: &Url, _: Self::Metadata, read_only: bool) -> Result<Self> {
        if !read_only {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "webcal only support read-only storages",
            ));
        }
        let fragment = url.fragment().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                "missing fragment with collection id",
            )
        })?;
        // NOTE: It would be nice to support `webcal://` here, but the Url crate won't allow
        // changing the scheme of such a Url.
        if ["http", "https"].contains(&url.scheme()) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "URL scheme must be http or https",
            ));
        };
        Ok(WebCalStorage {
            url: Rc::new(url.clone()),
            collection_name: fragment.to_owned(),
            client: reqwest::Client::new(),
        })
    }

    async fn check(&self) -> Result<()> {
        // TODO: Should map status codes to io::Error. if 404 -> NotFound, etc.
        let response = self
            .client
            .get((*self.url).clone())
            .send()
            .await
            .map_err(|e| Error::new(ErrorKind::Other, e))?;

        if response.status() != StatusCode::OK {
            return Err(Error::new(ErrorKind::Other, "request did not return 200"));
        }

        if !response
            .text()
            .await
            .map_err(|e| Error::new(ErrorKind::Other, e))?
            .starts_with("BEGIN:VCALENDAR")
        {
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
        // TODO: the fragment is not actually relevant here.
        if !href.is_empty() {
            return Err(Error::new(
                ErrorKind::NotFound,
                "only the '' collection is available via webical",
            ));
        }
        Ok(WebCalCollection {
            id: self.collection_name.to_owned(),
            url: self.url.clone(),
        })
    }
}

pub struct WebCalCollection {
    id: String,
    url: Rc<Url>,
}

impl Collection for WebCalCollection {
    async fn list(&self) -> Result<Vec<ItemRef>> {
        // TODO: need to parse icalendar data.
        todo!()
    }

    async fn get(&self, href: &str) -> Result<(Item, Etag)> {
        // TODO: need to parse icalendar data.
        todo!()
    }

    async fn get_many(&self, hrefs: &[&str]) -> Result<Vec<(Item, Etag)>> {
        // TODO: need to parse icalendar data.
        todo!()
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
