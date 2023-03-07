//! Generic webdav implementation.
use std::io;

use http::{Method, StatusCode, Uri};
use hyper::{client::HttpConnector, Body, Client};
use hyper_rustls::HttpsConnector;

use crate::{
    xml::{self, FromXml, HrefProperty, ResponseWithProp, SimplePropertyMeta},
    Auth, AuthError,
};

/// A generic error for `WebDav` operations.
#[derive(thiserror::Error, Debug)]
pub enum DavError {
    #[error("http error executing request")]
    Network(#[from] hyper::Error),

    #[error("failure parsing XML response")]
    Xml(#[from] crate::xml::Error),

    #[error("a request did not return a successful status code")]
    BadStatusCode(http::StatusCode),

    #[error("failed to build URL with the given input")]
    InvalidInput(#[from] http::Error),

    #[error("internal error with specified authentication")]
    Auth(#[from] crate::AuthError),

    #[error("the server returned an invalid response")]
    InvalidResponse(Box<dyn std::error::Error + Send + Sync>),
}

impl From<DavError> for io::Error {
    fn from(value: DavError) -> Self {
        match value {
            DavError::Network(e) => io::Error::new(io::ErrorKind::Other, e),
            DavError::Xml(e) => io::Error::new(io::ErrorKind::InvalidData, e),
            DavError::BadStatusCode(_) => io::Error::new(io::ErrorKind::Other, value),
            DavError::InvalidInput(e) => io::Error::new(io::ErrorKind::InvalidInput, e),
            DavError::Auth(_) => io::Error::new(io::ErrorKind::Other, value),
            DavError::InvalidResponse(e) => io::Error::new(io::ErrorKind::InvalidData, e),
        }
    }
}

#[derive(Debug)]
pub struct DavClient {
    /// Base URL to be used for all requests.
    pub(crate) base_url: Uri,
    pub(crate) auth: Auth,
    // TODO: we can eventually use a generic connector to allow explicitly
    // using caldav or caldavs.
    pub(crate) http_client: Client<HttpsConnector<HttpConnector>>,
    /// URL to a principal resource corresponding to the currently authenticated user.
    /// See: <https://www.rfc-editor.org/rfc/rfc5397#section-3>
    pub principal: Option<Uri>,
}

impl DavClient {
    /// Returns a request builder with the proper `Authorization` header set.
    pub(crate) fn request(&self) -> Result<http::request::Builder, AuthError> {
        // TODO: this isn't a great API. Maybe a `BuilderExt` trait would be a better pattern?
        self.auth.new_request()
    }

    /// Returns the default port to try and use.
    ///
    /// If the `base_url` has an explicit port, that value is returned. Otherwise,
    /// returns `443` for https, `80` for http, and `443` as a fallback for
    /// anything else.
    pub fn default_port(&self) -> u16 {
        self.base_url
            .port_u16()
            .unwrap_or_else(|| match self.base_url.scheme() {
                Some(scheme) if scheme == "https" => 443,
                Some(scheme) if scheme == "http" => 80,
                _ => 443,
            })
    }

    /// Returns a URL pointing to the server's context path.
    pub fn context_path(&self) -> &Uri {
        &self.base_url
    }

    /// Returns a new URI relative to the server's root.
    ///
    /// # Errors
    ///
    /// If constructing a new URI fails.
    pub fn relative_uri(&self, path: &str) -> Result<Uri, http::Error> {
        let mut parts = self.base_url.clone().into_parts();
        parts.path_and_query = Some(path.try_into().map_err(http::Error::from)?);
        Uri::from_parts(parts).map_err(http::Error::from)
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

    /// Find a URL for the currently authenticated user's principal resource on the server.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc5397>
    pub async fn query_current_user_principal(&self, url: Uri) -> Result<Option<Uri>, DavError> {
        let property_data = SimplePropertyMeta {
            name: b"current-user-principal".to_vec(),
            namespace: xml::DAV.to_vec(),
        };

        self.find_href_prop_as_uri(url, "<current-user-principal/>", property_data)
            .await
    }

    /// Internal helper to find an `href` property
    ///
    /// Very specific, but de-duplicates a few identical methods.
    pub(crate) async fn find_href_prop_as_uri(
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

    /// Sends a `PROPFIND` request and parses the result.
    pub async fn propfind<T: FromXml>(
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
}
