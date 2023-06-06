#![warn(clippy::pedantic)]

//! This library contains caldav and carddav clients.
//!
//! See [`CalDavClient`] and [`CardDavClient`] as a useful entry points.
//!
//! Both clients implement `Deref<Target = DavClient>`, so all the associated
//! functions for [`dav::WebDavClient`] are usable directly.
use std::io;

use crate::auth::{Auth, AuthError};
use dav::DavError;
use dav::{FindCurrentUserPrincipalError, WebDavClient};
use dns::{
    find_context_path_via_txt_records, resolve_srv_record, DiscoverableService, SrvError, TxtError,
};
use domain::base::Dname;
use http::StatusCode;
use hyper::Uri;

pub mod auth;
pub mod builder;
mod caldav;
mod carddav;
pub mod dav;
pub mod dns;
pub mod xml;

pub use caldav::CalDavClient;
pub use carddav::CardDavClient;

/// An error automatically bootstrapping a new client.
#[derive(thiserror::Error, Debug)]
pub enum BootstrapError {
    #[error("the input URL is not valid")]
    InvalidUrl(&'static str),

    #[error("error resolving DNS SRV records")]
    DnsError(SrvError),

    #[error("SRV records returned domain/port pair that failed to parse")]
    UnusableSrv(http::Error),

    #[error("error resolving context path via TXT records")]
    TxtError(#[from] TxtError),

    #[error(transparent)]
    HomeSet(#[from] FindHomeSetError),

    #[error("error querying current user principal")]
    CurrentPrincipal(#[from] FindCurrentUserPrincipalError),

    #[error(transparent)]
    DavError(#[from] DavError),
}

impl From<BootstrapError> for io::Error {
    fn from(value: BootstrapError) -> Self {
        match value {
            BootstrapError::InvalidUrl(msg) => io::Error::new(io::ErrorKind::InvalidInput, msg),
            BootstrapError::DnsError(_)
            | BootstrapError::TxtError(_)
            | BootstrapError::HomeSet(_)
            | BootstrapError::CurrentPrincipal(_) => io::Error::new(io::ErrorKind::Other, value),
            BootstrapError::UnusableSrv(_) => io::Error::new(io::ErrorKind::InvalidData, value),
            BootstrapError::DavError(dav) => io::Error::from(dav),
        }
    }
}

/// Error finding home set.
#[derive(thiserror::Error, Debug)]
#[error("error finding home set collection")]
pub struct FindHomeSetError(pub DavError);

impl<T> From<T> for FindHomeSetError
where
    DavError: std::convert::From<T>,
{
    fn from(value: T) -> Self {
        FindHomeSetError(DavError::from(value))
    }
}

/// A big chunk of the bootstrap logic that's shared between both types.
///
/// Mutates the `base_url` for the client to the discovered one.
async fn common_bootstrap(
    client: &mut WebDavClient,
    port: u16,
    service: DiscoverableService,
) -> Result<(), BootstrapError> {
    let domain = client
        .base_url
        .host()
        .ok_or(BootstrapError::InvalidUrl("a host is required"))?;

    let dname = Dname::bytes_from_str(domain)
        .map_err(|_| BootstrapError::InvalidUrl("invalid domain name"))?;
    let host_candidates = {
        let candidates = resolve_srv_record(service, &dname, port)
            .await
            .map_err(BootstrapError::DnsError)?;

        // If there are no SRV records, try the domain/port in the provided URI.
        if candidates.is_empty() {
            vec![(domain.to_string(), port)]
        } else {
            candidates
        }
    };

    if let Some(path) = find_context_path_via_txt_records(service, &dname).await? {
        let candidate = &host_candidates[0];

        // TODO: check `DAV:` capabilities here.
        client.base_url = Uri::builder()
            .scheme(service.scheme())
            .authority(format!("{}:{}", candidate.0, candidate.1))
            .path_and_query(path)
            .build()
            .map_err(BootstrapError::UnusableSrv)?;
    } else {
        for candidate in host_candidates {
            if let Ok(Some(url)) = client
                .find_context_path(service, &candidate.0, candidate.1)
                .await
            {
                client.base_url = url;
                break;
            }
        }
    }

    client.principal = client.find_current_user_principal().await?;

    Ok(())
}

/// See [`FetchedResource`]
#[derive(Debug, PartialEq, Eq)]
pub struct FetchedResourceContent {
    pub data: String,
    pub etag: String,
}

/// A parsed resource fetched from a server.
#[derive(Debug, PartialEq, Eq)]
pub struct FetchedResource {
    /// The absolute path to the resource in the server.
    pub href: String,
    /// The contents of the resource if available, or the status code if unavailable.
    pub content: Result<FetchedResourceContent, StatusCode>,
}

/// Returned when checking support for a feature encounters an error.
#[derive(thiserror::Error, Debug)]
pub enum CheckSupportError {
    #[error("the DAV header was missing from the response")]
    MissingHeader,

    #[error("the requested support is not advertised by the server")]
    NotAdvertised,

    #[error("the DAV header is not a valid string")]
    HeaderNotAscii(#[from] http::header::ToStrError),

    #[error("http error executing request")]
    Network(#[from] hyper::Error),

    #[error("invalid input URL")]
    InvalidInput(#[from] http::Error),

    #[error("internal error with specified authentication")]
    Auth(#[from] crate::AuthError),

    #[error("http request returned {0}")]
    BadStatusCode(http::StatusCode),
}

impl From<StatusCode> for CheckSupportError {
    fn from(status: StatusCode) -> Self {
        CheckSupportError::BadStatusCode(status)
    }
}
