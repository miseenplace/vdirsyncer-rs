use std::io;

// A lot of the code is derived from cardamom:
// https://github.com/soywod/cardamom/
//
// Co-Authored-By: Clément DOUIN <clement.douin@posteo.net>
use serde::Deserialize;

/// A multi-status response (webdav-specific)
///
/// ```xml
/// <multistatus xmlns="DAV:">
///     <response>
///         ...
///     </response>
///     <response>
///         ...
///     </response>
///     ...
/// </multistatus>
/// ```
///
/// See: https://www.rfc-editor.org/rfc/rfc2518#section-11
#[derive(Debug, Deserialize)]
pub struct Multistatus<T> {
    #[serde(rename = "response", default)]
    pub responses: Vec<Response<T>>,
}

/// A generic error for `WebDav` operations.
#[derive(thiserror::Error, Debug)]
pub enum DavError {
    #[error("http error executing request")]
    Network(#[from] reqwest::Error),

    #[error("cannot parse dav response")]
    Data(#[from] quick_xml::de::DeError),

    #[error("failure parsing XML response")]
    Xml(#[from] crate::xml::Error),

    #[error("failed to parse a URL returned by the server")]
    BadUrl(#[from] url::ParseError),
}

impl From<DavError> for io::Error {
    fn from(value: DavError) -> Self {
        match value {
            DavError::Network(_) => io::Error::new(io::ErrorKind::Other, value),
            DavError::Data(_) => io::Error::new(io::ErrorKind::InvalidData, value),
            DavError::BadUrl(_) => io::Error::new(io::ErrorKind::InvalidInput, value),
            DavError::Xml(e) => io::Error::new(io::ErrorKind::InvalidData, e),
        }
    }
}

impl<T: for<'a> Deserialize<'a> + Default> Multistatus<T> {
    /// Convert an HTTP response into a [`Multistatus`].
    ///
    /// # Errors
    ///
    /// - If streaming the response fails.
    /// - If the data fails to deserialise as XML.
    pub(crate) async fn from_response(response: reqwest::Response) -> Result<Self, DavError> {
        let response = response.error_for_status()?;
        // TODO: Do we also error for non-207?
        let body = response.text().await?;
        let multi_status = quick_xml::de::from_str::<Multistatus<T>>(&body)?;

        Ok(multi_status)
    }
}

/// A single response from a [`Multistatus`].
///
/// Each response contains a `href` and multiple `propstat`.
///
/// ```xml
/// <response>
///     <href>/path</href>
///     <propstat>
///         ...
///     </propstat>
///     <propstat>
///         ...
///     </propstat>
///     ...
/// <response>
/// ```
#[derive(Debug, Deserialize)]
pub struct Response<T> {
    pub href: String,
    #[serde(default)]
    pub propstat: Vec<Propstat<T>>,
}

/// A single properties associated to the WebDAV response. The propstat contains
/// a property `prop` and sometimes a `status` code.
///
/// ```xml
/// <propstat>
///     <prop>
///         ...
///     </prop>
///     <status>HTTP/1.1 200 OK</status>
/// </propstat>
/// ```
#[derive(Debug, Deserialize)]
pub struct Propstat<T> {
    pub prop: T,
    // From https://www.rfc-editor.org/rfc/rfc2518#section-11:
    // > contains a set of XML elements called response which contain 200, 300, 400, and 500 series
    // > status codes generated during the method invocation
    // TODO: Should probably not use string here.
    pub status: Option<String>,
}

/// The current user's principal.
///
/// See: https://www.rfc-editor.org/rfc/rfc5397
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CurrentUserPrincipalProp {
    pub current_user_principal: CurrentUserPrincipal,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CurrentUserPrincipal {
    pub href: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CalendarHomeSetProp {
    pub calendar_home_set: CalendarHomeSet,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CalendarHomeSet {
    pub href: String,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ResourceTypeProp {
    pub resourcetype: ResourceType,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ResourceType {
    // Unused (for now?):
    // pub collection: Option<Collection>,

    // FIXME: this doesn't care about the namespace
    // E.g.: assumes xmlns:C="urn:ietf:params:xml:ns:caldav"
    pub calendar: Option<Calendar>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Collection {}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Calendar {}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DisplayNameProp {
    pub displayname: String,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ColourProp {
    #[serde(rename = "calendar-color")]
    pub color: String,
}

#[cfg(test)]
mod tests {
    use quick_xml::de as xml;

    use super::*;

    #[test]
    fn empty_response() {
        let res: Multistatus<String> = xml::from_str(r#"<multistatus xmlns="DAV:" />"#).unwrap();
        assert_eq!(0, res.responses.len());
    }

    #[test]
    fn single_propstat() {
        let res: Multistatus<String> = xml::from_str(
            r#"
            <multistatus xmlns="DAV:">
                <response>
                    <href>/path</href>
                    <propstat>
                        <prop>data</prop>
                        <status>HTTP/1.1 200 OK</status>
                    </propstat>
                </response>
            </multistatus>
            "#,
        )
        .unwrap();

        assert_eq!(1, res.responses.len());
        assert_eq!("/path", res.responses[0].href);
        assert_eq!(1, res.responses[0].propstat.len());
        assert_eq!("data", res.responses[0].propstat[0].prop);
        assert_eq!(
            Some("HTTP/1.1 200 OK"),
            res.responses[0].propstat[0]
                .status
                .as_ref()
                .map(|s| s.as_ref())
        );
    }

    #[test]
    fn multiple_propstats() {
        #[derive(Debug, Default, Deserialize)]
        struct Response {
            getetag: Option<String>,
            getlastmodified: Option<String>,
        }

        let res: Multistatus<Response> = xml::from_str(
            r#"
            <multistatus xmlns="DAV:">
                <response>
                    <href>/path</href>
                    <propstat>
                        <prop>
                            <getetag>etag</getetag>
                        </prop>
                        <status>HTTP/1.1 200 OK</status>
                    </propstat>
                    <propstat>
                        <prop>
                            <getlastmodified />
                        </prop>
                        <status>HTTP/1.1 404 Not Found</status>
                    </propstat>
                </response>
            </multistatus>
            "#,
        )
        .unwrap();

        assert_eq!(1, res.responses.len());
        assert_eq!("/path", res.responses[0].href);
        assert_eq!(2, res.responses[0].propstat.len());
        assert_eq!(
            Some("etag"),
            res.responses[0].propstat[0]
                .prop
                .getetag
                .as_ref()
                .map(|etag| etag.as_ref())
        );
        assert_eq!(
            Some("HTTP/1.1 200 OK"),
            res.responses[0].propstat[0]
                .status
                .as_ref()
                .map(|v| v.as_ref())
        );
        assert_eq!(
            Some(""),
            res.responses[0].propstat[1]
                .prop
                .getlastmodified
                .as_ref()
                .map(|v| v.as_ref())
        );
        assert_eq!(
            Some("HTTP/1.1 404 Not Found"),
            res.responses[0].propstat[1]
                .status
                .as_ref()
                .map(|s| s.as_ref())
        );
    }
}
