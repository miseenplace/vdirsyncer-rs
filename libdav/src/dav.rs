//! Generic webdav implementation.
use std::io;

use http::{Method, StatusCode, Uri};
use hyper::{client::HttpConnector, Body, Client};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};

use crate::{
    xml::{
        self, FromXml, HrefProperty, ItemDetails, ResponseWithProp, SimplePropertyMeta,
        StringProperty, DAV,
    },
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
            DavError::BadStatusCode(_) | DavError::Auth(_) => {
                io::Error::new(io::ErrorKind::Other, value)
            }
            DavError::InvalidInput(e) => io::Error::new(io::ErrorKind::InvalidInput, e),
            DavError::InvalidResponse(e) => io::Error::new(io::ErrorKind::InvalidData, e),
        }
    }
}

/// A generic webdav client.
#[derive(Debug)]
pub struct DavClient {
    /// Base URL to be used for all requests.
    pub(crate) base_url: Uri,
    pub(crate) auth: Auth,
    // TODO: we can eventually use a generic connector to allow explicitly
    // using caldav or caldavs.
    pub(crate) http_client: Client<HttpsConnector<HttpConnector>>,
    /// URL to a principal resource corresponding to the currently authenticated user.
    ///
    /// In order to determine the principal, see [`query_current_user_principal`].
    ///
    /// [`query_current_user_principal`]: (DavClient::query_current_user_principal).
    ///
    /// # See also
    ///
    /// - <https://www.rfc-editor.org/rfc/rfc5397#section-3>
    pub principal: Option<Uri>,
}

impl DavClient {
    /// Builds a new webdav client.
    pub fn new(base_url: Uri, auth: Auth) -> DavClient {
        let https = HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_only()
            .enable_http1()
            .build();
        DavClient {
            base_url,
            auth,
            http_client: Client::builder().build(https),
            principal: None,
        }
    }

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
    /// If this client's `base_url` is invalid or the provided `path` is not an acceptable path.
    pub fn relative_uri(&self, path: &str) -> Result<Uri, http::Error> {
        let mut parts = self.base_url.clone().into_parts();
        parts.path_and_query = Some(path.try_into().map_err(http::Error::from)?);
        Uri::from_parts(parts).map_err(http::Error::from)
    }

    /// Resolves the current user's principal resource.
    ///
    /// Returns `None` if the response's status code is 404 or if no principal was found.
    ///
    /// # Errors
    ///
    /// - If the underlying HTTP request fails.
    /// - If the response status code is neither success nor 404.
    /// - If parsing the XML response fails.
    ///
    /// # See also
    ///
    /// - <https://www.rfc-editor.org/rfc/rfc5397>
    pub async fn resolve_current_user_principal(&self) -> Result<Option<Uri>, DavError> {
        // Try querying the provided base url...
        let maybe_principal = self
            .query_current_user_principal(self.base_url.clone())
            .await;

        match maybe_principal {
            Err(DavError::BadStatusCode(StatusCode::NOT_FOUND)) | Ok(None) => {}
            Err(err) => return Err(err),
            Ok(Some(p)) => return Ok(Some(p)),
        };

        // ... Otherwise, try querying the root path.
        let root = self.relative_uri("/")?;
        self.query_current_user_principal(root).await // Hint: This can be Ok(None)

        // NOTE: If no principal is resolved, it needs to be provided interactively
        //       by the user. We use `base_url` as a fallback.
    }

    /// Find a URL for the currently authenticated user's principal resource on the server.
    ///
    /// Returns `None` if the response is valid but does not contain any `href`.
    ///
    /// # Errors
    ///
    /// - If there are any network issues.
    /// - If parsing the XML response fails.
    /// - If the `href` cannot be parsed into a valid [`Uri`]
    ///
    /// # See also
    ///
    /// - <https://www.rfc-editor.org/rfc/rfc5397>
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
    ///
    /// # Errors
    ///
    /// - If the network request fails.
    /// - If the response is not successful (e.g.L in the 200-299 range).
    /// - If parsing the XML fails.
    /// - If the XML does not match the parametrized type.
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
        xml::parse_multistatus::<T>(&body, &data).map_err(DavError::from)
    }

    /// Returns the `displayname` for the collection at path `href`.
    ///
    /// From [rfc3744#section-4](https://www.rfc-editor.org/rfc/rfc3744#section-4):
    ///
    /// > A principal MUST have a non-empty DAV:displayname property
    ///
    /// # Errors
    ///
    /// If the HTTP call fails or parsing the XML response fails.
    pub async fn get_collection_displayname(&self, href: &str) -> Result<Option<String>, DavError> {
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

    /// Enumerates entries in a collection
    ///
    /// Returns an array of results. Because the server can return a non-ok status for individual
    /// entries, some of them may be `Err`, while other are `Ok(ItemDetails)`.
    ///
    /// Note that the collection itself is also present as an item.
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
    }
}
