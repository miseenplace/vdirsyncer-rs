//! Helpers for parsing XML responses returned by CalDav servers.
//!
//! This mostly uses the low-level [`NsReader`] API from `quick-xml`, which
//! supports namespaces and properties with details defined at runtime.
//!
//! These types are used internally by this crate and are generally reserved
//! for advanced usage.

use quick_xml::{
    events::Event,
    name::{QName, ResolveResult},
    NsReader,
};
use std::io::BufRead;

pub(crate) const DAV: &[u8] = b"DAV:";
pub(crate) const CALDAV: &[u8] = b"urn:ietf:params:xml:ns:caldav";
pub(crate) const CARDDAV: &[u8] = b"urn:ietf:params:xml:ns:carddav";

/// An error parsing XML data.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("missing field in data")]
    MissingData(&'static str),

    #[error(transparent)]
    Parser(#[from] quick_xml::Error),

    #[error("status code is not 200")]
    BadStatus(String),
}

/// A type that can be built by parsing XML.
pub trait FromXml: Sized {
    type Data;
    /// Builds a new instance by parsing the XML reader.
    ///
    /// The opening tag for this type is expected to have been consumed prior
    /// to calling this method.
    // TODO: on failure, the reader should be moved to the end of the matching end node.
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
        let mut buf = Vec::new();

        #[derive(Debug)]
        enum State {
            Prop,
            ResourceType,
            GetContentType,
            GetEtag,
        }
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
                        (State::ResourceType, DAV, b"resourcetype") => state = State::Prop,
                        (State::GetContentType, DAV, b"getcontenttype") => state = State::Prop,
                        (State::GetEtag, DAV, b"getetag") => state = State::Prop,
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
        let mut buf = Vec::new();

        #[derive(Debug)]
        enum State {
            Response,
            Href,
            PropStat,
            Status,
        }
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
                        (State::Href, DAV, b"href") => state = State::Response,
                        (State::PropStat, DAV, b"propstat") => state = State::Response,
                        (State::Status, DAV, b"status") => state = State::PropStat,
                        (_, _, _) => {
                            // TODO: log unknown/unhandled node
                        }
                    }
                }
                (State::Href, (ResolveResult::Unbound, Event::Text(text))) => {
                    href = Some(text.unescape()?.to_string())
                }
                (State::Status, (ResolveResult::Unbound, Event::Text(text))) => {
                    status = Some(text.unescape()?.to_string())
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

#[derive(Debug)]
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
        let mut buf = Vec::new();

        #[derive(Debug)]
        enum State {
            Prop,
            Inner,
        }
        let mut state = State::Prop;
        let mut value = Option::<String>::None;

        loop {
            match (&state, reader.read_resolved_event_into(&mut buf)?) {
                (State::Prop, (ResolveResult::Bound(namespace), Event::Start(element)))
                    if namespace.as_ref() == data.namespace
                        && element.local_name().as_ref() == data.name =>
                {
                    state = State::Inner
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
                (State::Inner, (ResolveResult::Unbound, Event::CData(cdata))) => {
                    let text = std::str::from_utf8(&cdata.into_inner())
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
/// # use vcaldav::xml::SimplePropertyMeta;;
/// let property_data = SimplePropertyMeta {
///     name: b"current-user-principal".to_vec(),
///     namespace: b"DAV:".to_vec(),
/// };
/// ```
pub struct HrefProperty(Option<String>);

impl ResponseWithProp<HrefProperty> {
    pub fn into_maybe_string(self) -> Option<String> {
        self.prop.0
    }
}

impl From<ResponseWithProp<HrefProperty>> for Option<String> {
    fn from(value: ResponseWithProp<HrefProperty>) -> Option<String> {
        value.prop.0
    }
}

impl FromXml for HrefProperty {
    type Data = SimplePropertyMeta;

    fn from_xml<R: BufRead>(
        reader: &mut NsReader<R>,
        data: &SimplePropertyMeta,
    ) -> Result<Self, Error> {
        let mut buf = Vec::new();

        #[derive(Debug)]
        enum State {
            Prop,
            Inner,
            Href,
        }
        let mut state = State::Prop;
        let mut value = Option::<String>::None;

        loop {
            match (&state, reader.read_resolved_event_into(&mut buf)?) {
                (State::Prop, (ResolveResult::Bound(namespace), Event::Start(element)))
                    if namespace.as_ref() == data.namespace
                        && element.local_name().as_ref() == data.name =>
                {
                    state = State::Inner
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
                (State::Href, (ResolveResult::Unbound, Event::CData(cdata))) => {
                    let text = std::str::from_utf8(&cdata.into_inner())
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

/// Parse a raw multi-response when listing items.
///
/// # Errors
///
/// - If parsing the XML fails in any way.
/// - If any necessary fields are missing.
/// - If any unexpected XML nodes are found.
pub(crate) fn parse_multistatus<F: FromXml>(
    raw: &str,
    data: F::Data,
) -> Result<Vec<Result<F, Error>>, Error> {
    //TODO: Use an async reader instead (this is mostly a Poc).
    let reader = &mut NsReader::from_str(raw);
    reader.trim_text(true);

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
                if namespace.as_ref() == DAV && element.local_name().as_ref() == b"multistatus" =>
            {
                state = State::Multistatus;
            }
            (State::Root, (ResolveResult::Bound(namespace), Event::Empty(element)))
                if namespace.as_ref() == DAV && element.local_name().as_ref() == b"multistatus" =>
            {
                return Ok(items);
            }
            (State::Root, (_, Event::Eof)) => return Err(Error::MissingData("multistatus")),
            (State::Multistatus, (ResolveResult::Bound(namespace), Event::Start(element)))
                if namespace.as_ref() == DAV && element.local_name().as_ref() == b"response" =>
            {
                let item = F::from_xml(reader, &data);
                // FIXME: HACK: missing data doesn't leave the reader inconsistent.
                if let Err(ref err) = item {
                    if let Error::MissingData(_) = err {
                    } else {
                        reader.read_to_end(QName("response".as_bytes()))?;
                    }
                }
                items.push(item);
            }
            (State::Multistatus, (ResolveResult::Bound(namespace), Event::End(element)))
                if namespace.as_ref() == DAV && element.local_name().as_ref() == b"multistatus" =>
            {
                return Ok(items);
            }
            (_, (_, _)) => {
                // TODO: log unknown/unhandled event
            }
        }
    }
}

#[cfg(test)]
mod more_tests {

    use super::*;

    #[test]
    fn test_parse_list_result() {
        let raw = r#"
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

        let parsed = parse_multistatus::<ResponseWithProp<ItemDetails>>(raw, ()).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].as_ref().unwrap(), &ResponseWithProp {
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
        assert_eq!(parsed[1].as_ref().unwrap(), &ResponseWithProp {
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
        let raw = r#"<multistatus xmlns="DAV:" />"#;
        let parsed = parse_multistatus::<ResponseWithProp<ItemDetails>>(raw, ()).unwrap();
        assert_eq!(parsed.len(), 0);
    }

    #[test]
    fn test_single_propstat() {
        let raw = r#"
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
        let parsed =
            parse_multistatus::<ResponseWithProp<StringProperty>>(raw, property_data).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].as_ref().unwrap(),
            &ResponseWithProp {
                href: "/path".to_string(),
                prop: StringProperty(Some("test calendar".to_string())),
                status: "HTTP/1.1 200 OK".to_string()
            }
        );
    }
}
