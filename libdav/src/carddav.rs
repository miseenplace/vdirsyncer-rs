// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::ops::Deref;

use http::Method;
use hyper::{Body, Uri};
use log::debug;
use roxmltree::ExpandedName;

use crate::builder::{ClientBuilder, NeedsUri};
use crate::common::common_bootstrap;
use crate::dav::{check_status, DavError, FoundCollection};
use crate::dns::DiscoverableService;
use crate::names::{
    ADDRESSBOOK, ADDRESSBOOK_HOME_SET, ADDRESS_DATA, GETETAG, HREF, RESOURCETYPE,
    SUPPORTED_REPORT_SET, SYNC_COLLECTION,
};
use crate::{dav::WebDavClient, BootstrapError, FindHomeSetError};
use crate::{CheckSupportError, FetchedResource};

/// A client to communicate with a carddav server.
///
/// Instances are created via a builder:
///
/// ```rust,no_run
/// # use libdav::CardDavClient;
/// use http::Uri;
/// use libdav::auth::{Auth, Password};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let uri = Uri::try_from("https://example.com").unwrap();
/// let auth = Auth::Basic {
///     username: String::from("user"),
///     password: Some(Password::from("secret")),
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
        common_bootstrap(&mut self.dav_client, port, service).await?;

        // If obtaining a principal fails, the specification says we should query the user. This
        // tries to use the `base_url` first, since the user might have provided it for a reason.
        let principal_url = self.principal.as_ref().unwrap_or(&self.base_url);
        self.addressbook_home_set = self.find_addressbook_home_set(principal_url).await?;

        Ok(self)
    }

    async fn find_addressbook_home_set(&self, url: &Uri) -> Result<Option<Uri>, FindHomeSetError> {
        self.find_href_prop_as_uri(url, &ADDRESSBOOK_HOME_SET)
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
        let (head, body) = self
            .propfind(url, &[&RESOURCETYPE, &GETETAG, &SUPPORTED_REPORT_SET], 1)
            .await?;
        check_status(head.status)?;

        let body = std::str::from_utf8(body.as_ref())?;
        let doc = roxmltree::Document::parse(body)?;
        let root = doc.root_element();

        let response_name = ExpandedName::from(("DAV:", "response"));
        let responses = root
            .descendants()
            .filter(|node| node.tag_name() == response_name);

        let mut items = Vec::new();
        for response in responses {
            let is_addressbook = response
                .descendants()
                .find(|node| node.tag_name() == RESOURCETYPE)
                .map_or(false, |node| {
                    node.descendants()
                        .any(|node| node.tag_name() == ADDRESSBOOK)
                });
            if !is_addressbook {
                continue;
            }

            let href = response
                .descendants()
                .find(|node| node.tag_name() == HREF)
                .ok_or(DavError::InvalidResponse("missing href in response".into()))?
                .text()
                .ok_or(DavError::InvalidResponse("missing text in href".into()))?
                .to_string();
            let etag = response
                .descendants()
                .find(|node| node.tag_name() == GETETAG)
                .and_then(|node| node.text().map(str::to_string));
            let supports_sync = response
                .descendants()
                .find(|node| node.tag_name() == SUPPORTED_REPORT_SET)
                .map_or(false, |node| {
                    node.descendants()
                        .any(|node| node.tag_name() == SYNC_COLLECTION)
                });

            items.push(FoundCollection {
                href,
                etag,
                supports_sync,
            });
        }

        Ok(items)
    }

    // TODO: get_addressbook_description ("addressbook-description", "urn:ietf:params:xml:ns:carddav")
    // TODO: DRY: the above methods are super repetitive.
    //       Maybe all these props impl a single trait, so the API could be `get_prop<T>(url)`?

    /// Fetches existing vcard resources.
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    pub async fn get_resources<S1, S2>(
        &self,
        addressbook_href: S1,
        hrefs: &[S2],
    ) -> Result<Vec<FetchedResource>, DavError>
    where
        S1: AsRef<str>,
        S2: AsRef<str>,
    {
        let mut body = String::from(
            r#"
            <C:addressbook-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav">
                <D:prop>
                    <D:getetag/>
                    <C:address-data/>
                </D:prop>"#,
        );
        for href in hrefs {
            body.push_str(&format!("<href>{}</href>", href.as_ref()));
        }
        body.push_str("</C:addressbook-multiget>");

        self.multi_get(addressbook_href.as_ref(), body, &ADDRESS_DATA)
            .await
    }

    /// Checks that the given URI advertises carddav support.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc6352#section-6.1>
    ///
    /// # Errors
    ///
    /// If there are any network issues or if the server does not explicitly advertise carddav
    /// support.
    pub async fn check_support(&self, url: &Uri) -> Result<(), CheckSupportError> {
        let request = self
            .request_builder()?
            .method(Method::OPTIONS)
            .uri(url)
            .body(Body::empty())?;

        let (head, _body) = self.request(request).await?;
        check_status(head.status)?;

        let header = head
            .headers
            .get("DAV")
            .ok_or(CheckSupportError::MissingHeader)?
            .to_str()?;

        debug!("DAV header: '{}'", header);
        if header
            .split(|c| c == ',')
            .any(|part| part.trim() == "addressbook")
        {
            Ok(())
        } else {
            Err(CheckSupportError::NotAdvertised)
        }
    }

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
            "https" | "carddavs" => Ok(DiscoverableService::CardDavs),
            "http" | "carddav" => Ok(DiscoverableService::CardDav),
            _ => Err(BootstrapError::InvalidUrl("scheme is invalid")),
        }
    }
}
