//! Helpers for parsing XML responses returned by CalDav servers.
//!
//! This mostly uses the low-level [`NsReader`] API from `quick-xml`. The
//! low-level API supports namespaces, which are quite relevant for `WebDav`.

use quick_xml::{events::Event, name::ResolveResult, NsReader};
use std::io::BufRead;

const DAV: &[u8] = b"DAV:";
const CALDAV: &[u8] = b"urn:ietf:params:xml:ns:caldav";
const CARDDAV: &[u8] = b"urn:ietf:params:xml:ns:carddav";

/// An error parsing XML data.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("missing field in data")]
    MissingData(&'static str),

    /// Indicates that we found unexpected XML data in a response.
    ///
    /// This will likely be removed after extensive testing, since it is mostly
    /// a hack to easily debug edge cases that have been missing.
    #[error("unexpected structure in XML")]
    UnexpectedXml(String),

    #[error(transparent)]
    Parser(#[from] quick_xml::Error),

    #[error("status code is not 200")]
    BadStatus(String),
}

pub trait FromXml: Sized {
    type Data;
    fn from_xml<R: BufRead>(reader: &mut NsReader<R>, data: &Self::Data) -> Result<Self, Error>;
}

/// Details of a single item that are returned when listing them.
///
/// This does not include actual item data, it only includes their metadata.
#[derive(Debug, PartialEq, Eq)]
pub struct ItemDetails {
    pub href: String,
    pub content_type: String,
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
            Root,
            Href,
            PropStat,
            Prop,
            ResourceType,
            GetContentType,
            GetEtag,
            Status,
        }
        let mut state = State::Root;
        let mut href = Option::<String>::None;
        let mut content_type = Option::<String>::None;
        let mut etag = Option::<String>::None;
        let mut is_collection = false;
        let mut is_calendar = false;
        let mut is_address_book = false;

        loop {
            match (&state, reader.read_resolved_event_into(&mut buf)?) {
                (_, (ResolveResult::Bound(namespace), Event::Start(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Root, DAV, b"href") => state = State::Href,
                        (State::Root, DAV, b"propstat") => state = State::PropStat,
                        (State::PropStat, DAV, b"prop") => state = State::Prop,
                        (State::PropStat, DAV, b"status") => state = State::Status,
                        (State::Prop, DAV, b"resourcetype") => state = State::ResourceType,
                        (State::Prop, DAV, b"getcontenttype") => state = State::GetContentType,
                        (State::Prop, DAV, b"getetag") => state = State::GetEtag,
                        (state, namespace, element) => {
                            return Err(Error::UnexpectedXml(format!(
                                "{state:?}, {namespace:?}, {element:?}",
                            )));
                        }
                    }
                }
                (_, (ResolveResult::Bound(namespace), Event::End(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Root, DAV, b"response") => break,
                        (State::Href, DAV, b"href") => state = State::Root,
                        (State::PropStat, DAV, b"propstat") => state = State::Root,
                        (State::Prop, DAV, b"prop") => state = State::PropStat,
                        (State::ResourceType, DAV, b"resourcetype") => state = State::Prop,
                        (State::GetContentType, DAV, b"getcontenttype") => state = State::Prop,
                        (State::GetEtag, DAV, b"getetag") => state = State::Prop,
                        (State::Status, DAV, b"status") => state = State::PropStat,
                        (state, namespace, element) => {
                            return Err(Error::UnexpectedXml(format!(
                                "{state:?}, {namespace:?}, {element:?}",
                            )));
                        }
                    }
                }
                (_, (ResolveResult::Bound(namespace), Event::Empty(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Prop, DAV, b"resourcetype") => {}
                        (State::ResourceType, DAV, b"collection") => is_collection = true,
                        (State::ResourceType, CALDAV, b"calendar") => is_calendar = true,
                        (State::ResourceType, CARDDAV, b"addressbook") => is_address_book = true,
                        (state, namespace, element) => {
                            return Err(Error::UnexpectedXml(format!(
                                "{state:?}, {namespace:?}, {element:?}",
                            )));
                        }
                    }
                }
                (State::Href, (ResolveResult::Unbound, Event::Text(text))) => {
                    href = Some(text.unescape()?.to_string())
                }
                (State::Status, (ResolveResult::Unbound, Event::Text(text))) => {
                    let unescaped = text.unescape()?;
                    if !unescaped.ends_with("200 OK") {
                        return Err(Error::BadStatus(unescaped.to_string()));
                    }
                }
                (State::GetContentType, (ResolveResult::Unbound, Event::Text(text))) => {
                    content_type = Some(text.unescape()?.to_string());
                }
                (State::GetEtag, (ResolveResult::Unbound, Event::Text(text))) => {
                    etag = Some(text.unescape()?.to_string());
                }
                (state, (resolve, event)) => {
                    return Err(Error::UnexpectedXml(format!(
                        "{state:?}, {resolve:?}, {event:?}",
                    )));
                }
            };
        }

        Ok(ItemDetails {
            href: href.ok_or(Error::MissingData("href"))?,
            content_type: content_type.ok_or(Error::MissingData("content_type"))?,
            etag: etag.ok_or(Error::MissingData("etag"))?,
            is_collection,
            is_calendar,
            is_address_book,
        })
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
            (State::Root, (ResolveResult::Bound(namespace), Event::Start(element))) => {
                if namespace.as_ref() == DAV && element.local_name().as_ref() == b"multistatus" {
                    state = State::Multistatus;
                }
            }
            (State::Root, (_, Event::Eof)) => return Err(Error::MissingData("multistatus")),
            (State::Multistatus, (ResolveResult::Bound(namespace), Event::Start(element))) => {
                if namespace.as_ref() == DAV && element.local_name().as_ref() == b"response" {
                    items.push(F::from_xml(reader, &data));
                } else {
                    return Err(Error::UnexpectedXml(format!("{namespace:?} {element:?}")));
                };
            }
            (State::Multistatus, (ResolveResult::Bound(namespace), Event::End(element))) => {
                if namespace.as_ref() == DAV && element.local_name().as_ref() == b"multistatus" {
                    return Ok(items);
                } else {
                    return Err(Error::UnexpectedXml(format!(
                        "{state:?} {namespace:?}, {element:?}",
                    )));
                };
            }
            (State::Multistatus, (_, Event::Eof)) => return Err(Error::MissingData("response")),
            (state, (resolve, event)) => {
                return Err(Error::UnexpectedXml(format!(
                    "{state:?} {resolve:?}, {event:?}",
                )));
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

        let parsed = parse_multistatus::<ItemDetails>(raw, ()).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].as_ref().unwrap(), &ItemDetails {
            href: "/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/".to_string(),
            content_type: "text/calendar; charset=utf-8".to_string(),
            etag: "\"1591712486-1-1\"".to_string(),
            is_collection: true,
            is_calendar: true,
            is_address_book: false,
        });
        assert_eq!(parsed[1].as_ref().unwrap(), &ItemDetails {
            href: "/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/395b00a0-eebc-40fd-a98e-176a06367c82.ics".to_string(),
            content_type: "text/calendar; charset=utf-8; component=VEVENT".to_string(),
            etag: "\"e7577ff2b0924fe8e9a91d3fb2eb9072598bf9fb\"".to_string(),
            is_collection: false,
            is_calendar: false,
            is_address_book: false,
        });
    }
}
