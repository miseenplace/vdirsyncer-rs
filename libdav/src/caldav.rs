use std::ops::{Deref, DerefMut};

use http::Method;
use hyper::{Body, Uri};
use log::debug;

use crate::builder::{ClientBuilder, NeedsUri};
use crate::dav::{check_status, DavError, FoundCollection};
use crate::dns::DiscoverableService;
use crate::xml::{ItemDetails, ReportField, ResponseVariant, SimplePropertyMeta, StringProperty};
use crate::{common_bootstrap, CheckSupportError, FetchedResource};
use crate::{dav::WebDavClient, BootstrapError, FindHomeSetError};

/// A client to communicate with a caldav server.
///
/// Instances are created via a builder:
///
/// ```rust,no_run
/// # use libdav::CalDavClient;
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
/// let client = CalDavClient::builder()
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
pub struct CalDavClient {
    /// The `base_url` may be (due to bootstrapping discovery) different to the one provided as input.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc6764#section-1>
    dav_client: WebDavClient,
    /// URL of collections that are either calendar collections or ordinary collections
    /// that have child or descendant calendar collections owned by the principal.
    /// See: <https://www.rfc-editor.org/rfc/rfc4791#section-6.2.1>
    ///
    /// This field is automatically populated by [`auto_bootstrap`][Self::auto_bootstrap].
    pub calendar_home_set: Option<Uri>, // TODO: timeouts
}

impl Deref for CalDavClient {
    type Target = WebDavClient;

    fn deref(&self) -> &Self::Target {
        &self.dav_client
    }
}
impl DerefMut for CalDavClient {
    fn deref_mut(&mut self) -> &mut crate::dav::WebDavClient {
        &mut self.dav_client
    }
}

impl ClientBuilder<CalDavClient, crate::builder::Ready> {
    /// Return a built client.
    pub fn build(self) -> CalDavClient {
        CalDavClient {
            dav_client: WebDavClient::new(self.state.uri, self.state.auth),
            calendar_home_set: None,
        }
    }
}

impl CalDavClient {
    /// Creates a new builder. See [`CalDavClient`] and [`ClientBuilder`] for details.
    #[must_use]
    pub fn builder() -> ClientBuilder<Self, NeedsUri> {
        ClientBuilder::new()
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
    pub async fn auto_bootstrap(mut self) -> Result<Self, BootstrapError> {
        let port = self.default_port()?;
        let service = self.service()?;
        common_bootstrap(&mut self, port, service).await?;

        // If obtaining a principal fails, the specification says we should query the user. This
        // tries to use the `base_url` first, since the user might have provided it for a reason.
        let principal_url = self.principal.as_ref().unwrap_or(&self.base_url);
        self.calendar_home_set = self.find_calendar_home_set(principal_url).await?;

        Ok(self)
    }

    async fn find_calendar_home_set(&self, url: &Uri) -> Result<Option<Uri>, FindHomeSetError> {
        let property_data = SimplePropertyMeta {
            name: b"calendar-home-set".to_vec(),
            namespace: crate::xml::CALDAV.to_vec(),
        };

        self.find_href_prop_as_uri(
            url,
            "<calendar-home-set xmlns=\"urn:ietf:params:xml:ns:caldav\"/>",
            &property_data,
        )
        .await
        .map_err(FindHomeSetError)
    }

    /// Find calendars collections under the given `url`.
    ///
    /// It `url` is not specified, this client's calendar home set is used instead. If no calendar
    /// home set has been found, then the server's context path will be used. When using a client
    /// bootstrapped via automatic discovery, passing `None` will usually yield the expected
    /// results.
    ///
    /// # Errors
    ///
    /// If the HTTP call fails or parsing the XML response fails.
    pub async fn find_calendars(
        &self,
        url: Option<&Uri>,
    ) -> Result<Vec<FoundCollection>, DavError> {
        let url = url.unwrap_or(self.calendar_home_set.as_ref().unwrap_or(&self.base_url));
        let items = self
            .propfind::<ItemDetails>(url, "<resourcetype/><getetag/>", 1, &())
            .await
            .map_err(DavError::from)?
            .into_iter()
            .filter_map(|c| match c.variant {
                ResponseVariant::WithProps { propstats } => {
                    if propstats.iter().any(|p| p.prop.is_calendar) {
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

        self.propfind::<StringProperty>(
            &url,
            "<calendar-color xmlns=\"http://apple.com/ns/ical/\"/>",
            0,
            &property_data,
        )
        .await?
        .pop()
        .ok_or(DavError::from(crate::xml::Error::MissingData(
            "calendar-color",
        )))
        .map(Option::<String>::from)
    }

    // TODO: get_calendar_description ("calendar-description", "urn:ietf:params:xml:ns:caldav")
    // TODO: get_calendar_order ("calendar-order", "http://apple.com/ns/ical/")
    // TODO: DRY: the above methods are super repetitive.
    //       Maybe all these props impl a single trait, so the API could be `get_prop<T>(url)`?

    /// Fetches existing icalendar resources.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](WebDavClient::request_multistatus).
    pub async fn get_resources<S1, S2>(
        &self,
        calendar_href: S1,
        hrefs: &[S2],
    ) -> Result<Vec<FetchedResource>, DavError>
    where
        S1: AsRef<str>,
        S2: AsRef<str>,
    {
        let mut body = String::from(
            r#"
            <C:calendar-multiget xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
                <prop>
                    <getetag/>
                    <C:calendar-data/>
                </prop>"#,
        );
        for href in hrefs {
            body.push_str(&format!("<href>{}</href>", href.as_ref()));
        }
        body.push_str("</C:calendar-multiget>");

        self.multi_get(calendar_href.as_ref(), body, &ReportField::CALENDAR_DATA)
            .await
    }

    /// Checks that the given URI advertises caldav support.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc4791#section-5.1>
    ///
    /// # Known Issues
    ///
    /// - This is currently broken on Nextcloud. [Bug report][nextcloud].
    ///
    /// [nextcloud]: https://github.com/nextcloud/server/issues/37374
    ///
    /// # Errors
    ///
    /// If there are any network issues or if the server does not explicitly advertise caldav
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
            .any(|part| part.trim() == "calendar-access")
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
    #[inline]
    fn default_port(&self) -> Result<u16, BootstrapError> {
        if let Some(port) = self.base_url.port_u16() {
            Ok(port)
        } else {
            match self.base_url.scheme() {
                Some(scheme) if scheme == "https" => Ok(443),
                Some(scheme) if scheme == "http" => Ok(80),
                Some(scheme) if scheme == "caldavs" => Ok(443),
                Some(scheme) if scheme == "caldav" => Ok(80),
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
