//! Helpers for parsing XML responses returned by WebDav/CalDav/CardDav servers.
//!
//! These types are used internally by this crate and are generally reserved
//! **for advanced usage**.

use http::{status::InvalidStatusCode, StatusCode};
use log::{debug, warn};
use quick_xml::name::Namespace;
use quick_xml::{events::Event, name::ResolveResult, NsReader};
use std::str::FromStr;

/// Namespace for properties defined in webdav specifications.
///
/// See: <https://www.rfc-editor.org/rfc/rfc3744>
pub(crate) const DAV_STR: &str = "DAV:";
pub(crate) const CALDAV_STR: &str = "urn:ietf:params:xml:ns:caldav";
pub(crate) const CARDDAV_STR: &str = "urn:ietf:params:xml:ns:carddav";

pub(crate) const DAV: &[u8] = DAV_STR.as_bytes();
pub(crate) const CALDAV: &[u8] = CALDAV_STR.as_bytes();
pub(crate) const CARDDAV: &[u8] = CARDDAV_STR.as_bytes();

const NS_DAV: Namespace = Namespace(DAV);
const NS_CALDAV: Namespace = Namespace(CALDAV);
const NS_CARDDAV: Namespace = Namespace(CARDDAV);

/// An error parsing XML data.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("missing field in data")]
    MissingData(&'static str),

    #[error("invalid status code")]
    InvalidStatusCode(#[from] InvalidStatusCode),

    #[error(transparent)]
    Parser(#[from] quick_xml::Error),
}

/// Details of a single item that are returned when listing them.
///
/// This does not include actual item data, it only includes their metadata.
#[derive(Debug, PartialEq, Eq)]
pub struct ItemDetails {
    pub content_type: Option<String>,
    pub etag: Option<String>,
    pub resource_type: ResourceType,
    /// From: <https://www.rfc-editor.org/rfc/rfc6578>
    pub supports_sync: bool,
}

pub(crate) struct ItemDetailsParser;

impl Parser for ItemDetailsParser {
    type ParsedData = ItemDetails;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut item = ItemDetails {
            content_type: None,
            etag: None,
            resource_type: ResourceType::default(),
            supports_sync: false,
        };

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(NS_DAV), Event::End(element))
                    if element.local_name().as_ref() == b"prop" =>
                {
                    break;
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"resourcetype" =>
                {
                    item.resource_type = ResourceTypeParser.parse(reader)?;
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"getcontenttype" =>
                {
                    item.content_type = GetContentTypeParser.parse(reader)?;
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"getetag" =>
                {
                    item.etag = GetETagParser.parse(reader)?;
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"supported-report-set" =>
                {
                    item.supports_sync = SupportedReportSetParser.parse(reader)?.unwrap_or(false);
                }
                (ResolveResult::Bound(NS_DAV), Event::Empty(element))
                    if element.local_name().as_ref() == b"resourcetype" => {}
                (ResolveResult::Bound(NS_DAV), Event::Empty(element))
                    if element.local_name().as_ref() == b"getetag" =>
                {
                    warn!("missing etag in response");
                }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        Ok(item)
    }
}

/// Etag and contents of a single calendar resource.
///
/// This is a single prop from a `REPORT`.
#[derive(Debug, PartialEq, Eq)]
pub struct ReportProp {
    pub etag: Option<String>,
    pub data: Option<String>,
}

/// A custom named node.
///
/// This is, essentially, a `PropParser` which contains multiple inner nodes. The node name is
/// `DAV:prop`. This likely needs to be refactored and folded into [`PropParser`] somehow.
///
/// See: <https://www.rfc-editor.org/rfc/rfc4791#appendix-B>
pub struct ReportPropParser {
    pub namespace: &'static [u8],
    pub name: &'static [u8],
}

impl ReportPropParser {}

impl Parser for ReportPropParser {
    type ParsedData = ReportProp;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut etag = None;
        let mut data = None;

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(NS_DAV), Event::End(element))
                    if element.local_name().as_ref() == b"prop" =>
                {
                    break;
                }
                (ResolveResult::Bound(namespace), Event::Start(element))
                    if namespace.as_ref() == self.namespace
                        && element.local_name().as_ref() == self.name =>
                {
                    data = TextNodeParser {
                        namespace: self.namespace,
                        name: self.name,
                    }
                    .parse(reader)?;
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"getetag" =>
                {
                    etag = GetETagParser.parse(reader)?;
                }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            }
        }

        Ok(ReportProp { etag, data })
    }
}

/// A single response as defined in [rfc2518 section-12.9.1]
///
/// The inner type `T` will be parsed from the response's `prop` node.
/// Generally, this is used for responses to `PROPFIND` or `REPORT`.
///
///[rfc2518 section-12.9.1]: https://www.rfc-editor.org/rfc/rfc2518#section-12.9.1
#[derive(Debug, PartialEq, Eq)]
pub struct Response<T> {
    pub href: String,
    pub variant: ResponseVariant<T>,
    // TODO: responsedescription
}

/// One of the two variants for WebDav responses.
///
/// See [`Response`].
#[derive(Debug, PartialEq, Eq)]
pub enum ResponseVariant<T> {
    WithProps {
        propstats: Vec<PropStat<T>>,
    },
    WithoutProps {
        hrefs: Vec<String>,
        status: StatusCode,
    },
}

impl<T> Response<Option<T>> {
    /// Returns the first prop inside the response, if any.
    #[must_use]
    pub fn first_prop(self) -> Option<T> {
        if let ResponseVariant::WithProps { mut propstats } = self.variant {
            propstats.pop()?.prop
        } else {
            None
        }
    }
}

/// A `propstat` as defined in [rfc2518 section-12.9.1.1]
///
/// [rfc2518 section-12.9.1.1]: https://www.rfc-editor.org/rfc/rfc2518#section-12.9.1.1
#[derive(Debug, PartialEq, Eq)]
pub struct PropStat<T> {
    pub prop: T,
    pub status: StatusCode,
    // TODO: responsedescription
}

// See: https://www.rfc-editor.org/rfc/rfc2068#section-6.1
fn parse_statusline<S: AsRef<str>>(status_line: S) -> Result<StatusCode, InvalidStatusCode> {
    let mut iter = status_line.as_ref().splitn(3, ' ');
    iter.next();
    let code = iter.next().unwrap_or("");
    StatusCode::from_str(code)
}

/// Internal helper to build `ResponseVariant`.
enum ResponseVariantBuilder<T> {
    None,
    WithProps {
        propstats: Vec<PropStat<T>>,
    },
    WithoutProps {
        hrefs: Vec<String>,
        status: Option<StatusCode>,
    },
}

impl<T> ResponseVariantBuilder<T> {
    fn build(self) -> Result<ResponseVariant<T>, Error> {
        match self {
            ResponseVariantBuilder::None => Ok(ResponseVariant::WithProps {
                propstats: Vec::new(),
            }),
            ResponseVariantBuilder::WithProps { propstats } => {
                Ok(ResponseVariant::WithProps { propstats })
            }
            ResponseVariantBuilder::WithoutProps { hrefs, status } => {
                Ok(ResponseVariant::WithoutProps {
                    hrefs,
                    status: status.ok_or(Error::MissingData("status"))?,
                })
            }
        }
    }

    fn add_href(&mut self, href: String) -> Result<(), Error> {
        match self {
            ResponseVariantBuilder::None => {
                *self = ResponseVariantBuilder::WithoutProps {
                    hrefs: vec![href],
                    status: None,
                };
            }
            ResponseVariantBuilder::WithProps { .. } => {
                return Err(Error::Parser(quick_xml::Error::UnexpectedToken(
                    "href in response with props".to_string(),
                )))
            }
            ResponseVariantBuilder::WithoutProps { ref mut hrefs, .. } => {
                hrefs.push(href);
            }
        }

        Ok(())
    }
}

/// A `multistatus` response.
///
/// See: <https://www.rfc-editor.org/rfc/rfc2518#section-12.9>
#[derive(Debug)]
pub struct Multistatus<T> {
    pub responses: Vec<T>,
    // TODO: responsedescription
}

impl<T> Multistatus<T> {
    #[must_use]
    fn empty() -> Self {
        Self {
            responses: Vec::default(),
        }
    }

    #[must_use]
    #[inline]
    pub fn into_responses(self) -> Vec<T> {
        self.responses
    }
}

/// Parse an XML document using the given parser.
///
/// # Errors
///
/// If parsing the XML fails in any way or any necessary fields are missing.
pub(crate) fn parse_xml<X>(raw: &[u8], parser: &X) -> Result<X::ParsedData, Error>
where
    X: Parser,
{
    let mut reader = NsReader::from_reader(raw);
    reader.trim_text(true);
    parser.parse(&mut reader)
}

#[cfg(test)]
mod more_tests {

    use super::*;

    #[test]
    fn test_parse_list_result() {
        let raw = br#"
<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CS="http://calendarserver.org/ns/">
  <response>
    <href>/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/</href>
    <propstat>
      <prop>
        <resourcetype>
          <collection/>
          <C:calendar/>
        </resourcetype>
        <getcontenttype>text/calendar; charset=utf-8</getcontenttype>
        <getetag>"1591712486-1-1"</getetag>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
  <response>
    <href>/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/395b00a0-eebc-40fd-a98e-176a06367c82.ics</href>
    <propstat>
      <prop>
        <resourcetype/>
        <getcontenttype>text/calendar; charset=utf-8; component=VEVENT</getcontenttype>
        <getetag>"e7577ff2b0924fe8e9a91d3fb2eb9072598bf9fb"</getetag>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#;

        let parser = MultistatusDocumentParser(&ResponseParser(&ItemDetailsParser));
        let parsed = parse_xml(raw, &parser).unwrap().into_responses();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], Response {
            href: "/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/".to_string(),
            variant: ResponseVariant::WithProps {
                propstats: vec![
                    PropStat {
                        prop: ItemDetails {
                            content_type: Some("text/calendar; charset=utf-8".to_string()),
                            etag: Some("\"1591712486-1-1\"".to_string()),
                            resource_type: ResourceType {
                                is_collection: true,
                                is_calendar: true,
                                is_address_book: false,
                            },
                            supports_sync: false,
                        },
                        status: StatusCode::OK,
                    },
                ],
            }
        });
        assert_eq!(parsed[1], Response {
            href: "/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/395b00a0-eebc-40fd-a98e-176a06367c82.ics".to_string(),
            variant: ResponseVariant::WithProps {
                propstats: vec![
                    PropStat {
                        prop: ItemDetails {
                            content_type: Some("text/calendar; charset=utf-8; component=VEVENT".to_string()),
                            etag: Some("\"e7577ff2b0924fe8e9a91d3fb2eb9072598bf9fb\"".to_string()),
                            resource_type: ResourceType {
                                is_collection: false,
                                is_calendar: false,
                                is_address_book: false,
                            },
                            supports_sync: false,
                        },
                        status: StatusCode::OK,
                    },
                ],
            }
        });
    }

    #[test]
    fn test_multi_propstat() {
        let raw = br#"
<d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav">
    <d:response>
        <d:href>/remote.php/dav/calendars/vdirsyncer/1678996875/</d:href>
        <d:propstat>
            <d:prop>
                <d:resourcetype>
                    <d:collection/>
                    <cal:calendar/>
                </d:resourcetype>
            </d:prop>
            <d:status>HTTP/1.1 200 OK</d:status>
        </d:propstat>
        <d:propstat>
            <d:prop>
                <d:getetag/>
            </d:prop>
            <d:status>HTTP/1.1 404 Not Found</d:status>
        </d:propstat>
    </d:response>
</d:multistatus>"#;
        let parser = MultistatusDocumentParser(&ResponseParser(&ItemDetailsParser));
        let parsed = parse_xml(raw, &parser).unwrap().into_responses();
        assert_eq!(parsed.len(), 1);
        match &parsed.first().unwrap().variant {
            ResponseVariant::WithProps { propstats } => assert_eq!(propstats.len(), 2),
            ResponseVariant::WithoutProps { .. } => unreachable!(),
        };
    }

    #[test]
    fn test_multi_variants() {
        let raw = br#"<ns0:multistatus
	xmlns:ns0="DAV:"
	xmlns:ns1="urn:ietf:params:xml:ns:caldav">
	<ns0:response>
		<ns0:href>/user/calendars/Q208cKvMGjAdJFUw/qJJ9Li5DPJYr.ics</ns0:href>
		<ns0:propstat>
			<ns0:status>HTTP/1.1 200 OK</ns0:status>
			<ns0:prop>
				<ns0:getetag>"adb2da8d3cb1280a932ed8f8a2e8b4ecf66d6a02"</ns0:getetag>
				<ns1:calendar-data>CALENDAR-DATA-HERE</ns1:calendar-data>
			</ns0:prop>
		</ns0:propstat>
	</ns0:response>
	<ns0:response>
		<ns0:href>/user/calendars/Q208cKvMGjAdJFUw/rKbu4uUn.ics</ns0:href>
		<ns0:status>HTTP/1.1 404 Not Found</ns0:status>
	</ns0:response>
</ns0:multistatus>"#;
        let parser = MultistatusDocumentParser(&ResponseParser(&crate::caldav::CALENDAR_DATA));
        let parsed = parse_xml(raw, &parser).unwrap().into_responses();
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0],
            Response {
                href: "/user/calendars/Q208cKvMGjAdJFUw/qJJ9Li5DPJYr.ics".to_string(),
                variant: ResponseVariant::WithProps {
                    propstats: vec![PropStat {
                        prop: ReportProp {
                            etag: Some("\"adb2da8d3cb1280a932ed8f8a2e8b4ecf66d6a02\"".to_string()),
                            data: Some("CALENDAR-DATA-HERE".to_string())
                        },
                        status: StatusCode::OK
                    }],
                }
            }
        );
        assert_eq!(
            parsed[1],
            Response {
                href: "/user/calendars/Q208cKvMGjAdJFUw/rKbu4uUn.ics".to_string(),
                variant: ResponseVariant::WithoutProps {
                    hrefs: vec![],
                    status: StatusCode::NOT_FOUND,
                }
            }
        );
    }

    #[test]
    fn test_empty_response() {
        let raw = br#"<multistatus xmlns="DAV:" />"#;
        let parser = MultistatusDocumentParser(&ResponseParser(&ItemDetailsParser));
        let parsed = parse_xml(raw, &parser).unwrap().into_responses();
        assert_eq!(parsed.len(), 0);
    }

    #[test]
    fn test_single_propstat() {
        let raw = br#"
            <multistatus xmlns="DAV:">
                <response>
                    <href>/path</href>
                    <propstat>
                        <prop>
                            <displayname><![CDATA[test calendar]]></displayname>
                        </prop>
                        <status>HTTP/1.1 200 OK</status>
                    </propstat>
                </response>
            </multistatus>
            "#;
        let parser = MultistatusDocumentParser(&ResponseParser(&PropParser {
            inner: &TextNodeParser {
                name: b"displayname",
                namespace: DAV,
            },
        }));
        let parsed = parse_xml(raw, &parser).unwrap().into_responses();
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0],
            Response {
                href: "/path".to_string(),
                variant: ResponseVariant::WithProps {
                    propstats: vec![PropStat {
                        prop: Some("test calendar".to_string()),
                        status: StatusCode::OK,
                    }],
                },
            }
        );
    }
}

/// A helper that can parse a single XML node.
///
/// Implementations can have instance-specific attributes with specific details on extraction
/// (e.g.: for handling runtime-defined fields, etc).
pub trait Parser {
    type ParsedData;

    /// Parse a node with a given reader.
    ///
    /// If an error is not returned, then the reader is guaranteed to have read the corresponding
    /// end tag for the XML node.
    ///
    /// # Errors
    ///
    /// See [`Error`].
    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error>;
}

pub trait NamedNodeParser: Parser {
    fn name(&self) -> &[u8];

    fn namespace(&self) -> &[u8];
}

struct GetETagParser;

impl Parser for GetETagParser {
    type ParsedData = Option<String>;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut etag = None;

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(NS_DAV), Event::End(element))
                    if element.local_name().as_ref() == b"getetag" =>
                {
                    break;
                }
                (ResolveResult::Unbound, Event::Text(text)) => {
                    etag = Some(text.unescape()?.to_string());
                }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        Ok(etag)
    }
}

struct StatusParser;

impl Parser for StatusParser {
    type ParsedData = Option<StatusCode>;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut status = None;

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(NS_DAV), Event::End(element))
                    if element.local_name().as_ref() == b"status" =>
                {
                    break;
                }
                (ResolveResult::Unbound, Event::Text(text)) => {
                    status = Some(parse_statusline(text.unescape()?)?);
                }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        Ok(status)
    }
}

/// Parses a single [`PropStat`] node.
struct PropStatParser<'a, Prop: Parser> {
    /// A parser for the inner prop type.
    prop: &'a Prop,
}

impl<'a, P: Parser> Parser for PropStatParser<'a, P> {
    type ParsedData = PropStat<P::ParsedData>;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut status = None;
        let mut prop = None;

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(NS_DAV), Event::End(element))
                    if element.local_name().as_ref() == b"propstat" =>
                {
                    break;
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"status" =>
                {
                    status = StatusParser.parse(reader)?;
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"prop" =>
                {
                    prop = Some(self.prop.parse(reader)?);
                }
                (ResolveResult::Unbound, Event::Text(text)) => {
                    status = Some(parse_statusline(text.unescape()?)?);
                }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        Ok(PropStat {
            prop: prop.ok_or(Error::MissingData("prop"))?,
            status: status.ok_or(Error::MissingData("status"))?,
        })
    }
}

/// A named node with a single `href` child.
///
/// This example:
///
/// ```xml
/// <?xml version="1.0" encoding="utf-8"?>
/// <multistatus xmlns="DAV:">
///   <response>
///     <href>/dav/calendars</href>
///     <propstat>
///       <prop>
///         <current-user-principal>
///           <href>/dav/principals/user/vdirsyncer@example.com/</href>
///         </current-user-principal>
///       </prop>
///       <status>HTTP/1.1 200 OK</status>
///     </propstat>
///   </response>
/// </multistatus>
/// ```
///
/// Can be parsed with the following [`HrefParentParser`]:
///
/// ```rust
/// # use libdav::xml::{PropParser, HrefParentParser};;
/// let parser = PropParser {
///     inner: &HrefParentParser {
///         name: b"current-user-principal",
///         namespace: b"DAV:",
///     },
/// };
/// ```
pub struct HrefParentParser<'a> {
    pub namespace: &'a [u8],
    pub name: &'a [u8],
}

impl NamedNodeParser for HrefParentParser<'_> {
    #[inline]
    fn name(&self) -> &[u8] {
        self.name
    }

    #[inline]
    fn namespace(&self) -> &[u8] {
        self.namespace
    }
}

impl Parser for HrefParentParser<'_> {
    type ParsedData = Option<String>;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut value = None;

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(namespace), Event::End(element))
                    if namespace.as_ref() == self.namespace
                        && element.local_name().as_ref() == self.name =>
                {
                    break;
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"href" =>
                {
                    value = TextNodeParser::HREF_PARSER.parse(reader)?;
                }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        Ok(value)
    }
}

struct ReportParser;

impl Parser for ReportParser {
    type ParsedData = Option<bool>;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut supports_sync = None;

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(NS_DAV), Event::End(element))
                    if element.local_name().as_ref() == b"report" =>
                {
                    break;
                }
                (ResolveResult::Bound(NS_DAV), Event::Empty(element))
                    if element.local_name().as_ref() == b"sync-collection" =>
                {
                    supports_sync = Some(true);
                }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        Ok(supports_sync)
    }
}

struct SupportedReportSetParser;

impl Parser for SupportedReportSetParser {
    type ParsedData = Option<bool>;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut supports_sync = None;

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(NS_DAV), Event::End(element))
                    if element.local_name().as_ref() == b"supported-report-set" =>
                {
                    break;
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"report" =>
                {
                    supports_sync = ReportParser.parse(reader)?;
                }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        Ok(supports_sync)
    }
}

/// Parses a `getcontenttype` node.
// TODO: The PROPERTY always has data: https://www.rfc-editor.org/rfc/rfc2518#section-13.5
//       But there's also the empty node in: https://www.rfc-editor.org/rfc/rfc2518#section-8.1.3
struct GetContentTypeParser;

impl Parser for GetContentTypeParser {
    type ParsedData = Option<String>;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut content_type = None;

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(NS_DAV), Event::End(element))
                    if element.local_name().as_ref() == b"getcontenttype" =>
                {
                    break;
                }
                (ResolveResult::Unbound, Event::Text(text)) => {
                    content_type = Some(text.unescape()?.to_string());
                }

                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        Ok(content_type)
    }
}

struct ResourceTypeParser;

#[derive(Default, Debug, PartialEq, Eq)]
pub struct ResourceType {
    pub is_collection: bool,
    pub is_calendar: bool,
    pub is_address_book: bool,
}

impl Parser for ResourceTypeParser {
    type ParsedData = ResourceType;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut resource_type = ResourceType::default();

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(NS_DAV), Event::End(element))
                    if element.local_name().as_ref() == b"resourcetype" =>
                {
                    break;
                }
                (ResolveResult::Bound(NS_DAV), Event::Empty(element))
                    if element.local_name().as_ref() == b"collection" =>
                {
                    resource_type.is_collection = true;
                }
                (ResolveResult::Bound(NS_CALDAV), Event::Empty(element))
                    if element.local_name().as_ref() == b"calendar" =>
                {
                    resource_type.is_calendar = true;
                }
                (ResolveResult::Bound(NS_CARDDAV), Event::Empty(element))
                    if element.local_name().as_ref() == b"addressbook" =>
                {
                    resource_type.is_address_book = true;
                }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        Ok(resource_type)
    }
}

pub struct ResponseParser<'a, T: Parser>(pub(crate) &'a T);

impl<'a, T: Parser> Parser for ResponseParser<'a, T> {
    type ParsedData = Response<T::ParsedData>;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut href = None;
        let mut variant = ResponseVariantBuilder::None;

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(NS_DAV), Event::End(element))
                    if element.local_name().as_ref() == b"response" =>
                {
                    break
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"href" =>
                {
                    let h = TextNodeParser::HREF_PARSER
                        .parse(reader)?
                        .ok_or(Error::MissingData("text in href"))?;

                    // The first `href` is stored separately.
                    if href.is_some() {
                        variant.add_href(h)?;
                    } else {
                        href = Some(h);
                    }
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"propstat" =>
                {
                    let propstat = PropStatParser { prop: self.0 }.parse(reader)?;

                    match variant {
                        ResponseVariantBuilder::None => {
                            variant = ResponseVariantBuilder::WithProps {
                                propstats: vec![propstat],
                            };
                        }
                        ResponseVariantBuilder::WithProps { ref mut propstats } => {
                            propstats.push(propstat);
                        }
                        ResponseVariantBuilder::WithoutProps { .. } => {
                            return Err(Error::Parser(quick_xml::Error::UnexpectedToken(
                                "propstat".to_string(),
                            )))
                        }
                    }
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"status" =>
                {
                    match variant {
                        ResponseVariantBuilder::None => {
                            variant = ResponseVariantBuilder::WithoutProps {
                                hrefs: Vec::new(),
                                status: StatusParser.parse(reader)?,
                            };
                        }
                        ResponseVariantBuilder::WithProps { .. } => {
                            return Err(Error::Parser(quick_xml::Error::UnexpectedToken(
                                "status".to_string(),
                            )))
                        }
                        ResponseVariantBuilder::WithoutProps { ref mut status, .. } => {
                            *status = StatusParser.parse(reader)?;
                        }
                    }
                }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (resolve, event) => {
                    debug!("unexpected event: {:?}, {:?}", resolve, event);
                }
            }
        }

        Ok(Response {
            href: href.ok_or(Error::MissingData("href"))?,
            variant: variant.build()?,
        })
    }
}

/// Parses an entire XML containing a [`Multistatus`] node.
pub struct MultistatusDocumentParser<'a, T>(pub(crate) &'a T);

impl<'a, T: Parser> Parser for MultistatusDocumentParser<'a, T> {
    type ParsedData = Multistatus<T::ParsedData>;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut multistatus = None::<Multistatus<T::ParsedData>>;

        loop {
            match reader.read_resolved_event()? {
                (_, Event::Eof) => {
                    break;
                }
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"multistatus" =>
                {
                    match multistatus {
                        Some(ref mut m) => m
                            .responses
                            .append(&mut MultistatusParser(self.0).parse(reader)?.responses),
                        None => {
                            multistatus = Some(MultistatusParser(self.0).parse(reader)?);
                        }
                    }
                }
                (ResolveResult::Bound(NS_DAV), Event::Empty(element))
                    if element.local_name().as_ref() == b"multistatus" =>
                {
                    if multistatus.is_none() {
                        multistatus = Some(Multistatus::empty());
                    }
                }
                (_, Event::Decl(_)) => {}
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        multistatus.ok_or(Error::MissingData("multistatus"))
    }
}

/// Parses a single [`Multistatus`] node.
struct MultistatusParser<'a, T>(&'a T);

impl<'a, T: Parser> Parser for MultistatusParser<'a, T> {
    type ParsedData = Multistatus<T::ParsedData>;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut items = Vec::new();

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(NS_DAV), Event::Start(element))
                    if element.local_name().as_ref() == b"response" =>
                {
                    let item = self.0.parse(reader)?;
                    items.push(item);
                }
                (ResolveResult::Bound(NS_DAV), Event::End(element))
                    if element.local_name().as_ref() == b"multistatus" =>
                {
                    // XXX: orignal impl returns abruply here (e.g.: ends parent).
                    break;
                }
                // (ResolveResult::Unbound, Event::Text(text)) => {
                //     etag = Some(text.unescape()?.to_string());
                // }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        Ok(Multistatus { responses: items })
    }
}

/// A `prop` node which contains a single node.
pub struct PropParser<'a, X: NamedNodeParser> {
    pub inner: &'a X,
}

impl<'a, X: NamedNodeParser> Parser for PropParser<'a, X> {
    type ParsedData = X::ParsedData;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut inner = None;

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(NS_DAV), Event::End(element))
                    if element.local_name().as_ref() == b"prop" =>
                {
                    break;
                }
                (ResolveResult::Bound(namespace), Event::Start(element))
                    if namespace.as_ref() == self.inner.namespace()
                        && element.local_name().as_ref() == self.inner.name() =>
                {
                    inner = Some(self.inner.parse(reader)?);
                }
                (ResolveResult::Bound(namespace), Event::Empty(element))
                    if namespace.as_ref() == self.inner.namespace()
                        && element.local_name().as_ref() == self.inner.name() =>
                {
                    // no-op
                }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        inner.ok_or(Error::MissingData("inner data for propparser"))
    }
}

/// A node with just has text (or cdata) content.
pub(crate) struct TextNodeParser<'a> {
    pub namespace: &'a [u8],
    pub name: &'a [u8],
}

impl TextNodeParser<'_> {
    const HREF_PARSER: TextNodeParser<'static> = TextNodeParser {
        namespace: DAV,
        name: b"href",
    };
}

impl<'a> Parser for TextNodeParser<'a> {
    type ParsedData = Option<String>;

    fn parse(&self, reader: &mut NsReader<&[u8]>) -> Result<Self::ParsedData, Error> {
        let mut value = None;

        loop {
            match reader.read_resolved_event()? {
                (ResolveResult::Bound(namespace), Event::End(element))
                    if namespace.as_ref() == self.namespace
                        && element.local_name().as_ref() == self.name =>
                {
                    break;
                }
                (ResolveResult::Unbound, Event::Text(text)) => {
                    value = Some(text.unescape()?.to_string());
                }
                (ResolveResult::Unbound, Event::CData(cdata)) => {
                    let text = std::str::from_utf8(&cdata.into_inner())
                        .map_err(|e| Error::Parser(quick_xml::Error::NonDecodable(Some(e))))?
                        .to_string();
                    value = Some(text);
                }
                (_, Event::Eof) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (result, event) => {
                    debug!("unexpected data: {:?}, {:?}", result, event);
                }
            };
        }

        Ok(value)
    }
}

impl NamedNodeParser for TextNodeParser<'_> {
    #[inline]
    fn name(&self) -> &[u8] {
        self.name
    }

    #[inline]
    fn namespace(&self) -> &[u8] {
        self.namespace
    }
}
