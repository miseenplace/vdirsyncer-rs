//! This library contains caldav and carddav clients.
//!
//! See [`CalDavClient`] and [`CardDavClient`] as a useful entry points.
use std::{
    io,
    ops::{Deref, DerefMut},
};

use crate::auth::{Auth, AuthError};
use async_trait::async_trait;
use dav::DavClient;
use dav::DavError;
use dns::{
    find_context_path_via_txt_records, resolve_srv_record, DiscoverableService, SrvError, TxtError,
};
use domain::base::Dname;
use hyper::Uri;
use xml::{ItemDetails, ResponseWithProp, SimplePropertyMeta, StringProperty};

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
    InvalidUrl(&'static str),

    #[error("error resolving DNS SRV records")]
    DnsError(SrvError),

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
            BootstrapError::InvalidUrl(msg) => io::Error::new(io::ErrorKind::InvalidInput, msg),
            BootstrapError::DnsError(_) | BootstrapError::TxtError(_) => {
                io::Error::new(io::ErrorKind::Other, value)
            }
            BootstrapError::BadSrv(_) => io::Error::new(io::ErrorKind::InvalidData, value),
            BootstrapError::DavError(dav) => io::Error::from(dav),
        }
    }
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
        client = client.common_bootstrap().await?;

        // If obtaining a principal fails, the specification says we should query the user. This
        // tries to use the `base_url` first, since the user might have provided it for a reason.
        let principal_url = client
            .principal
            .as_ref()
            .unwrap_or(&client.base_url)
            .clone();
        client.calendar_home_set = client.find_calendar_home_set(principal_url).await?;

        Ok(client)
    }

    async fn find_calendar_home_set(&self, url: Uri) -> Result<Option<Uri>, DavError> {
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
}

#[async_trait]
impl DavWithAutoDiscovery for CalDavClient {
    #[inline]

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

    fn set_principal(&mut self, principal: Option<Uri>) {
        self.principal = principal
    }
}

#[derive(Debug)]
pub struct CardDavClient {
    /// The `base_url` may be (due to bootstrapping discovery) different to the one provided as input.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc6764#section-1>
    dav_client: DavClient,
    /// URL of collections that are either address book collections or ordinary collections
    /// that have child or descendant address book collections owned by the principal.
    /// See: <https://www.rfc-editor.org/rfc/rfc4791#section-6.2.1>
    pub addressbook_home_set: Option<Uri>, // TODO: timeouts
}

impl Deref for CardDavClient {
    type Target = DavClient;

    fn deref(&self) -> &Self::Target {
        &self.dav_client
    }
}
impl DerefMut for CardDavClient {
    fn deref_mut(&mut self) -> &mut dav::DavClient {
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

    async fn find_addressbook_home_set(&self, url: Uri) -> Result<Option<Uri>, DavError> {
        let property_data = SimplePropertyMeta {
            name: b"addressbook-home-set".to_vec(),
            namespace: xml::CARDDAV.to_vec(),
        };

        self.find_href_prop_as_uri(
            url,
            "<addressbook-home-set xmlns=\"urn:ietf:params:xml:ns:carddav\"/>",
            property_data,
        )
        .await
    }

    /// Find address book collections under the given `url`.
    ///
    /// Returns absolute paths to each addressbook. This method should be called
    /// with the `addressbook_home_set` URL to find the current user's address books.
    ///
    /// # Errors
    ///
    /// If the HTTP call fails or parsing the XML response fails.
    pub async fn find_addresbooks(&self, url: Uri) -> Result<Vec<String>, DavError> {
        // FIXME: DRY: This is almost a copy-paste of the same method from CalDavClient
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
                    cal.prop.is_address_book
                } else {
                    true
                }
            })
            // FIXME: silently ignores collections with any issues:
            .filter_map(|r| r.map(|c| c.href).ok())
            .collect())
    }

    // TODO: get_addressbook_description ("addressbook-description", "urn:ietf:params:xml:ns:carddav")
    // TODO: DRY: the above methods are super repetitive.
    //       Maybe all these props impl a single trait, so the API could be `get_prop<T>(url)`?
}

#[async_trait]
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
        self.principal = principal
    }
}

/// Trait implementing some common bits between CardDav and CalDav.
///
/// This trait is deliberately made private; it's just a convenient recipe to reuse
/// some bits of code.
#[async_trait]
pub(crate) trait DavWithAutoDiscovery:
    DerefMut<Target = DavClient> + Sized + Send + Sync
{
    fn default_port(&self) -> Result<u16, BootstrapError>;
    fn service(&self) -> Result<DiscoverableService, BootstrapError>;
    fn set_principal(&mut self, principal: Option<Uri>);

    /// A big chunk of the bootstrap logic that's shared between both types.
    ///
    /// NOTE: This is not public. Both `CalDavClient` and `CardDavClient` wrap this with extra steps.
    async fn common_bootstrap(mut self) -> Result<Self, BootstrapError> {
        let domain = self
            .base_url
            .host()
            .ok_or(BootstrapError::InvalidUrl("a host is required"))?;
        let port = self.default_port()?;
        let service = self.service()?;

        let dname = Dname::bytes_from_str(domain)
            .map_err(|_| BootstrapError::InvalidUrl("invalid domain name"))?;
        let candidates = {
            let mut candidates = resolve_srv_record(service, &dname, port)
                .await
                .map_err(BootstrapError::DnsError)?;

            // If there are no SRV records, try the domain/port in the provided URI.
            if candidates.is_empty() {
                candidates.push((domain.to_string(), port));
            }
            candidates
        };

        if let Some(path) = find_context_path_via_txt_records(service, &dname).await? {
            // TODO: validate that the path works on the chosen server.
            let candidate = &candidates[0];

            self.base_url = Uri::builder()
                .scheme(service.scheme())
                .authority(format!("{}:{}", candidate.0, candidate.1))
                .path_and_query(path)
                .build()
                .map_err(BootstrapError::BadSrv)?;
        } else {
            for candidate in candidates {
                if let Ok(Some(url)) = self
                    .find_context_path(service, &candidate.0, candidate.1)
                    .await
                {
                    self.base_url = url;
                    break;
                }
            }
        }

        self.set_principal(self.find_current_user_principal().await?);
        Ok(self)
    }
}
