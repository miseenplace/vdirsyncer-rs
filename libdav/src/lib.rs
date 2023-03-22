//! This library contains caldav and carddav clients.
//!
//! See [`CalDavClient`] and [`CardDavClient`] as a useful entry points.
//!
//! Both clients implement `Deref<Target = DavClient>`, so all the associated
//! functions for [`dav::DavClient`] are usable directly.
use std::{io, ops::DerefMut};

use crate::auth::{Auth, AuthError};
use async_trait::async_trait;
use dav::DavError;
use dav::{DavClient, FindCurrentUserPrincipalError};
use dns::{
    find_context_path_via_txt_records, resolve_srv_record, DiscoverableService, SrvError, TxtError,
};
use domain::base::Dname;
use http::StatusCode;
use hyper::Uri;

pub mod auth;
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
    BadSrv(http::Error),

    #[error("error resolving context path via TXT records")]
    TxtError(#[from] TxtError),

    #[error(transparent)]
    HomeSet(#[from] FindHomeSetError),

    #[error(transparent)]
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
            BootstrapError::BadSrv(_) => io::Error::new(io::ErrorKind::InvalidData, value),
            BootstrapError::DavError(dav) => io::Error::from(dav),
        }
    }
}

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
