//! Generic webdav implementation.
//!
//! This mostly implements the necessary bits for the caldav and carddav implementations. It should
//! not be considered a general purpose webdav implementation.
use std::{iter::once, string::FromUtf8Error};

use http::{response::Parts, Method, Request, StatusCode, Uri};
use hyper::{body::Bytes, client::HttpConnector, Body, Client};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};

use crate::{
    auth::AuthExt,
    dns::DiscoverableService,
    xml::{
        self, HrefParentParser, ItemDetails, ItemDetailsParser, Multistatus,
        MultistatusDocumentParser, Parser, PropParser, ReportPropParser, Response, ResponseParser,
        ResponseVariant, SelfClosingPropertyNode, TextNodeParser, CALDAV_STR, CARDDAV_STR, DAV,
    },
    Auth, AuthError, FetchedResource, FetchedResourceContent,
};

/// A generic error for WebDav operations.
#[derive(thiserror::Error, Debug)]
pub enum DavError {
    #[error("http error executing request")]
    Network(#[from] hyper::Error),

    #[error("failure parsing XML response")]
    Xml(#[from] crate::xml::Error),

    #[error("http request returned {0}")]
    BadStatusCode(http::StatusCode),

    #[error("failed to build URL with the given input")]
    InvalidInput(#[from] http::Error),

    #[error("internal error with specified authentication")]
    Auth(#[from] crate::AuthError),

    #[error("the server returned an response with an invalid etag header")]
    InvalidEtag(#[from] FromUtf8Error),

    #[error("the server returned an invalid response")]
    InvalidResponse(Box<dyn std::error::Error + Send + Sync>),
}

impl From<StatusCode> for DavError {
    fn from(status: StatusCode) -> Self {
        DavError::BadStatusCode(status)
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
    #[error("error sending or parsing request")]
    RequestError(#[from] DavError),

    // XXX: This should not really happen, but the API for `http` won't let us validate this
    // earlier with a clear approach.
    #[error("cannot use base_url to build request uri")]
    InvalidInput(#[from] http::Error),
}

/// A generic webdav client.
#[derive(Debug, Clone)]
pub struct WebDavClient {
    /// Base URL to be used for all requests.
    pub(crate) base_url: Uri,
    auth: Auth,
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

impl WebDavClient {
    /// Builds a new webdav client.
    ///
    /// Only `https` is enabled by default. Plain-text `http` is only enabled if the
    /// input uri has a scheme of `http` or `caldav`.
    pub fn new(base_url: Uri, auth: Auth) -> WebDavClient {
        let builder = HttpsConnectorBuilder::new().with_native_roots();
        let builder = match base_url.scheme() {
            Some(scheme) if scheme.as_str() == "http" => builder.https_or_http(),
            Some(scheme) if scheme.as_str() == "caldav" => builder.https_or_http(),
            _ => builder.https_only(),
        };

        let https = builder.enable_http1().build();
        WebDavClient {
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
        parts.path_and_query = Some(path.try_into()?);
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
        let parser = PropParser {
            inner: &HrefParentParser {
                name: b"current-user-principal",
                namespace: xml::DAV,
            },
        };

        // Try querying the provided base url...
        let maybe_principal = self
            .find_href_prop_as_uri(&self.base_url, "<current-user-principal/>", &parser)
            .await;

        match maybe_principal {
            Err(DavError::BadStatusCode(StatusCode::NOT_FOUND)) | Ok(None) => {}
            Err(err) => return Err(FindCurrentUserPrincipalError::RequestError(err)),
            Ok(Some(p)) => return Ok(Some(p)),
        };

        // ... Otherwise, try querying the root path.
        let root = self.relative_uri("/")?;
        self.find_href_prop_as_uri(&root, "<current-user-principal/>", &parser)
            .await
            .map_err(FindCurrentUserPrincipalError::RequestError)

        // NOTE: If no principal is resolved, it needs to be provided interactively
        //       by the user. We use `base_url` as a fallback.
    }

    /// Internal helper to find an `href` property
    ///
    /// Very specific, but de-duplicates a few identical methods.
    pub(crate) async fn find_href_prop_as_uri<X: Parser<ParsedData = Option<String>>>(
        &self,
        url: &Uri,
        prop: &str,
        parser: &X,
    ) -> Result<Option<Uri>, DavError> {
        let maybe_href = match self.propfind(url, prop, 0, parser).await?.pop() {
            Some(prop) => prop.first_prop(),
            None => return Ok(None),
        };

        let Some(href) = maybe_href else { return Ok(None) };

        let path = href
            .try_into()
            .map_err(|e| DavError::InvalidResponse(Box::from(e)))?;

        let mut parts = url.clone().into_parts();
        parts.path_and_query = Some(path);
        Some(Uri::from_parts(parts))
            .transpose()
            .map_err(|e| DavError::InvalidResponse(Box::from(e)))
    }

    /// Sends a `PROPFIND` request and parses the result.
    ///
    /// This is a shortcut for simple `PROPFIND` requests.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
    pub async fn propfind<X: Parser>(
        &self,
        url: &Uri,
        prop: &str,
        depth: u8,
        parser: &X,
    ) -> Result<Vec<Response<X::ParsedData>>, DavError> {
        let request = self
            .request_builder()?
            .method(Method::from_bytes(b"PROPFIND").expect("API for HTTP methods is stupid"))
            .uri(url)
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", depth.to_string())
            .body(Body::from(format!(
                r#"<propfind xmlns="DAV:"><prop>{prop}</prop></propfind>"#
            )))?;

        let parser = ResponseParser(parser);
        self.request_multistatus(request, &parser)
            .await
            .map(Multistatus::into_responses)
    }

    /// Send a request which expects a multistatus response and parse it as `T`.
    ///
    /// # Errors
    ///
    /// - If a network error occurs executing the underlying HTTP request.
    /// - If the server returns an error status code.
    /// - If the response is not a valid XML document.
    /// - If the response's XML schema does not match the expected type.
    pub async fn request_multistatus<T: Parser>(
        &self,
        request: Request<Body>,
        parser: &T,
    ) -> Result<Multistatus<T::ParsedData>, DavError> {
        let (head, body) = self.request(request).await?;
        check_status(head.status)?;

        let parser = MultistatusDocumentParser(parser);
        xml::parse_xml(&body, &parser).map_err(DavError::from)
    }

    // Internal wrapper around `http_client.request` that logs all response bodies.
    pub(crate) async fn request(
        &self,
        request: Request<Body>,
    ) -> Result<(Parts, Bytes), hyper::Error> {
        // QUIRK: When trying to fetch a resource on a URL that is a collection, iCloud
        // will terminate the connection at this point (unexpected end of file).
        let response = self.http_client.request(request).await?;
        let (head, body) = response.into_parts();
        let body = hyper::body::to_bytes(body).await?;

        log::debug!("Response ({}): {:?}", head.status, body);
        Ok((head, body))
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

        let parser = PropParser {
            inner: &TextNodeParser {
                name: b"displayname",
                namespace: DAV,
            },
        };

        self.propfind(&url, "<displayname/>", 0, &parser)
            .await?
            .pop()
            .ok_or(xml::Error::MissingData("displayname"))
            .map(Response::first_prop)
            .map_err(DavError::from)
    }

    /// Sends a `PROPUPDATE` query to the server.
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    pub async fn propupdate(
        &self,
        url: &Uri,
        prop: &str,
        prop_ns: &str,
        value: Option<&str>,
    ) -> Result<(), DavError> {
        let (action, inner) = match value {
            Some(value) => {
                let escaped = quick_xml::escape::partial_escape(value);
                (
                    "set",
                    format!("<{prop} xmlns=\"{prop_ns}\">{escaped}</{prop}>"),
                )
            }
            None => ("remove", format!("<{prop} />")),
        };
        let request = self
            .request_builder()?
            .method(Method::from_bytes(b"PROPPATCH").expect("ugh"))
            .uri(url)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(Body::from(format!(
                r#"<propertyupdate xmlns="DAV:">
                <{action}>
                    <prop>
                        {inner}
                    </prop>
                </{action}>
            </propertyupdate>"#
            )))?;

        let (head, body) = self.request(request).await?;
        check_status(head.status)?;

        let parser = &SelfClosingPropertyNode {
            namespace: prop.as_bytes(),
            name: prop_ns.as_bytes(),
        };
        let parser = &ResponseParser(parser);
        let parser = MultistatusDocumentParser(parser);
        let response = xml::parse_xml(&body, &parser).map_err(DavError::from)?;

        // TODO: should Err if we got more than one response?
        let status = response
            .into_responses()
            .into_iter()
            .next()
            .ok_or(DavError::InvalidResponse(
                "Multistatus has no responses".into(),
            ))?
            .first_status()
            .ok_or(DavError::InvalidResponse(
                "Expected at least one status code in multiresponse".into(),
            ))?;

        check_status(status).map_err(DavError::BadStatusCode)
    }

    /// Sets the `displayname` for a collection
    ///
    /// The `displayname` string is expected not to be escaped.
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    pub async fn set_collection_displayname(
        &self,
        href: &str,
        displayname: Option<&str>,
    ) -> Result<(), DavError> {
        let url = self.relative_uri(href)?;
        self.propupdate(&url, "displayname", "DAV:", displayname)
            .await
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
        let response = self.http_client.request(request).await?;
        let (head, _body) = response.into_parts();
        log::debug!("Response finding context path: {}", head.status);

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
            Uri::builder()
                .scheme(service.scheme())
                .authority(format!("{host}:{port}"))
                .path_and_query(std::str::from_utf8(location)?)
                .build()?
        } else {
            Uri::try_from(location)?
        };
        Ok(Some(uri))
    }

    /// Enumerates resources in a collection
    ///
    /// Returns an array of results.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
    pub async fn list_resources(
        &self,
        collection_href: &str,
    ) -> Result<Vec<ListedResource>, DavError> {
        let url = self.relative_uri(collection_href)?;

        let items = self
            .propfind::<ItemDetailsParser>(
                &url,
                "<resourcetype/><getcontenttype/><getetag/>",
                1,
                &ItemDetailsParser,
            )
            .await?
            .into_iter()
            .filter(|r| r.href != collection_href);

        let mut result = Vec::new();
        for item in items {
            match item.variant {
                ResponseVariant::WithProps { mut propstats } => {
                    result.push(ListedResource {
                        details: propstats
                            .pop()
                            .ok_or(xml::Error::MissingData("props"))?
                            .prop,
                        href: item.href,
                    });
                }
                ResponseVariant::WithoutProps { .. } => {
                    // FIXME: this fails when a collection has nested collections. It should not.
                    return Err(DavError::Xml(xml::Error::MissingData("propstat")));
                }
            }
        }

        Ok(result)
    }

    /// Inner helper with common logic between `create` and `update`.
    async fn put<Href, Etag, MimeType>(
        &self,
        href: Href,
        data: Vec<u8>,
        etag: Option<Etag>,
        mime_type: MimeType,
    ) -> Result<Option<String>, DavError>
    where
        Href: AsRef<str>,
        Etag: AsRef<str>,
        MimeType: AsRef<[u8]>,
    {
        let mut builder = self
            .request_builder()?
            .method(Method::PUT)
            .uri(self.relative_uri(href.as_ref())?)
            .header("Content-Type", mime_type.as_ref());

        builder = match etag {
            Some(etag) => builder.header("If-Match", etag.as_ref()),
            None => builder.header("If-None-Match", "*"),
        };

        let request = builder.body(Body::from(data))?;

        let (head, _body) = self.request(request).await?;
        check_status(head.status)?;

        // TODO: check multi-response

        let new_etag = head
            .headers
            .get("etag")
            .map(|hv| String::from_utf8(hv.as_bytes().to_vec()))
            .transpose()?;
        Ok(new_etag)
    }

    /// Creates a new resource
    ///
    /// Returns an `Etag` if present in the server's response.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
    pub async fn create_resource<Href, MimeType>(
        &self,
        href: Href,
        data: Vec<u8>,
        mime_type: MimeType,
    ) -> Result<Option<String>, DavError>
    where
        Href: AsRef<str>,
        MimeType: AsRef<[u8]>,
    {
        self.put(href, data, Option::<String>::None, mime_type)
            .await
    }

    /// Updates an existing resource
    ///
    /// Returns an `Etag` if present in the server's response.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
    pub async fn update_resource<Href, Etag, MimeType>(
        &self,
        href: Href,
        data: Vec<u8>,
        etag: Etag,
        mime_type: MimeType,
    ) -> Result<Option<String>, DavError>
    where
        Href: AsRef<str>,
        Etag: AsRef<str>,
        MimeType: AsRef<[u8]>,
    {
        self.put(href, data, Some(etag), mime_type).await
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
    ) -> Result<(), DavError> {
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

        let (head, _body) = self.request(request).await?;
        // TODO: we should check the response body here, but some servers (e.g.: Fastmail) return an empty body.
        check_status(head.status)?;

        Ok(())
    }

    /// Deletes the resource at `href`.
    ///
    /// The resource MAY be a collection. Because the implementation for deleting resources and
    /// collections is identical, this same method covers both cases.
    ///
    /// If the Etag does not match (i.e.: if the resource has been altered), the operation will
    /// fail and return an Error.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
    // TODO: document WHICH error is returned on Etag mismatch.
    pub async fn delete<Href, Etag>(&self, href: Href, etag: Etag) -> Result<(), DavError>
    where
        Href: AsRef<str>,
        Etag: AsRef<str>,
    {
        let request = self
            .request_builder()?
            .method(Method::DELETE)
            .uri(self.relative_uri(href.as_ref())?)
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("If-Match", etag.as_ref())
            .body(Body::empty())?;

        let response = self.http_client.request(request).await?;
        let status = response.status();

        check_status(status).map_err(DavError::BadStatusCode)
    }

    /// Force deletion of the resource at `href`.
    ///
    /// This function cannot guarantee that a resource or collection has not been modified since
    /// it was last read. **Use this function with great care**.
    ///
    /// The resource MAY be a collection. Because the implementation for deleting resources and
    /// collections is identical, this same method covers both cases.
    ///
    /// # Errors
    ///
    /// See [`request_multistatus`](Self::request_multistatus).
    pub async fn force_delete<Href>(&self, href: Href) -> Result<(), DavError>
    where
        Href: AsRef<str>,
    {
        let request = self
            .request_builder()?
            .method(Method::DELETE)
            .uri(self.relative_uri(href.as_ref())?)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(Body::empty())?;

        let response = self.http_client.request(request).await?;
        let status = response.status();

        check_status(status).map_err(DavError::BadStatusCode)
    }

    pub(crate) async fn multi_get(
        &self,
        collection_href: &str,
        body: String,
        data: &ReportPropParser,
    ) -> Result<Vec<FetchedResource>, DavError> {
        let request = self
            .request_builder()?
            .method(Method::from_bytes(b"REPORT").expect("API for HTTP methods is dumb"))
            .uri(self.relative_uri(collection_href.as_ref())?)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(Body::from(body))?;

        let parser = ResponseParser(data);
        let responses = self
            .request_multistatus(request, &parser)
            .await?
            .into_responses();

        let mut items = Vec::new();
        for r in responses {
            match r.variant {
                ResponseVariant::WithProps { propstats } => {
                    let err_prop = propstats.iter().find(|p| !p.status.is_success());
                    let content = if let Some(prop) = err_prop {
                        Err(prop.status)
                    } else {
                        let mut data = None;
                        let mut etag = None;
                        for propstat in propstats {
                            if let Some(d) = propstat.prop.data {
                                data = Some(d);
                            }
                            if let Some(e) = propstat.prop.etag {
                                etag = Some(e);
                            }
                        }
                        // Missing `etag` or `data` with a non-error status is invalid.
                        // This may be an invalid response or a parser issue.
                        Ok(FetchedResourceContent {
                            data: data.ok_or(crate::xml::Error::MissingData("data"))?,
                            etag: etag.ok_or(crate::xml::Error::MissingData("etag"))?,
                        })
                    };

                    items.push(FetchedResource {
                        href: r.href,
                        content,
                    });
                }
                ResponseVariant::WithoutProps { hrefs, status } => {
                    for href in hrefs.into_iter().chain(once(r.href)) {
                        items.push(FetchedResource {
                            href,
                            content: Err(status),
                        });
                    }
                }
            };
        }

        Ok(items)
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
pub(crate) fn check_status(status: StatusCode) -> Result<(), StatusCode> {
    if status.is_success() {
        Ok(())
    } else {
        Err(status)
    }
}

pub mod mime_types {
    pub const CALENDAR: &[u8] = b"text/calendar";
    pub const ADDRESSBOOK: &[u8] = b"text/vcard";
}

/// Metadata for a resource.
///
/// This type is returned when listing resources. It contains metadata on
/// resources but no the resource data itself.
pub struct ListedResource {
    pub details: ItemDetails,
    pub href: String,
}

/// Metadata for a collection.
///
/// This type is returned when listing collections. It contains metadata on
/// collection itself, but not the entires themselves.
#[derive(Debug)]
pub struct FoundCollection {
    pub href: String,
    pub etag: Option<String>,
    pub supports_sync: bool,
    // TODO: query displayname by default too.
}
