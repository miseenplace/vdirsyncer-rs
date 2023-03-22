use std::ops::{Deref, DerefMut};

use hyper::Uri;

use crate::auth::Auth;
use crate::dav::DavError;
use crate::dns::DiscoverableService;
use crate::xml::{ItemDetails, ResponseVariant, ResponseWithProp, SimplePropertyMeta};
use crate::{dav::DavClient, BootstrapError, DavWithAutoDiscovery, FindHomeSetError};

/// A client to communicate with a carddav server.
///
/// Wraps around a [`DavClient`], which provides the underlying webdav functionality.
#[derive(Debug)]
pub struct CardDavClient {
    /// The `base_url` may be (due to bootstrapping discovery) different to the one provided as input.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc6764#section-1>
    dav_client: DavClient,
    /// URL of collections that are either address book collections or ordinary collections
    /// that have child or descendant address book collections owned by the principal.
    /// See: <https://www.rfc-editor.org/rfc/rfc6352#section-7.1.1>
    pub addressbook_home_set: Option<Uri>, // TODO: timeouts
}

impl Deref for CardDavClient {
    type Target = DavClient;

    fn deref(&self) -> &Self::Target {
        &self.dav_client
    }
}
impl DerefMut for CardDavClient {
    fn deref_mut(&mut self) -> &mut crate::dav::DavClient {
        &mut self.dav_client
    }
}

impl CardDavClient {
    /// Returns a client without any automatic bootstrapping.
    ///
    /// It is generally advised to use [`CardDavClient::auto_bootstrap`] instead.
    pub fn raw_client(base_url: Uri, auth: Auth) -> Self {
        // TODO: check that the URL is http or https (or mailto:?).

        Self {
            dav_client: DavClient::new(base_url, auth),
            addressbook_home_set: None,
        }
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
    pub async fn auto_bootstrap(base_url: Uri, auth: Auth) -> Result<Self, BootstrapError> {
        let mut client = Self::raw_client(base_url, auth);
        client = client.common_bootstrap().await?;

        // If obtaining a principal fails, the specification says we should query the user. This
        // tries to use the `base_url` first, since the user might have provided it for a reason.
        let principal_url = client
            .principal
            .as_ref()
            .unwrap_or(&client.base_url)
            .clone();
        client.addressbook_home_set = client.find_addressbook_home_set(principal_url).await?;

        Ok(client)
    }

    async fn find_addressbook_home_set(&self, url: Uri) -> Result<Option<Uri>, FindHomeSetError> {
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
    /// Returns absolute paths to each addressbook and their respective etag. This method should be
    /// called with the `addressbook_home_set` URL to find the current user's address books.
    ///
    /// # Errors
    ///
    /// If the HTTP call fails or parsing the XML response fails.
    pub async fn find_addresbooks(
        &self,
        url: Uri,
    ) -> Result<Vec<(String, Option<String>)>, DavError> {
        // FIXME: DRY: This is almost a copy-paste of the same method from CalDavClient
        let items = self
            // XXX: depth 1 or infinity?
            .propfind::<ResponseWithProp<ItemDetails>>(
                url.clone(),
                "<resourcetype/><getetag/>",
                1,
                &(),
            )
            .await
            .map_err(DavError::from)?
            .into_iter()
            .filter_map(|c| match c.variant {
                ResponseVariant::WithProps { propstats } => {
                    if propstats.iter().any(|p| p.prop.is_address_book) {
                        Some((c.href, propstats.into_iter().find_map(|p| p.prop.etag)))
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
}

impl DavWithAutoDiscovery for CardDavClient {
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

    fn set_principal(&mut self, principal: Option<Uri>) {
        self.principal = principal;
    }
}
