use dav::{CalendarHomeSetProp, ColourProp, DavError, DisplayNameProp};
use dns::{find_context_path_via_txt_records, resolve_srv_record, TxtError};
use domain::base::Dname;
use reqwest::{Client, IntoUrl, Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use url::{ParseError, Url};

use crate::dav::{CurrentUserPrincipalProp, Multistatus, ResourceTypeProp};

pub mod dav;
pub mod dns;

#[non_exhaustive]
#[derive(Debug)]
pub enum Auth {
    None,
    Basic {
        username: String,
        password: Option<String>,
    },
}

// TODO FIXME: Need to figure out how to reuse as much as possible for carddav and caldav.
#[derive(Debug)]
pub struct CalDavClient {
    base_url: Url,
    auth: Auth,
    client: Client,
    /// URL to be used for all requests. This may be (due to bootstrapping discovery)
    /// a path than the one provided as `base_url`.
    /// See: https://www.rfc-editor.org/rfc/rfc6764#section-1
    pub context_path: Option<Url>,
    /// URL to a principal resource corresponding to the currently authenticated user.
    /// See: https://www.rfc-editor.org/rfc/rfc5397#section-3
    pub principal: Option<Url>,
    /// URL of collections that are either calendar collections or ordinary collections
    /// that have child or descendant calendar collections owned by the principal.
    /// See: https://www.rfc-editor.org/rfc/rfc4791#section-6.2.1
    pub calendar_home_set: Option<Url>, // TODO: timeouts
}

#[derive(thiserror::Error, Debug)]
pub enum BootstrapError {
    #[error("the input URL is not valid")]
    InvalidUrl,

    // FIXME: See: https://github.com/NLnetLabs/domain/pull/183
    #[error("error resolving DNS SRV records")]
    DnsError,

    #[error("SRV records returned domain/port pair that failed to parse")]
    BadSrv(#[from] ParseError),

    #[error("error resolving context path via TXT records")]
    TxtError(#[from] TxtError),

    #[error(transparent)]
    DavError(#[from] DavError),
}

// TODO: Minimal input from a user would consist of a calendar user address and a password.  A
// calendar user address is defined by iCalendar [RFC5545] to be a URI [RFC3986].
// https://www.rfc-editor.org/rfc/rfc6764#section-6
impl CalDavClient {
    /// Returns a client without any automatic bootstrapping.
    ///
    /// It is generally advised to use [`auto_bootstrap`] instead.
    pub fn raw_client(base_url: Url, auth: Auth) -> Self {
        // TODO: check that the URL is http or https (or mailto:?).
        Self {
            base_url,
            auth,
            client: Client::new(),
            context_path: None,
            principal: None,
            calendar_home_set: None,
        }
    }

    // TODO: methods to serialise and deserialise (mostly to cache all discovery data).

    /// Returns a request builder with the proper `Authorization` header set.
    fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        let request = self.client.request(method, url);
        match &self.auth {
            Auth::None => request,
            Auth::Basic { username, password } => request.basic_auth(username, password.as_ref()),
        }
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
    pub async fn auto_bootstrap(base_url: Url, auth: Auth) -> Result<Self, BootstrapError> {
        let mut client = Self::raw_client(base_url, auth);

        let domain = client.base_url.domain().ok_or(BootstrapError::InvalidUrl)?;
        let port = client.base_url.port_or_known_default().unwrap_or(443);

        let dname = Dname::bytes_from_str(domain).map_err(|_| BootstrapError::InvalidUrl)?;
        let mut candidates = resolve_srv_record(dname, port)
            .await
            .map_err(|_| BootstrapError::DnsError)?;

        // If none of the SRV candidates work, try the domain/port in the provided URI.
        candidates.push((domain.to_string(), port));

        if let Some(path) = find_context_path_via_txt_records(domain).await? {
            let mut ctx_path_url = client.base_url.clone();
            ctx_path_url.set_path(&path);
            // TODO: validate that the path works?
            client.context_path = Some(ctx_path_url);
        };

        if client.context_path.is_none() {
            for candidate in candidates {
                let url = Url::parse(&format!("https://{}:{}", candidate.0, candidate.1))?;

                if let Ok(Some(path)) = client.resolve_context_path(Some(url)).await {
                    client.context_path = Some(path);
                    break;
                }
            }
        }

        // From https://www.rfc-editor.org/rfc/rfc6764#section-6, subsection 5:
        // > clients MUST properly handle HTTP redirect responses for the request
        client.principal = client.resolve_current_user_principal().await?;

        // NOTE: If obtaining a principal fails, the specification says we should query the user.
        //       We assume here that the provided `base_url` is exactly that.
        let principal_url = client.principal.as_ref().unwrap_or(&client.base_url).clone();
        client.calendar_home_set = client.query_calendar_home_set(principal_url).await?;

        Ok(client)
    }

    /// Resolve the default context path with the well-known URL.
    ///
    /// See: https://www.rfc-editor.org/rfc/rfc6764#section-5
    pub async fn resolve_context_path(
        &self,
        url: Option<Url>,
    ) -> Result<Option<Url>, reqwest::Error> {
        let mut url = url.unwrap_or(self.base_url.clone());
        url.set_path("/.well-known/caldav");

        // From https://www.rfc-editor.org/rfc/rfc6764#section-5:
        // > [...] the server MAY require authentication when a client tries to
        // > access the ".well-known" URI
        let final_url = self
            .request(Method::GET, url)
            .send()
            .await
            .map(|resp| resp.url().to_owned())?;

        // If the response was a redirection, then we treat that as context path.
        if final_url != self.base_url {
            Ok(Some(final_url))
        } else {
            Ok(None)
        }
        // TODO: Should actually check that we've gotten 301, 303 or 307.
        //       The main issue is that reqwest does not allow changing this on a per-request basis.
        //       Maybe hyper is more adequate here?
    }

    /// Resolves the current user's principal resource.
    ///
    /// See: https://www.rfc-editor.org/rfc/rfc5397
    pub async fn resolve_current_user_principal(&self) -> Result<Option<Url>, DavError> {
        // Try querying the provided base url...
        if let Some(context_path) = &self.context_path {
            let maybe_principal = self
                .query_current_user_principal(context_path.clone())
                .await;

            match maybe_principal {
                Err(DavError::Network(err)) if err.status() == Some(StatusCode::NOT_FOUND) => {}
                Err(err) => return Err(err),
                Ok(Some(p)) => return Ok(Some(p)),
                Ok(None) => {}
            };
        }

        // ... Otherwise, try querying the root path.
        let mut root = self.base_url.clone();
        root.set_path("/");
        self.query_current_user_principal(root).await // Hint: This can be Ok(None)

        // NOTE: If not principal is resolved, it needs to be provided interactively
        //       by the user. We should use base_url in our case maybe...?
    }

    async fn query_current_user_principal(&self, url: Url) -> Result<Option<Url>, DavError> {
        self.propfind::<CurrentUserPrincipalProp>(url.clone(), "<current-user-principal />", 0)
            .await?
            .responses
            .first()
            .and_then(|res| res.propstat.first())
            .map(|propstat| propstat.prop.current_user_principal.href.to_owned())
            .map(|principal| url.join(principal.as_ref()).map_err(DavError::BadUrl))
            .transpose()
    }

    async fn query_calendar_home_set(&self, url: Url) -> Result<Option<Url>, DavError> {
        self.propfind::<CalendarHomeSetProp>(
            url.clone(),
            "<calendar-home-set xmlns=\"urn:ietf:params:xml:ns:caldav\"/>",
            0,
        )
        .await?
        .responses
        .first()
        .and_then(|res| res.propstat.first())
        .map(|propstat| propstat.prop.calendar_home_set.href.to_owned())
        .map(|principal| url.join(principal.as_ref()).map_err(DavError::BadUrl))
        .transpose()
    }

    // FIXME: other APIs here return a URL, but this just returns a relative path.
    //        need to figure out which is best. Probably returning the String, since joining the
    //        URL might be work that's not needed.
    // TODO: Make argument Option<Url>. If none is passed, use the `calendar_home_set`.
    /// Find calendars collections under the given `url`.
    ///
    /// Generally, this method should be called with this collection's `calendar_home_set`
    /// to find the current user's calendars..
    pub async fn find_calendars(&self, url: Url) -> Result<Vec<String>, DavError> {
        self.propfind::<ResourceTypeProp>(url.clone(), "<resourcetype />", 1)
            .await
            .map(|multi_response| {
                multi_response
                    .responses
                    .into_iter()
                    // TODO: I'm ignoring the status code.
                    .filter(|response| {
                        response
                            .propstat
                            .first()
                            .map(|propstat| propstat.prop.resourcetype.calendar.is_some())
                            .unwrap_or(false) // Should not actually happen.
                    })
                    .map(|response| response.href)
                    .collect()
            })
    }

    pub async fn get_calendar_displayname(&self, url: Url) -> Result<Option<String>, DavError> {
        self.propfind::<DisplayNameProp>(url.clone(), "<displayname/>", 0)
            .await
            .map(|multi_response| {
                multi_response
                    .responses
                    .first()
                    .and_then(|res| res.propstat.first())
                    .map(|propstat| propstat.prop.displayname.to_owned())
            })
    }

    /// Returns the colour for a calendar
    ///
    /// This is not a formally standardised property, but is relatively widespread.
    ///
    /// # Errors
    ///
    /// If the network request fails, or if the response cannot be parsed.
    pub async fn get_calendar_colour(&self, url: Url) -> Result<Option<String>, DavError> {
        self.propfind::<ColourProp>(url.clone(), "<calendar-color xmlns=\"http://apple.com/ns/ical/\"/>", 0)
            .await
            .map(|multi_response| {
                multi_response
                    .responses
                    .first()
                    .and_then(|res| res.propstat.first())
                    .map(|propstat| propstat.prop.color.to_owned())
            })
    }

    // TODO: get_calendar_description ("calendar-description", "urn:ietf:params:xml:ns:caldav")
    // TODO: get_calendar_order ("calendar-order", "http://apple.com/ns/ical/")
    // TODO: DRY: the above methods are super repetitive.
    //       Maybe all these props impl a single trait, so the API could be `get_prop<T>(url)`?

    /// Sends a `PROPFIND` request and parses the result.
    async fn propfind<T: for<'a> Deserialize<'a> + Default>(
        &self,
        url: Url,
        prop: &str,
        depth: u8,
    ) -> Result<Multistatus<T>, DavError> {
        let response = self
            .request(
                Method::from_bytes(b"PROPFIND").expect("API for HTTP methods is stupid"),
                url,
            )
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", format!("{depth}"))
            .body(format!(
                r#"
                <propfind xmlns="DAV:">
                    <prop>
                        {prop}
                    </prop>
                </propfind>
                "#
            ))
            .send()
            .await?;

        Multistatus::<T>::from_response(response).await
    }
}
