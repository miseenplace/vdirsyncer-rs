//! This library contains caldav and carddav clients.
//!
//! See [`CalDavClient`] as a useful entry point.
use std::{
    io,
    ops::{Deref, DerefMut},
};

use crate::auth::{Auth, AuthError};
use dav::DavClient;
use dav::DavError;
use dns::{find_context_path_via_txt_records, resolve_srv_record, TxtError};
use domain::base::Dname;
use http::Method;
use hyper::{Body, Uri};
use xml::{ItemDetails, ResponseWithProp, SimplePropertyMeta, StringProperty, DAV};

pub mod auth;
pub mod dav;
pub mod dns;
pub mod xml;

/// A client to communicate with a caldav server.
///
/// Wraps around a [`DavClient`], which provides the underlying webdav functionality.
// TODO FIXME: Need to figure out how to reuse as much as possible for carddav and caldav.
#[derive(Debug)]
pub struct CalDavClient {
    /// The `base_url` may be (due to bootstrapping discovery) different to the one provided as input.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc6764#section-1>
    dav_client: DavClient,
    /// URL of collections that are either calendar collections or ordinary collections
    /// that have child or descendant calendar collections owned by the principal.
    /// See: <https://www.rfc-editor.org/rfc/rfc4791#section-6.2.1>
    pub calendar_home_set: Option<Uri>, // TODO: timeouts
}

impl Deref for CalDavClient {
    type Target = DavClient;

    fn deref(&self) -> &Self::Target {
        &self.dav_client
    }
}
impl DerefMut for CalDavClient {
    fn deref_mut(&mut self) -> &mut dav::DavClient {
        &mut self.dav_client
    }
}

/// An error automatically bootstrapping a new client.
#[derive(thiserror::Error, Debug)]
pub enum BootstrapError {
    #[error("the input URL is not valid")]
    InvalidUrl,

    // FIXME: See: https://github.com/NLnetLabs/domain/pull/183
    #[error("error resolving DNS SRV records")]
    DnsError,

    #[error("SRV records returned domain/port pair that failed to parse")]
    BadSrv(http::Error),

    #[error("error resolving context path via TXT records")]
    TxtError(#[from] TxtError),

    #[error(transparent)]
    DavError(#[from] DavError),
}

impl From<BootstrapError> for io::Error {
    fn from(value: BootstrapError) -> Self {
        match value {
            BootstrapError::InvalidUrl => io::Error::new(io::ErrorKind::InvalidInput, value),
            BootstrapError::DnsError | BootstrapError::TxtError(_) => {
                io::Error::new(io::ErrorKind::Other, value)
            }
            BootstrapError::BadSrv(_) => io::Error::new(io::ErrorKind::InvalidData, value),
            BootstrapError::DavError(dav) => io::Error::from(dav),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ResolveContextPathError {
    #[error("failed to create uri and request with given parameters")]
    BadInput(#[from] http::Error),

    #[error("network error handling http stream")]
    Network(#[from] hyper::Error),

    #[error("missing Location header in response")]
    MissingLocation,

    #[error("error building new Uri with Location from response")]
    BadLocation(#[from] http::uri::InvalidUri),

    #[error("internal error with specified authentication")]
    Auth(#[from] AuthError),
}

// TODO: Minimal input from a user would consist of a calendar user address and a password.  A
// calendar user address is defined by iCalendar [RFC5545] to be a URI [RFC3986].
// https://www.rfc-editor.org/rfc/rfc6764#section-6
impl CalDavClient {
    /// Returns a client without any automatic bootstrapping.
    ///
    /// It is generally advised to use [`CalDavClient::auto_bootstrap`] instead.
    pub fn raw_client(base_url: Uri, auth: Auth) -> Self {
        // TODO: check that the URL is http or https (or mailto:?).

        Self {
            dav_client: DavClient::new(base_url, auth),
            calendar_home_set: None,
        }
    }

    // TODO: methods to serialise and deserialise (mostly to cache all discovery data).

    /// Auto-bootstrap a new client.
    ///
    /// Determines the caldav server's real host and the context path of the resources for a
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

        let domain = client.base_url.host().ok_or(BootstrapError::InvalidUrl)?;
        let port = client.default_port();

        let dname = Dname::bytes_from_str(domain).map_err(|_| BootstrapError::InvalidUrl)?;
        let candidates = {
            let mut candidates = resolve_srv_record(dname, port)
                .await
                .map_err(|_| BootstrapError::DnsError)?;

            // If there are no SRV records, try the domain/port in the provided URI.
            if candidates.is_empty() {
                candidates.push((domain.to_string(), port));
            }
            candidates
        };

        // FIXME: this all always assumes "https". We don't yet query plain-text SRV records, so
        //        that's kinda fine, but this might break for exotic setups.
        if let Some(path) = find_context_path_via_txt_records(domain).await? {
            // TODO: validate that the path works on the chosen server.
            let candidate = &candidates[0];

            client.base_url = Uri::builder()
                .scheme("https")
                .authority(format!("{}:{}", candidate.0, candidate.1))
                .path_and_query(path)
                .build()
                .map_err(BootstrapError::BadSrv)?;
        } else {
            for candidate in candidates {
                if let Ok(Some(url)) = client
                    .resolve_context_path("https", &candidate.0, candidate.1)
                    .await
                {
                    client.base_url = url;
                    break;
                }
            }
        }

        // From https://www.rfc-editor.org/rfc/rfc6764#section-6, subsection 5:
        // > clients MUST properly handle HTTP redirect responses for the request
        client.principal = client.resolve_current_user_principal().await?;

        // If obtaining a principal fails, the specification says we should query the user. This
        // tries to use the `base_url` first, since the user might have provided it for a reason.
        let principal_url = client
            .principal
            .as_ref()
            .unwrap_or(&client.base_url)
            .clone();
        client.calendar_home_set = client.query_calendar_home_set(principal_url).await?;

        Ok(client)
    }

    /// Resolve the default context path using the well-known path.
    ///
    /// # Errors
    ///
    /// - If the provided scheme, host and port cannot be used to construct a valid URL.
    /// - If there are any network errors.
    /// - If the response is not an HTTP redirection.
    /// - If the `Location` header in the response is missing or invalid.
    ///
    /// # See also
    ///
    /// - <https://www.rfc-editor.org/rfc/rfc6764#section-5>
    /// - [`ResolveContextPathError`]
    pub async fn resolve_context_path(
        &self,
        scheme: &str,
        host: &str,
        port: u16,
    ) -> Result<Option<Uri>, ResolveContextPathError> {
        let url = Uri::builder()
            .scheme(scheme)
            .authority(format!("{host}:{port}"))
            .path_and_query("/.well-known/caldav")
            .build()?;

        let request = self
            .request()?
            .method(Method::GET)
            .uri(url)
            .body(Body::default())?;

        // From https://www.rfc-editor.org/rfc/rfc6764#section-5:
        // > [...] the server MAY require authentication when a client tries to
        // > access the ".well-known" URI
        let (head, _body) = self.http_client.request(request).await?.into_parts();

        if !head.status.is_redirection() {
            return Ok(None);
        }

        // TODO: multiple redirections...?
        let location = head
            .headers
            .get(hyper::header::LOCATION)
            .ok_or(ResolveContextPathError::MissingLocation)?;
        // TODO: properly handle RELATIVE urls.
        Ok(Some(Uri::try_from(location.as_bytes())?))
    }

    async fn query_calendar_home_set(&self, url: Uri) -> Result<Option<Uri>, DavError> {
        let property_data = SimplePropertyMeta {
            name: b"calendar-home-set".to_vec(),
            namespace: xml::CALDAV.to_vec(),
        };

        self.find_href_prop_as_uri(
            url,
            "<calendar-home-set xmlns=\"urn:ietf:params:xml:ns:caldav\"/>",
            property_data,
        )
        .await
    }

    /// Find calendars collections under the given `url`.
    ///
    /// Returns absolute paths to each calendar. This method should be called
    /// with the `calendar_home_set` URL to find the current user's calendars.
    ///
    /// # Errors
    ///
    /// If the HTTP call fails or parsing the XML response fails.
    pub async fn find_calendars(&self, url: Uri) -> Result<Vec<String>, DavError> {
        Ok(self
            // XXX: depth 1 or infinity?
            .propfind::<ResponseWithProp<ItemDetails>>(
                url.clone(),
                "<resourcetype/><getetag/>",
                1,
                (),
            )
            .await
            .map_err(DavError::from)?
            .into_iter()
            .filter(|c| {
                if let Ok(cal) = c {
                    cal.prop.is_calendar
                } else {
                    true
                }
            })
            // FIXME: silently ignores collections with any issues:
            .filter_map(|r| r.map(|c| c.href).ok())
            .collect())
    }

    /// Returns the `displayname` for the calendar at path `href`.
    ///
    /// # Errors
    ///
    /// If the HTTP call fails or parsing the XML response fails.
    pub async fn get_calendar_displayname(&self, href: &str) -> Result<Option<String>, DavError> {
        let url = self.relative_uri(href)?;

        let property_data = SimplePropertyMeta {
            name: b"displayname".to_vec(),
            namespace: DAV.to_vec(),
        };

        self.propfind::<ResponseWithProp<StringProperty>>(
            url.clone(),
            "<displayname/>",
            0,
            property_data,
        )
        .await?
        .pop()
        .ok_or(xml::Error::MissingData("dispayname"))?
        .map(Option::<String>::from)
        .map_err(DavError::from)
    }

    /// Returns the colour for the calendar at path `href`.
    ///
    /// This is not a formally standardised property, but is relatively widespread.
    ///
    /// # Errors
    ///
    /// If the network request fails, or if the response cannot be parsed.
    pub async fn get_calendar_colour(&self, href: &str) -> Result<Option<String>, DavError> {
        let url = self.relative_uri(href)?;

        let property_data = SimplePropertyMeta {
            name: b"calendar-color".to_vec(),
            // XXX: prop_namespace: b"http://apple.com/ns/ical/".to_vec(),
            namespace: b"DAV:".to_vec(),
        };

        self.propfind::<ResponseWithProp<StringProperty>>(
            url.clone(),
            "<calendar-color xmlns=\"http://apple.com/ns/ical/\"/>",
            0,
            property_data,
        )
        .await?
        .pop()
        .ok_or(xml::Error::MissingData("calendar-color"))?
        .map(Option::<String>::from)
        .map_err(DavError::from)
    }

    // TODO: get_calendar_description ("calendar-description", "urn:ietf:params:xml:ns:caldav")
    // TODO: get_calendar_order ("calendar-order", "http://apple.com/ns/ical/")
    // TODO: DRY: the above methods are super repetitive.
    //       Maybe all these props impl a single trait, so the API could be `get_prop<T>(url)`?

    /// Enumerates entries in a collection
    ///
    /// Returns an array of results. Because the server can return a non-ok status for individual
    /// entries, some of them may be `Err`, while other are `Ok(ItemDetails)`.
    ///
    /// # Errors
    ///
    /// If there are network errors executing the request or parsing the XML response.
    pub async fn list_collection(
        &self,
        collection_href: &str,
    ) -> Result<Vec<Result<ResponseWithProp<ItemDetails>, crate::xml::Error>>, DavError> {
        let url = self.relative_uri(collection_href)?;

        self.propfind::<ResponseWithProp<ItemDetails>>(
            url,
            "<resourcetype/><getcontenttype/><getetag/>",
            1,
            (),
        )
        .await
        // TODO: map to a cleaner public type
    }
}
