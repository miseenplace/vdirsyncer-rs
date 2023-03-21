//! Generic webdav implementation.
use std::io;

use http::{Method, Request, StatusCode, Uri};
use hyper::{client::HttpConnector, Body, Client};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};

use crate::{
    auth::AuthExt,
    dns::DiscoverableService,
    xml::{
        self, FromXml, HrefProperty, ItemDetails, Multistatus, ResponseWithProp,
        SimplePropertyMeta, StringProperty, CALDAV_STR, CARDDAV_STR, DAV,
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

#[derive(thiserror::Error, Debug)]
pub enum ResolveContextPathError {
    #[error("failed to create uri and request with given parameters")]
    BadInput(#[from] http::Error),

    #[error("bad scheme in url")]
    BadScheme,

    #[error("network error handling http stream")]
    Network(#[from] hyper::Error),

    #[error("missing Location header in response")]
    MissingLocation,

    // TODO: somehow merge these two into one.
    #[error("error building new Uri with Location from response")]
    BadRelativeLocation(#[from] std::str::Utf8Error),
    #[error("error building new Uri with Location from response")]
    BadAbsoluteLocation(#[from] http::uri::InvalidUri),

    #[error("internal error with specified authentication")]
    Auth(#[from] AuthError),
}

#[derive(thiserror::Error, Debug)]
pub enum FindCurrentUserPrincipalError {
    #[error("error sending or parsing with request")]
    RequestError(#[from] DavError),

    // XXX: This should not really happen, but the API for `http` won't let us validate this
    // earlier with a clear approach.
    #[error("cannot use base_url to build request uri")]
    InvalidInput(#[from] http::Error),
}

/// A generic webdav client.
#[derive(Debug)]
pub struct DavClient {
    /// Base URL to be used for all requests.
    pub(crate) base_url: Uri,
    auth: Auth,
    // TODO: we can eventually use a generic connector to allow explicitly
    // using caldav or caldavs.
    http_client: Client<HttpsConnector<HttpConnector>>,
    /// URL to a principal resource corresponding to the currently authenticated user.
    ///
    /// In order to determine the principal, see [`find_current_user_principal`].
    ///
    /// [`find_current_user_principal`]: (DavClient::find_current_user_principal).
    ///
    /// # See also
    ///
    /// - <https://www.rfc-editor.org/rfc/rfc5397#section-3>
    pub(crate) principal: Option<Uri>,
}

impl DavClient {
    /// Builds a new webdav client.
    ///
    /// Only `https` is enabled by default. Plain-text `http` is only enabled if the
    /// input uri has a scheme of `http` or `caldav`.
    pub fn new(base_url: Uri, auth: Auth) -> DavClient {
        let builder = HttpsConnectorBuilder::new().with_native_roots();
        let builder = match base_url.scheme() {
            Some(scheme) if scheme.as_str() == "http" => builder.https_or_http(),
            Some(scheme) if scheme.as_str() == "caldav" => builder.https_or_http(),
            _ => builder.https_only(),
        };

        let https = builder.enable_http1().build();
        DavClient {
            base_url,
            auth,
            http_client: Client::builder().build(https),
            principal: None,
        }
    }

    /// Returns a request builder with the proper `Authorization` header set.
    pub(crate) fn request_builder(&self) -> Result<http::request::Builder, AuthError> {
        Request::builder().authenticate(&self.auth)
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
    /// - If the `href` cannot be parsed into a valid [`Uri`]
    ///
    /// # See also
    ///
    /// - <https://www.rfc-editor.org/rfc/rfc5397>
    pub async fn find_current_user_principal(
        &self,
    ) -> Result<Option<Uri>, FindCurrentUserPrincipalError> {
        let property_data = SimplePropertyMeta {
            name: b"current-user-principal".to_vec(),
            namespace: xml::DAV.to_vec(),
        };

        // Try querying the provided base url...
        let maybe_principal = self
            .find_href_prop_as_uri(
                self.base_url.clone(),
                "<current-user-principal/>",
                &property_data,
            )
            .await;

        match maybe_principal {
            Err(DavError::BadStatusCode(StatusCode::NOT_FOUND)) | Ok(None) => {}
            Err(err) => return Err(FindCurrentUserPrincipalError::RequestError(err)),
            Ok(Some(p)) => return Ok(Some(p)),
        };

        // ... Otherwise, try querying the root path.
        let root = self.relative_uri("/")?;
        self.find_href_prop_as_uri(root, "<current-user-principal/>", &property_data)
            .await
            .map_err(FindCurrentUserPrincipalError::RequestError)

        // NOTE: If no principal is resolved, it needs to be provided interactively
        //       by the user. We use `base_url` as a fallback.
    }

    /// Internal helper to find an `href` property
    ///
    /// Very specific, but de-duplicates a few identical methods.
    pub(crate) async fn find_href_prop_as_uri(
        &self,
        url: Uri,
        prop: &str,
        prop_type: &SimplePropertyMeta,
    ) -> Result<Option<Uri>, DavError> {
        let maybe_href = match self
            .propfind::<ResponseWithProp<HrefProperty>>(url.clone(), prop, 0, prop_type)
            .await?
            .pop()
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
    /// This is a shortcut for simple `PROPFIND` requests.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
    pub async fn propfind<T: FromXml>(
        &self,
        url: Uri,
        prop: &str,
        depth: u8,
        data: &T::Data,
    ) -> Result<Vec<T>, DavError> {
        let request = self
            .request_builder()?
            .method(Method::from_bytes(b"PROPFIND").expect("API for HTTP methods is stupid"))
            .uri(url)
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", depth.to_string())
            .body(Body::from(format!(
                r#"
                <propfind xmlns="DAV:">
                    <prop>
                        {prop}
                    </prop>
                </propfind>
                "#
            )))?;

        self.request_multistatus(request, data)
            .await
            .map(|ms| ms.into_responses())
    }

    /// Send a request which expects a multistatus response and parse it as `T`.
    ///
    /// # Errors
    ///
    /// - If a network error occurs executing the underlying HTTP request.
    /// - If the server returns an error status code.
    /// - If the response is not a valid XML document.
    /// - If the response's XML schema does not match the expected type.
    pub async fn request_multistatus<T: FromXml>(
        &self,
        request: Request<Body>,
        data: &T::Data,
    ) -> Result<Multistatus<T>, DavError> {
        let response = self.http_client.request(request).await?;
        let (head, body) = response.into_parts();
        check_status(head.status)?;

        let body = hyper::body::to_bytes(body).await?;
        xml::parse_multistatus::<T>(&body, data).map_err(DavError::from)
    }

    /// Returns the `displayname` for the collection at path `href`.
    ///
    /// From [rfc3744#section-4](https://www.rfc-editor.org/rfc/rfc3744#section-4):
    ///
    /// > A principal MUST have a non-empty DAV:displayname property
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
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
            &property_data,
        )
        .await?
        .pop()
        .ok_or(xml::Error::MissingData("displayname"))
        .map(Option::<String>::from)
        .map_err(DavError::from)
    }

    /// Resolve the default context path using a well-known path.
    ///
    /// This only applies for servers supporting webdav extensions like caldav or carddav.
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
    pub async fn find_context_path(
        &self,
        service: DiscoverableService,
        host: &str,
        port: u16,
    ) -> Result<Option<Uri>, ResolveContextPathError> {
        let uri = Uri::builder()
            .scheme(service.scheme())
            .authority(format!("{host}:{port}"))
            .path_and_query(service.well_known_path())
            .build()?;

        let request = self
            .request_builder()?
            .method(Method::GET)
            .uri(uri)
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
            .ok_or(ResolveContextPathError::MissingLocation)?
            .as_bytes();
        let uri = if location.starts_with(b"/") {
            self.relative_uri(std::str::from_utf8(location)?)?
        } else {
            Uri::try_from(location)?
        };
        Ok(Some(uri))
    }

    /// Enumerates resources in a collection
    ///
    /// Returns an array of results. Because the server can return an error status for individual
    /// resources, some of them may be `Err`, while other are `Ok(ItemDetails)`.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
    pub async fn list_resources(
        &self,
        collection_href: &str,
    ) -> Result<Vec<ResponseWithProp<ItemDetails>>, DavError> {
        // TODO: replace the return type for this with something more API-friendly.
        let url = self.relative_uri(collection_href)?;

        self.propfind::<ResponseWithProp<ItemDetails>>(
            url,
            "<resourcetype/><getcontenttype/><getetag/>",
            1,
            &(),
        )
        .await
        .map(|mut vec| {
            vec.retain(|r| r.href != collection_href);
            vec
        })
    }

    /// Inner helper with common logic between `create` and `update`.
    async fn put<Href, Etag>(
        &self,
        href: Href,
        data: Vec<u8>,
        etag: Option<Etag>,
    ) -> Result<Option<Vec<u8>>, DavError>
    where
        Href: AsRef<str>,
        Etag: AsRef<[u8]>,
    {
        let mut builder = self
            .request_builder()?
            .method(Method::PUT)
            .uri(self.relative_uri(href.as_ref())?)
            .header("Content-Type", "application/xml; charset=utf-8");

        builder = match etag {
            Some(etag) => builder.header("If-Match", etag.as_ref()),
            None => builder.header("If-None-Match", "*"),
        };

        let request = builder.body(Body::from(data))?;

        let response = self.http_client.request(request).await?;
        let (head, _body) = response.into_parts();
        check_status(head.status)?;

        // TODO: check multi-response

        let new_etag = head.headers.get("etag").map(|e| e.as_bytes().to_vec());
        Ok(new_etag)
    }

    /// Creates a new resource
    ///
    /// Returns an `Etag` if present in the server's response.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
    pub async fn create_resource<Href>(
        &self,
        href: Href,
        data: Vec<u8>,
    ) -> Result<Option<Vec<u8>>, CreateResourceError>
    where
        Href: AsRef<str>,
    {
        self.put(href, data, Option::<Vec<u8>>::None)
            .await
            .map_err(CreateResourceError)
    }

    /// Updates an existing resource
    ///
    /// Returns an `Etag` if present in the server's response.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
    pub async fn update_resource<Href, Etag>(
        &self,
        href: Href,
        data: Vec<u8>,
        etag: Etag,
    ) -> Result<Option<Vec<u8>>, UpdateResourceError>
    where
        Href: AsRef<str>,
        Etag: AsRef<[u8]>,
    {
        self.put(href, data, Some(etag))
            .await
            .map_err(UpdateResourceError)
    }

    /// Creates a collection under path `href`.
    ///
    /// # Caveats
    ///
    /// Because servers commonly don't return an Etag for this operation, it needs to be fetched in
    /// a separate operation.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
    pub async fn create_collection<Href: AsRef<str>>(
        &self,
        href: Href,
        resourcetype: CollectionType,
    ) -> Result<(), CreateCollectionError> {
        let body = format!(
            r#"
            <mkcol xmlns="DAV:">
                <set>
                    <prop>
                        <resourcetype>
                            <collection/>
                            {resourcetype}
                        </resourcetype>
                    </prop>
                </set>
            </mkcol>"#
        );

        let request = self
            .request_builder()?
            .method(Method::from_bytes(b"MKCOL").expect("API for HTTP methods is dumb"))
            .uri(self.relative_uri(href.as_ref())?)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(Body::from(body))?;

        let response = self.http_client.request(request).await?;
        let (head, _body) = response.into_parts();
        // TODO: we should check the response body here, but some servers (e.g.: Fastmail) return an empty body.
        check_status(head.status)?;

        Ok(())
    }

    /// Deletes a resource or collection at `href`.
    ///
    /// Because the implementation for deleting resources and collections is identical,
    /// this same method covers both cases.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
    pub async fn delete<Href, Etag>(&self, href: Href, etag: Etag) -> Result<(), DeleteError>
    where
        Href: AsRef<str>,
        Etag: AsRef<[u8]>,
    {
        let request = self
            .request_builder()?
            .method(Method::DELETE)
            .uri(self.relative_uri(href.as_ref())?)
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("If-Match", etag.as_ref())
            .body(Body::empty())?;

        let response = self.http_client.request(request).await?;
        let (head, _body) = response.into_parts();
        check_status(head.status)?;

        Ok(())
    }
}

/// Known types of collections.
pub enum CollectionType {
    /// A generic webdav collection.
    Collection,
    /// A caldav calendar collection.
    Calendar,
    /// A carddav addressbook collection.
    AddressBook,
}

impl std::fmt::Display for CollectionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CollectionType::Collection => write!(f, "<collection xmlns=\"DAV:\" />"),
            CollectionType::Calendar => write!(f, "<calendar xmlns=\"{CALDAV_STR}\" />"),
            CollectionType::AddressBook => write!(f, "<addressbook xmlns=\"{CARDDAV_STR}\" />"),
        }
    }
}

#[inline]
pub(crate) fn check_status(status: StatusCode) -> Result<(), DavError> {
    if !status.is_success() {
        Err(DavError::BadStatusCode(status))
    } else {
        Ok(())
    }
}

macro_rules! decl_error {
    ($($ident:ident, $msg:expr)*) => ($(
        #[derive(thiserror::Error, Debug)]
        #[error("$msg: {0}")]
        pub struct $ident (pub DavError);

        impl<T> From<T> for $ident
        where
            DavError: std::convert::From<T>,
        {
            fn from(value: T) -> Self {
                $ident(DavError::from(value))
            }
        }
    )*)
}

decl_error!(CreateResourceError, "error creating resources");
decl_error!(UpdateResourceError, "error updating resources");
decl_error!(GetResourceError, "error updating resources");
decl_error!(CreateCollectionError, "error creating collection");
decl_error!(DeleteError, "error deleting collection");
