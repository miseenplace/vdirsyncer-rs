use std::ops::{Deref, DerefMut};

use hyper::Uri;

use crate::builder::{ClientBuilder, NeedsUri};
use crate::common_bootstrap;
use crate::dav::{DavError, FoundCollection};
use crate::dns::DiscoverableService;
use crate::xml::{ItemDetails, ResponseVariant, SimplePropertyMeta};
use crate::{dav::WebDavClient, BootstrapError, FindHomeSetError};

/// A client to communicate with a carddav server.
///
/// Instances are created via a builder:
///
/// ```rust,no_run
/// # use libdav::CardDavClient;
/// use http::Uri;
/// use libdav::auth::Auth;
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let uri = Uri::try_from("https://example.com").unwrap();
/// let auth = Auth::Basic {
///     username: String::from("user"),
///     password: Some(String::from("secret")),
/// };
///
/// let client = CardDavClient::builder()
///     .with_uri(uri)
///     .with_auth(auth)
///     .build()
///     .auto_bootstrap()
///     .await
///     .unwrap();
/// # })
/// ```
///
/// For common cases, [`auto_bootstrap`](Self::auto_bootstrap) should be called on the client to
/// bootstrap it automatically.
#[derive(Debug)]
pub struct CardDavClient {
    /// The `base_url` may be (due to bootstrapping discovery) different to the one provided as input.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc6764#section-1>
    dav_client: WebDavClient,
    /// URL of collections that are either address book collections or ordinary collections
    /// that have child or descendant address book collections owned by the principal.
    /// See: <https://www.rfc-editor.org/rfc/rfc6352#section-7.1.1>
    ///
    /// This field is automatically populated by [`auto_bootstrap`][Self::auto_bootstrap].
    pub addressbook_home_set: Option<Uri>, // TODO: timeouts
}

impl Deref for CardDavClient {
    type Target = WebDavClient;

    fn deref(&self) -> &Self::Target {
        &self.dav_client
    }
}
impl DerefMut for CardDavClient {
    fn deref_mut(&mut self) -> &mut crate::dav::WebDavClient {
        &mut self.dav_client
    }
}

impl ClientBuilder<CardDavClient, crate::builder::Ready> {
    /// Return a built client.
    pub fn build(self) -> CardDavClient {
        CardDavClient {
            dav_client: WebDavClient::new(self.state.uri, self.state.auth),
            addressbook_home_set: None,
        }
    }
}

impl CardDavClient {
    /// Creates a new builder. See [`CardDavClient`] and [`ClientBuilder`] for details.
    #[must_use]
    pub fn builder() -> ClientBuilder<Self, NeedsUri> {
        ClientBuilder::new()
    }

    // TODO: methods to serialise and deserialise (mostly to cache all discovery data).

    /// Auto-bootstrap a new client.
    ///
    /// Determines the carddav server's real host and the context path of the resources for a
    /// server, following the discovery mechanism described in [rfc6764].
    ///
    /// [rfc6764]: https://www.rfc-editor.org/rfc/rfc6764
    ///
    /// # Errors
    ///
    /// If any of the underlying DNS or HTTP requests fail, or if any of the responses fail to
    /// parse.
    ///
    /// Does not return an error if DNS records as missing, only if they contain invalid data.
    pub async fn auto_bootstrap(mut self) -> Result<Self, BootstrapError> {
        let port = self.default_port()?;
        let service = self.service()?;
        common_bootstrap(&mut self, port, service).await?;

        // If obtaining a principal fails, the specification says we should query the user. This
        // tries to use the `base_url` first, since the user might have provided it for a reason.
        let principal_url = self.principal.as_ref().unwrap_or(&self.base_url);
        self.addressbook_home_set = self.find_addressbook_home_set(principal_url).await?;

        Ok(self)
    }

    async fn find_addressbook_home_set(&self, url: &Uri) -> Result<Option<Uri>, FindHomeSetError> {
        let property_data = SimplePropertyMeta {
            name: b"addressbook-home-set".to_vec(),
            namespace: crate::xml::CARDDAV.to_vec(),
        };

        self.find_href_prop_as_uri(
            url,
            "<addressbook-home-set xmlns=\"urn:ietf:params:xml:ns:carddav\"/>",
            &property_data,
        )
        .await
        .map_err(FindHomeSetError)
    }

    /// Find address book collections under the given `url`.
    ///
    /// It `url` is not specified, this client's address book home set is used instead. If no
    /// address book home set has been found, then the server's context path will be used. When
    /// using a client bootstrapped via automatic discovery, passing `None` will usually yield the
    /// expected results.
    ///
    /// # Errors
    ///
    /// If the HTTP call fails or parsing the XML response fails.
    pub async fn find_addresbooks(
        &self,
        url: Option<&Uri>,
    ) -> Result<Vec<FoundCollection>, DavError> {
        let url = url.unwrap_or(self.addressbook_home_set.as_ref().unwrap_or(&self.base_url));
        // FIXME: DRY: This is almost a copy-paste of the same method from CalDavClient
        let items = self
            // XXX: depth 1 or infinity?
            .propfind::<ItemDetails>(url, "<resourcetype/><getetag/>", 1, &())
            .await
            .map_err(DavError::from)?
            .into_iter()
            .filter_map(|c| match c.variant {
                ResponseVariant::WithProps { propstats } => {
                    if propstats.iter().any(|p| p.prop.is_address_book) {
                        Some(FoundCollection {
                            href: c.href,
                            etag: propstats.into_iter().find_map(|p| p.prop.etag),
                        })
                    } else {
                        None
                    }
                }
                ResponseVariant::WithoutProps { .. } => None,
            })
            .collect();

        Ok(items)
    }

    // TODO: get_addressbook_description ("addressbook-description", "urn:ietf:params:xml:ns:carddav")
    // TODO: DRY: the above methods are super repetitive.
    //       Maybe all these props impl a single trait, so the API could be `get_prop<T>(url)`?

    /// Returns the default port to try and use.
    ///
    /// If the `base_url` has an explicit port, that value is returned. Otherwise,
    /// returns `443` for https, `80` for http, and `443` as a fallback for
    /// anything else.
    fn default_port(&self) -> Result<u16, BootstrapError> {
        // raise InvaidUrl?
        if let Some(port) = self.base_url.port_u16() {
            Ok(port)
        } else {
            match self.base_url.scheme() {
                Some(scheme) if scheme == "https" => Ok(443),
                Some(scheme) if scheme == "http" => Ok(80),
                Some(scheme) if scheme == "carddavs" => Ok(443),
                Some(scheme) if scheme == "carddav" => Ok(80),
                _ => Err(BootstrapError::InvalidUrl("invalid scheme (and no port)")),
            }
        }
    }

    fn service(&self) -> Result<DiscoverableService, BootstrapError> {
        let scheme = self
            .base_url
            .scheme()
            .ok_or(BootstrapError::InvalidUrl("missing scheme"))?;
        match scheme.as_ref() {
            "https" | "caldavs" => Ok(DiscoverableService::CalDavs),
            "http" | "caldav" => Ok(DiscoverableService::CalDav),
            _ => Err(BootstrapError::InvalidUrl("scheme is invalid")),
        }
    }
}
