use dav::{CalendarHomeSetProp, DavError, DisplayNameProp};
use reqwest::{Client, IntoUrl, Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use url::Url;

use crate::dav::{CurrentUserPrincipalProp, Multistatus, ResourceTypeProp};

pub mod dav;

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

impl CalDavClient {
    /// Returns a client without any automatic bootstrapping.
    ///
    /// It is generally advised to use [`bootstrapped`].
    pub fn raw_client(base_url: Url, auth: Auth) -> Self {
        Self {
            base_url,
            auth,
            client: Client::new(),
            context_path: None,
            principal: None,
            calendar_home_set: None,
        }
    }

    // TODO: using only a `mailto:` as input would be compliant:
    //       See: https://www.rfc-editor.org/rfc/rfc6764#section-6
    //       The Url crate seems to support this.

    /// Returns a bootstrapped client.
    pub async fn bootstrapped(base_url: Url, auth: Auth) -> Result<Self, DavError> {
        let mut client = Self::raw_client(base_url, auth);
        client.bootstrap().await?;
        Ok(client)
    }

    // TODO: methods to serialise and deserialise (mostly to cache all the data).

    /// Returns a request with the proper `Authorization` header set.
    fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        let request = self.client.request(method, url);
        match &self.auth {
            Auth::None => request,
            Auth::Basic { username, password } => request.basic_auth(username, password.as_ref()),
        }
    }

    /// Bootstrap this client
    ///
    /// Determines the real URL and path of the CalDav resources for a server.
    ///
    /// See: https://www.rfc-editor.org/rfc/rfc6764
    pub async fn bootstrap(&mut self) -> Result<(), DavError> {
        // TODO: check DNS-SD service labels: https://www.rfc-editor.org/rfc/rfc6764#section-3 (_caldavs._tcp)
        // TODO: check DNS-SD service labels: https://www.rfc-editor.org/rfc/rfc6764#section-3 (_caldav._tcp)
        // TODO: check TXT records: https://www.rfc-editor.org/rfc/rfc6764#section-4

        // NOTE: context path may have been defined via TXT record!
        //       (though if it errors, we should still resolve via well-known.
        self.context_path = Some(self.resolve_context_path().await?);

        // From https://www.rfc-editor.org/rfc/rfc6764#section-6, subsection 5:
        // > clients MUST properly handle HTTP redirect responses for the request
        self.principal = self.resolve_current_user_principal().await?;

        // NOTE: If obtaining a principal fails, we should query the user.
        //       We assume here that the provided `base_url` is exactly that.
        if let Some(principal) = &self.principal {
            self.calendar_home_set = self.query_calendar_home_set(principal.clone()).await?;
        } else {
            self.calendar_home_set = self.query_calendar_home_set(self.base_url.clone()).await?;
        }

        // TODO: use the user principal url to exec PROPFIND and discover calendars.

        Ok(())
    }

    /// Resolve the default context path with the well-known URL.
    ///
    /// See: https://www.rfc-editor.org/rfc/rfc6764#section-5
    pub async fn resolve_context_path(&self) -> Result<Url, reqwest::Error> {
        let mut url = self.base_url.clone();
        url.set_path("/.well-known/caldav");

        // From https://www.rfc-editor.org/rfc/rfc6764#section-5:
        // > [...] the server MAY require authentication when a client tries to
        // > access the ".well-known" URI
        self.request(Method::GET, url)
            .send()
            .await
            .map(|resp| resp.url().to_owned())
        // FIXME: if the URL is the same, we have no context path
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
