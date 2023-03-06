use std::io;

use base64::{prelude::BASE64_STANDARD, write::EncoderWriter};
use dns::{find_context_path_via_txt_records, resolve_srv_record, TxtError};
use domain::base::Dname;
use http::{HeaderValue, Method, Request, StatusCode};
use hyper::{client::HttpConnector, Body, Client, Uri};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use std::io::Write;
use xml::{
    FromXml, HrefProperty, ItemDetails, ResponseWithProp, SimplePropertyMeta, StringProperty, DAV,
};

mod dav;
pub mod dns;
pub mod xml;

pub use dav::DavError;

/// Authentication schemes supported by [`CalDavClient`].
#[non_exhaustive]
#[derive(Debug)]
pub enum Auth {
    None,
    Basic {
        username: String,
        password: Option<String>,
    },
}

/// Internal error resolving authentication.
#[derive(thiserror::Error, Debug)]
#[error(transparent)]
pub struct AuthError(Box<dyn std::error::Error + Sync + Send>);

impl AuthError {
    fn from<E: std::error::Error + Sync + Send + 'static>(err: E) -> Self {
        Self(Box::from(err))
    }
}

impl Auth {
    /// Apply this authentication to a request builder.
    fn new_request(&self) -> Result<http::request::Builder, AuthError> {
        let request = Request::builder();
        match self {
            Auth::None => Ok(request),
            Auth::Basic { username, password } => {
                let mut sequence = b"Basic ".to_vec();
                let mut encoder = EncoderWriter::new(&mut sequence, &BASE64_STANDARD);
                if let Some(pwd) = password {
                    write!(encoder, "{username}:{pwd}").map_err(AuthError::from)?;
                } else {
                    write!(encoder, "{username}:").map_err(AuthError::from)?;
                }
                drop(encoder); // Releases the mutable borrow for `sequence`.

                let mut header = HeaderValue::from_bytes(&sequence).map_err(AuthError::from)?;
                header.set_sensitive(true);
                Ok(request.header(hyper::header::AUTHORIZATION, header.clone()))
            }
        }
    }
}

/// A client to communicate with a CalDav server.
// TODO FIXME: Need to figure out how to reuse as much as possible for carddav and caldav.
#[derive(Debug)]
pub struct CalDavClient {
    /// Base URL to be used for all requests.
    ///
    /// This may be (due to bootstrapping discovery) a path than the one provided as input.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc6764#section-1>
    base_url: Uri,
    auth: Auth,
    // TODO: we can eventually use a generic connector to allow explicitly
    // using caldav or caldavs.
    http_client: Client<HttpsConnector<HttpConnector>>,
    /// URL to a principal resource corresponding to the currently authenticated user.
    /// See: <https://www.rfc-editor.org/rfc/rfc5397#section-3>
    pub principal: Option<Uri>,
    /// URL of collections that are either calendar collections or ordinary collections
    /// that have child or descendant calendar collections owned by the principal.
    /// See: <https://www.rfc-editor.org/rfc/rfc4791#section-6.2.1>
    pub calendar_home_set: Option<Uri>, // TODO: timeouts
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
            BootstrapError::DnsError => io::Error::new(io::ErrorKind::Other, value),
            BootstrapError::BadSrv(_) => io::Error::new(io::ErrorKind::InvalidData, value),
            BootstrapError::TxtError(_) => io::Error::new(io::ErrorKind::Other, value),
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

        let https = HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_only()
            .enable_http1()
            .build();

        Self {
            base_url,
            auth,
            http_client: Client::builder().build(https),
            principal: None,
            calendar_home_set: None,
        }
    }

    // TODO: methods to serialise and deserialise (mostly to cache all discovery data).

    /// Returns a request builder with the proper `Authorization` header set.
    fn request(&self) -> Result<http::request::Builder, AuthError> {
        self.auth.new_request()
    }

    /// Returns the default port to try and use.
    ///
    /// If the `base_url` has an explicit port, use that one. Use `443` for https,
    /// `80` for http, and `443` as a fallback for anything else.
    fn default_port(&self) -> u16 {
        self.base_url
            .port_u16()
            .unwrap_or_else(|| match self.base_url.scheme() {
                Some(scheme) if scheme == "https" => 443,
                Some(scheme) if scheme == "http" => 80,
                _ => 443,
            })
    }

    /// Auto-bootstrap a new client.
    ///
    /// Determines the CalDav server's real host and the context path of the resources for a
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

    /// Returns a URL pointing to the server's context path.
    pub fn context_path(&self) -> &Uri {
        &self.base_url
    }

    /// Returns a new URI relative to the server's context path.
    ///
    /// # Errors
    ///
    /// If constructing a new URI fails.
    pub fn relative_uri(&self, path: &str) -> Result<Uri, http::Error> {
        let mut parts = self.base_url.clone().into_parts();
        parts.path_and_query = Some(path.try_into().map_err(http::Error::from)?);
        Uri::from_parts(parts).map_err(http::Error::from)
    }

    /// Resolve the default context path with the well-known URL.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc6764#section-5>
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
            .body(Default::default())?;

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

    /// Resolves the current user's principal resource.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc5397>
    pub async fn resolve_current_user_principal(&self) -> Result<Option<Uri>, DavError> {
        // Try querying the provided base url...
        let maybe_principal = self
            .query_current_user_principal(self.base_url.clone())
            .await;

        match maybe_principal {
            Err(DavError::BadStatusCode(StatusCode::NOT_FOUND)) => {}
            Err(err) => return Err(err),
            Ok(Some(p)) => return Ok(Some(p)),
            Ok(None) => {}
        };

        // ... Otherwise, try querying the root path.
        let root = self.relative_uri("/")?;
        self.query_current_user_principal(root).await // Hint: This can be Ok(None)

        // NOTE: If no principal is resolved, it needs to be provided interactively
        //       by the user. We use `base_url` as a fallback.
    }

    async fn query_current_user_principal(&self, url: Uri) -> Result<Option<Uri>, DavError> {
        let property_data = SimplePropertyMeta {
            name: b"current-user-principal".to_vec(),
            namespace: xml::DAV.to_vec(),
        };

        self.find_href_prop_as_uri(url, "<current-user-principal/>", property_data)
            .await
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

    /// Internal helper to find an `href` property
    ///
    /// Very specific, but de-duplicates two identical methods above.
    async fn find_href_prop_as_uri(
        &self,
        url: Uri,
        prop: &str,
        prop_type: SimplePropertyMeta,
    ) -> Result<Option<Uri>, DavError> {
        let maybe_href = match self
            .propfind::<ResponseWithProp<HrefProperty>>(url.clone(), prop, 0, prop_type)
            .await?
            .pop()
            .transpose()?
        {
            Some(prop) => prop.into_maybe_string(),
            None => return Ok(None),
        };

        if let Some(href) = maybe_href {
            let path = href
                .try_into()
                .map_err(|e| DavError::InvalidResponse(Box::from(e)))?;

            let mut parts = url.into_parts();
            parts.path_and_query = Some(path);
            Some(Uri::from_parts(parts))
                .transpose()
                .map_err(|e| DavError::InvalidResponse(Box::from(e)))
        } else {
            Ok(None)
        }
    }

    /// Find calendars collections under the given `url`.
    ///
    /// Returns absolute paths to each calendar. This method should be called
    /// the `calendar_home_set` URL to find the current user's calendars.
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

    /// Sends a `PROPFIND` request and parses the result.
    async fn propfind<T: FromXml>(
        &self,
        url: Uri,
        prop: &str,
        depth: u8,
        data: T::Data,
    ) -> Result<Vec<Result<T, xml::Error>>, DavError> {
        let request = self
            .request()?
            .method(Method::from_bytes(b"PROPFIND").expect("API for HTTP methods is stupid"))
            .uri(url)
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", format!("{depth}"))
            .body(Body::from(format!(
                r#"
                <propfind xmlns="DAV:">
                    <prop>
                        {prop}
                    </prop>
                </propfind>
                "#
            )))?;
        let response = self.http_client.request(request).await?;
        let (head, body) = response.into_parts();
        if !head.status.is_success() {
            return Err(DavError::BadStatusCode(head.status));
        }

        let body = hyper::body::to_bytes(body).await?;
        xml::parse_multistatus::<T>(&body, data).map_err(DavError::from)
    }

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
