//! Helpers for parsing XML responses returned by WebDav/CalDav/CardDav servers.
//!
//! This mostly uses the low-level [`NsReader`] API from `quick-xml`, which
//! supports namespaces and properties with details defined at runtime.
//!
//! These types are used internally by this crate and are generally reserved
//! for advanced usage.

use quick_xml::{events::Event, name::ResolveResult, NsReader};
use std::io::BufRead;

/// Namespace for properties defined in webdav specifications.
///
/// See: <https://www.rfc-editor.org/rfc/rfc3744>
pub(crate) const DAV_STR: &str = "DAV:";
pub(crate) const CALDAV_STR: &str = "urn:ietf:params:xml:ns:caldav";
pub(crate) const CARDDAV_STR: &str = "urn:ietf:params:xml:ns:carddav";

pub(crate) const DAV: &[u8] = DAV_STR.as_bytes();
pub(crate) const CALDAV: &[u8] = CALDAV_STR.as_bytes();
pub(crate) const CARDDAV: &[u8] = CARDDAV_STR.as_bytes();

/// An error parsing XML data.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("missing field in data")]
    MissingData(&'static str),

    #[error(transparent)]
    Parser(#[from] quick_xml::Error),
}

/// A type that can be built by parsing XML.
#[allow(clippy::module_name_repetitions)]
pub trait FromXml: Sized {
    type Data;
    /// Builds a new instance by parsing the XML reader.
    ///
    /// The opening tag for this type is expected to have been consumed prior
    /// to calling this method.
    ///
    /// # Errors
    ///
    /// If the raw data is not valid XML or does not match the expected format.
    // TODO: on failure, should the reader be moved to the end of the matching end node?
    fn from_xml<R: BufRead>(reader: &mut NsReader<R>, data: &Self::Data) -> Result<Self, Error>;
}

/// Details of a single item that are returned when listing them.
///
/// This does not include actual item data, it only includes their metadata.
#[derive(Debug, PartialEq, Eq)]
pub struct ItemDetails {
    pub content_type: Option<String>,
    pub etag: String,
    pub is_collection: bool,
    pub is_calendar: bool,
    pub is_address_book: bool,
}

impl FromXml for ItemDetails {
    type Data = ();
    /// Parse a list item using an XML reader.
    ///
    /// The reader is expected to have consumed the start tag for the `response`
    /// element, and will return after having consumed the corresponding end tag.
    ///
    /// # Errors
    ///
    /// - If parsing the XML fails in any way.
    /// - If any necessary fields are missing.
    /// - If a `response` object has a status code different to 200.
    /// - If any unexpected XML nodes are found.
    fn from_xml<R: BufRead>(reader: &mut NsReader<R>, _: &()) -> Result<ItemDetails, Error> {
        #[derive(Debug)]
        enum State {
            Prop,
            ResourceType,
            GetContentType,
            GetEtag,
        }

        let mut buf = Vec::new();
        let mut state = State::Prop;
        let mut content_type = Option::<String>::None;
        let mut etag = Option::<String>::None;
        let mut is_collection = false;
        let mut is_calendar = false;
        let mut is_address_book = false;

        loop {
            match (&state, reader.read_resolved_event_into(&mut buf)?) {
                (_, (ResolveResult::Bound(namespace), Event::Start(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Prop, DAV, b"resourcetype") => state = State::ResourceType,
                        (State::Prop, DAV, b"getcontenttype") => state = State::GetContentType,
                        (State::Prop, DAV, b"getetag") => state = State::GetEtag,
                        (_, _, _) => {
                            // TODO: log unknown/unhandled node
                        }
                    }
                }
                (_, (ResolveResult::Bound(namespace), Event::End(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Prop, DAV, b"prop") => break,
                        (State::ResourceType, DAV, b"resourcetype")
                        | (State::GetContentType, DAV, b"getcontenttype")
                        | (State::GetEtag, DAV, b"getetag") => state = State::Prop,
                        (_, _, _) => {
                            // TODO: log unknown/unhandled node
                        }
                    }
                }
                (_, (ResolveResult::Bound(namespace), Event::Empty(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Prop, DAV, b"resourcetype") => {}
                        (State::ResourceType, DAV, b"collection") => is_collection = true,
                        (State::ResourceType, CALDAV, b"calendar") => is_calendar = true,
                        (State::ResourceType, CARDDAV, b"addressbook") => is_address_book = true,
                        (_, _, _) => {
                            // TODO: log unknown/unhandled node
                        }
                    }
                }
                (State::GetContentType, (ResolveResult::Unbound, Event::Text(text))) => {
                    content_type = Some(text.unescape()?.to_string());
                }
                (State::GetEtag, (ResolveResult::Unbound, Event::Text(text))) => {
                    etag = Some(text.unescape()?.to_string());
                }
                (_, (_, _)) => {
                    // TODO: log unknown/unhandled event
                }
            };
        }

        Ok(ItemDetails {
            content_type,
            etag: etag.ok_or(Error::MissingData("etag"))?,
            is_collection,
            is_calendar,
            is_address_book,
        })
    }
}

/// Etag and contents of a single calendar resource.
#[derive(Debug, PartialEq, Eq)]
pub struct CalendarReport {
    pub etag: String,
    pub calendar_data: String,
}

impl FromXml for CalendarReport {
    type Data = ();
    /// Parse a list item using an XML reader.
    ///
    /// The reader is expected to have consumed the start tag for the `response`
    /// element, and will return after having consumed the corresponding end tag.
    ///
    /// # Errors
    ///
    /// - If parsing the XML fails in any way.
    /// - If any necessary fields are missing.
    /// - If a `response` object has a status code different to 200.
    /// - If any unexpected XML nodes are found.
    fn from_xml<R: BufRead>(reader: &mut NsReader<R>, _: &()) -> Result<CalendarReport, Error> {
        #[derive(Debug)]
        enum State {
            Prop,
            GetEtag,
            CalendarData,
        }

        let mut buf = Vec::new();
        let mut state = State::Prop;
        let mut etag = Option::<String>::None;
        let mut calendar_data = None;

        loop {
            match (&state, reader.read_resolved_event_into(&mut buf)?) {
                (_, (ResolveResult::Bound(namespace), Event::Start(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Prop, CALDAV, b"calendar-data") => state = State::CalendarData,
                        (State::Prop, DAV, b"getetag") => state = State::GetEtag,
                        (_, _, _) => {
                            // TODO: log unknown/unhandled node
                        }
                    }
                }
                (_, (ResolveResult::Bound(namespace), Event::End(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Prop, DAV, b"prop") => break,
                        (State::CalendarData, CALDAV, b"calendar-data")
                        | (State::GetEtag, DAV, b"getetag") => state = State::Prop,
                        (_, _, _) => {
                            // TODO: log unknown/unhandled node
                        }
                    }
                }
                (State::CalendarData, (ResolveResult::Unbound, Event::Text(text))) => {
                    // TODO: can I avoid copying here?
                    calendar_data = Some(text.unescape()?.to_string());
                }
                (State::GetEtag, (ResolveResult::Unbound, Event::Text(text))) => {
                    etag = Some(text.unescape()?.to_string());
                }
                (_, (_, _)) => {
                    // TODO: log unknown/unhandled event
                }
            };
        }

        Ok(CalendarReport {
            etag: etag.ok_or(Error::MissingData("etag"))?,
            calendar_data: calendar_data.ok_or(Error::MissingData("calendar-data"))?,
        })
    }
}

/// A response with one or more properties.
///
/// The inner type `T` will be parsed from the response's `prop` node.
/// Generally, this will be a response to a `PROPFIND`.
///
/// See: <https://www.rfc-editor.org/rfc/rfc2518>
#[derive(Debug, PartialEq, Eq)]
pub struct ResponseWithProp<T>
where
    T: FromXml,
{
    pub href: String,
    pub prop: T,
    pub status: String,
}

impl<T> FromXml for ResponseWithProp<T>
where
    T: FromXml,
{
    type Data = T::Data;

    fn from_xml<R: BufRead>(reader: &mut NsReader<R>, data: &T::Data) -> Result<Self, Error> {
        #[derive(Debug)]
        enum State {
            Response,
            Href,
            PropStat,
            Status,
        }

        let mut buf = Vec::new();
        let mut state = State::Response;
        let mut href = Option::<String>::None;
        let mut status = Option::<String>::None;
        let mut value = Option::<T>::None;

        loop {
            match (&state, reader.read_resolved_event_into(&mut buf)?) {
                (_, (ResolveResult::Bound(namespace), Event::Start(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Response, DAV, b"href") => state = State::Href,
                        (State::Response, DAV, b"propstat") => state = State::PropStat,
                        (State::PropStat, DAV, b"prop") => {
                            value = Some(T::from_xml(reader, data)?);
                        }
                        (State::PropStat, DAV, b"status") => state = State::Status,
                        (_, _, _) => {
                            // TODO: log unknown/unhandled node
                        }
                    }
                }
                (_, (ResolveResult::Bound(namespace), Event::End(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Response, DAV, b"response") => break,
                        (State::Href, DAV, b"href") | (State::PropStat, DAV, b"propstat") => {
                            state = State::Response;
                        }
                        (State::Status, DAV, b"status") => state = State::PropStat,
                        (_, _, _) => {
                            // TODO: log unknown/unhandled node
                        }
                    }
                }
                (State::Href, (ResolveResult::Unbound, Event::Text(text))) => {
                    href = Some(text.unescape()?.to_string());
                }
                (State::Status, (ResolveResult::Unbound, Event::Text(text))) => {
                    status = Some(text.unescape()?.to_string());
                }
                (_, (_, _)) => {
                    // TODO: log unknown/unhandled event
                }
            }
        }

        Ok(ResponseWithProp {
            href: href.ok_or(Error::MissingData("href"))?,
            prop: value.ok_or(Error::MissingData("property value"))?,
            status: status.ok_or(Error::MissingData("status"))?,
        })
    }
}

/// A simple string property, like a `displayname`, `color`, etc.
#[derive(Debug, PartialEq, Eq)]
pub struct StringProperty(Option<String>);

#[derive(Debug, Clone)]
pub struct SimplePropertyMeta {
    pub name: Vec<u8>,
    pub namespace: Vec<u8>,
}

impl From<ResponseWithProp<StringProperty>> for Option<String> {
    fn from(value: ResponseWithProp<StringProperty>) -> Option<String> {
        value.prop.0
    }
}

impl FromXml for StringProperty {
    type Data = SimplePropertyMeta;

    fn from_xml<R: BufRead>(
        reader: &mut NsReader<R>,
        data: &SimplePropertyMeta,
    ) -> Result<Self, Error> {
        #[derive(Debug)]
        enum State {
            Prop,
            Inner,
        }

        let mut buf = Vec::new();
        let mut state = State::Prop;
        let mut value = Option::<String>::None;

        loop {
            match (&state, reader.read_resolved_event_into(&mut buf)?) {
                (State::Prop, (ResolveResult::Bound(namespace), Event::Start(element)))
                    if namespace.as_ref() == data.namespace
                        && element.local_name().as_ref() == data.name =>
                {
                    state = State::Inner;
                }
                (State::Prop, (ResolveResult::Bound(namespace), Event::End(element)))
                    if namespace.as_ref() == DAV && element.local_name().as_ref() == b"prop" =>
                {
                    break;
                }
                (State::Prop, (ResolveResult::Bound(namespace), Event::Empty(element)))
                    if namespace.as_ref() == data.namespace
                        && element.local_name().as_ref() == data.name =>
                {
                    // No-op
                }
                (State::Inner, (ResolveResult::Bound(namespace), Event::End(element)))
                    if namespace.as_ref() == data.namespace
                        && element.local_name().as_ref() == data.name =>
                {
                    state = State::Prop;
                }
                (State::Inner, (ResolveResult::Unbound, Event::Text(text))) => {
                    value = Some(text.unescape()?.to_string());
                }
                (State::Inner, (ResolveResult::Unbound, Event::CData(c))) => {
                    let text = std::str::from_utf8(&c.into_inner())
                        .map_err(|e| Error::Parser(quick_xml::Error::NonDecodable(Some(e))))?
                        .to_string();
                    // TODO: on error, read_to_end to leave parser in a consistent state.
                    value = Some(text);
                }
                (_, (_, _)) => {
                    // TODO: log unknown/unhandled event
                }
            }
        }

        Ok(StringProperty(value))
    }
}

/// A property with a single `href` node.
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
/// Can be parsed with the following [`SimplePropertyMeta`]:
///
/// ```rust
/// # use libdav::xml::SimplePropertyMeta;;
/// let property_data = SimplePropertyMeta {
///     name: b"current-user-principal".to_vec(),
///     namespace: b"DAV:".to_vec(),
/// };
/// ```
pub struct HrefProperty(Option<String>);

impl ResponseWithProp<HrefProperty> {
    #[must_use]
    pub fn into_maybe_string(self) -> Option<String> {
        self.prop.0
    }
}

impl FromXml for HrefProperty {
    type Data = SimplePropertyMeta;

    fn from_xml<R>(reader: &mut NsReader<R>, data: &SimplePropertyMeta) -> Result<Self, Error>
    where
        R: BufRead,
    {
        #[derive(Debug)]
        enum State {
            Prop,
            Inner,
            Href,
        }

        let mut buf = Vec::new();
        let mut state = State::Prop;
        let mut value = Option::<String>::None;

        loop {
            match (&state, reader.read_resolved_event_into(&mut buf)?) {
                (State::Prop, (ResolveResult::Bound(namespace), Event::Start(element)))
                    if namespace.as_ref() == data.namespace
                        && element.local_name().as_ref() == data.name =>
                {
                    state = State::Inner;
                }
                (State::Prop, (ResolveResult::Bound(namespace), Event::End(element)))
                    if namespace.as_ref() == DAV && element.local_name().as_ref() == b"prop" =>
                {
                    break;
                }
                (State::Inner, (ResolveResult::Bound(namespace), Event::Start(element)))
                    if namespace.as_ref() == DAV && element.local_name().as_ref() == b"href" =>
                {
                    state = State::Href;
                }
                (State::Inner, (ResolveResult::Bound(namespace), Event::End(element)))
                    if namespace.as_ref() == data.namespace
                        && element.local_name().as_ref() == data.name =>
                {
                    state = State::Prop;
                }
                (State::Href, (ResolveResult::Unbound, Event::Text(text))) => {
                    value = Some(text.unescape()?.to_string());
                }
                (State::Href, (ResolveResult::Unbound, Event::CData(c))) => {
                    let text = std::str::from_utf8(&c.into_inner())
                        .map_err(|e| Error::Parser(quick_xml::Error::NonDecodable(Some(e))))?
                        .to_string();
                    value = Some(text);
                }
                (State::Href, (ResolveResult::Bound(namespace), Event::End(element)))
                    if namespace.as_ref() == DAV && element.local_name().as_ref() == b"href" =>
                {
                    state = State::Inner;
                }
                (_, (_, _)) => {
                    // TODO: log unknown/unhandled event
                }
            }
        }

        Ok(HrefProperty(value))
    }
}

#[derive(Debug)]
pub struct Multistatus<F> {
    responses: Vec<F>,
}

impl<F> Multistatus<F> {
    #[inline]
    pub fn into_responses(self) -> Vec<F> {
        self.responses
    }
}

impl<F> FromXml for Multistatus<F>
where
    F: FromXml,
{
    type Data = F::Data;

    fn from_xml<R: BufRead>(reader: &mut NsReader<R>, data: &Self::Data) -> Result<Self, Error> {
        #[derive(Debug)]
        enum State {
            Root,
            Multistatus,
        }

        let mut state = State::Root;
        let mut buf = Vec::new();
        let mut items = Vec::new();

        loop {
            match (&state, reader.read_resolved_event_into(&mut buf)?) {
                (State::Root, (_, Event::Decl(_))) => {}
                (State::Root, (ResolveResult::Bound(namespace), Event::Start(element)))
                    if namespace.as_ref() == DAV
                        && element.local_name().as_ref() == b"multistatus" =>
                {
                    state = State::Multistatus;
                }
                (State::Root, (ResolveResult::Bound(namespace), Event::Empty(element)))
                    if namespace.as_ref() == DAV
                        && element.local_name().as_ref() == b"multistatus" =>
                {
                    return Ok(Multistatus { responses: items });
                }
                (State::Root, (_, Event::Eof)) => return Err(Error::MissingData("multistatus")),
                (State::Multistatus, (ResolveResult::Bound(namespace), Event::Start(element)))
                    if namespace.as_ref() == DAV
                        && element.local_name().as_ref() == b"response" =>
                {
                    items.push(F::from_xml(reader, data)?);
                }
                (State::Multistatus, (ResolveResult::Bound(namespace), Event::End(element)))
                    if namespace.as_ref() == DAV
                        && element.local_name().as_ref() == b"multistatus" =>
                {
                    return Ok(Multistatus { responses: items });
                }
                (_, (_, _)) => {
                    // TODO: log unknown/unhandled event
                }
            }
        }
    }
}

/// Parse a raw multi-response when listing items.
///
/// # Errors
///
/// - If parsing the XML fails in any way.
/// - If any necessary fields are missing.
/// - If any unexpected XML nodes are found.
pub(crate) fn parse_multistatus<F>(raw: &[u8], data: &F::Data) -> Result<Multistatus<F>, Error>
where
    F: FromXml,
{
    Multistatus::from_xml(&mut NsReader::from_reader(raw), data)
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

        let parsed = parse_multistatus::<ResponseWithProp<ItemDetails>>(raw, &())
            .unwrap()
            .into_responses();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], ResponseWithProp {
            href: "/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/".to_string(),
            prop: ItemDetails {
                content_type: Some("text/calendar; charset=utf-8".to_string()),
                etag: "\"1591712486-1-1\"".to_string(),
                is_collection: true,
                is_calendar: true,
                is_address_book: false,
            },
            status: "HTTP/1.1 200 OK".to_string()
        });
        assert_eq!(parsed[1], ResponseWithProp {
            href: "/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/395b00a0-eebc-40fd-a98e-176a06367c82.ics".to_string(),
            prop: ItemDetails {
                content_type: Some("text/calendar; charset=utf-8; component=VEVENT".to_string()),
                etag: "\"e7577ff2b0924fe8e9a91d3fb2eb9072598bf9fb\"".to_string(),
                is_collection: false,
                is_calendar: false,
                is_address_book: false,
            },
            status: "HTTP/1.1 200 OK".to_string()
        });
    }
    #[test]
    fn test_empty_response() {
        let raw = br#"<multistatus xmlns="DAV:" />"#;
        let parsed = parse_multistatus::<ResponseWithProp<ItemDetails>>(raw, &())
            .unwrap()
            .into_responses();
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
        let property_data = SimplePropertyMeta {
            name: b"displayname".to_vec(),
            namespace: DAV.to_vec(),
        };
        let parsed = parse_multistatus::<ResponseWithProp<StringProperty>>(raw, &property_data)
            .unwrap()
            .into_responses();
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0],
            ResponseWithProp {
                href: "/path".to_string(),
                prop: StringProperty(Some("test calendar".to_string())),
                status: "HTTP/1.1 200 OK".to_string()
            }
        );
    }
}
