//! Helpers for parsing XML responses returned by WebDav/CalDav/CardDav servers.
//!
//! This mostly uses the low-level [`NsReader`] API from `quick-xml`, which
//! supports namespaces and properties with details defined at runtime.
//!
//! These types are used internally by this crate and are generally reserved
//! for advanced usage.

use http::{status::InvalidStatusCode, StatusCode};
use log::{debug, warn};
use quick_xml::{events::Event, name::ResolveResult, NsReader};
use std::str::FromStr;
use std::{borrow::Cow, io::BufRead};

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

    #[error("invalid status code")]
    InvalidStatusCode(#[from] InvalidStatusCode),

    #[error(transparent)]
    Parser(#[from] quick_xml::Error),
}

/// A type that can be built by parsing XML.
#[allow(clippy::module_name_repetitions)]
pub trait FromXml: Sized {
    /// Optional information used to extract values.
    ///
    /// Some `FromXml` implementations can be generic and parse data with
    /// dynamic rules. This associated type allows passing such rules.
    ///
    /// For the general case, just set this to `()`.
    type Data;
    /// Builds a new instance by parsing the XML reader.
    ///
    /// The opening tag for this type is expected to have been consumed prior
    /// to calling this method. The end tag will be consumed before returning,
    /// unless an error is returned.
    ///
    /// # Errors
    ///
    /// - If parsing the XML fails in any way.
    /// - If any mandatory fields are missing.
    fn from_xml<R: BufRead>(reader: &mut NsReader<R>, data: &Self::Data) -> Result<Self, Error>;
}

/// Details of a single item that are returned when listing them.
///
/// This does not include actual item data, it only includes their metadata.
#[derive(Debug, PartialEq, Eq)]
pub struct ItemDetails {
    pub content_type: Option<String>,
    pub etag: Option<String>,
    pub is_collection: bool,
    pub is_calendar: bool,
    pub is_address_book: bool,
}

/// Shortcut to keep log statements short.
#[inline]
fn s(data: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(data)
}

impl FromXml for ItemDetails {
    type Data = ();

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
                        (state, ns, name) => {
                            debug!("unexpected start: {:?}, {}, {}", state, s(ns), s(name));
                        }
                    }
                }
                (_, (ResolveResult::Bound(namespace), Event::End(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Prop, DAV, b"prop") => break,
                        (State::ResourceType, DAV, b"resourcetype")
                        | (State::GetContentType, DAV, b"getcontenttype")
                        | (State::GetEtag, DAV, b"getetag") => state = State::Prop,
                        (state, ns, name) => {
                            debug!("unexpected end: {:?}, {}, {}", state, s(ns), s(name));
                        }
                    }
                }
                (_, (ResolveResult::Bound(namespace), Event::Empty(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Prop, DAV, b"resourcetype") => {}
                        (State::ResourceType, DAV, b"collection") => is_collection = true,
                        (State::ResourceType, CALDAV, b"calendar") => is_calendar = true,
                        (State::ResourceType, CARDDAV, b"addressbook") => is_address_book = true,
                        (State::Prop, DAV, b"getetag") => {
                            warn!("missing etag in response");
                        }
                        (state, ns, name) => {
                            debug!("unexpected empty: {:?}, {}, {}", state, s(ns), s(name));
                        }
                    }
                }
                (State::GetContentType, (ResolveResult::Unbound, Event::Text(text))) => {
                    content_type = Some(text.unescape()?.to_string());
                }
                (State::GetEtag, (ResolveResult::Unbound, Event::Text(text))) => {
                    etag = Some(text.unescape()?.to_string());
                }
                (_, (_, Event::Eof)) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (state, (_, event)) => {
                    debug!("unexpected event: {:?}, {:?}", state, event);
                }
            };
        }

        Ok(ItemDetails {
            content_type,
            etag,
            is_collection,
            is_calendar,
            is_address_book,
        })
    }
}

/// Etag and contents of a single calendar resource.
#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub etag: Option<String>,
    pub data: Option<String>,
}

/// Metadata describing which field contains the `data` for a `Report`.
pub struct ReportField {
    pub namespace: &'static [u8],
    pub name: &'static [u8],
}

impl ReportField {
    pub const CALENDAR_DATA: ReportField = ReportField {
        namespace: CALDAV,
        name: b"calendar-data",
    };

    pub const ADDRESS_DATA: ReportField = ReportField {
        namespace: CARDDAV,
        name: b"address-data",
    };
}

impl FromXml for Report {
    type Data = ReportField;

    fn from_xml<R>(reader: &mut NsReader<R>, field: &ReportField) -> Result<Report, Error>
    where
        R: BufRead,
    {
        #[derive(Debug)]
        enum State {
            Prop,
            GetEtag,
            CalendarData,
        }

        let mut buf = Vec::new();
        let mut state = State::Prop;
        let mut etag = None;
        let mut data = None;

        loop {
            match (&state, reader.read_resolved_event_into(&mut buf)?) {
                (_, (ResolveResult::Bound(namespace), Event::Start(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Prop, ns, name) if ns == field.namespace && name == field.name => {
                            state = State::CalendarData;
                        }
                        (State::Prop, DAV, b"getetag") => state = State::GetEtag,
                        (state, ns, name) => {
                            debug!("unexpected start: {:?}, {}, {}", state, s(ns), s(name));
                        }
                    }
                }
                (_, (ResolveResult::Bound(namespace), Event::End(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Prop, DAV, b"prop") => break,
                        (State::CalendarData, ns, name)
                            if ns == field.namespace && name == field.name =>
                        {
                            state = State::Prop;
                        }
                        (State::GetEtag, DAV, b"getetag") => state = State::Prop,
                        (state, ns, name) => {
                            debug!("unexpected end: {:?}, {}, {}", state, s(ns), s(name));
                        }
                    }
                }
                (State::CalendarData, (ResolveResult::Unbound, Event::Text(text))) => {
                    // TODO: can I avoid copying here?
                    data = Some(text.unescape()?.to_string());
                }
                (State::CalendarData, (ResolveResult::Unbound, Event::CData(c))) => {
                    // TODO: assuming UTF-8
                    let text = std::str::from_utf8(&c.into_inner())
                        .map_err(|e| Error::Parser(quick_xml::Error::NonDecodable(Some(e))))?
                        .to_string();
                    data = Some(text);
                }
                (State::GetEtag, (ResolveResult::Unbound, Event::Text(text))) => {
                    etag = Some(text.unescape()?.to_string());
                }
                (_, (_, Event::Eof)) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (state, (_, event)) => {
                    debug!("unexpected event: {:?}, {:?}", state, event);
                }
            };
        }

        Ok(Report { etag, data })
    }
}

/// A single response from a multistatus response.
///
/// The inner type `T` will be parsed from the response's `prop` node.
/// Generally, this is used for responses to `PROPFIND` or `REPORT`.
///
/// See: <https://www.rfc-editor.org/rfc/rfc2518>
#[derive(Debug, PartialEq, Eq)]
pub struct Response<T>
where
    T: FromXml,
{
    pub href: String,
    pub variant: ResponseVariant<T>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResponseVariant<T: FromXml> {
    WithProps {
        propstats: Vec<PropStat<T>>,
    },
    WithoutProps {
        hrefs: Vec<String>,
        status: StatusCode,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct PropStat<T>
where
    T: FromXml,
{
    pub prop: T,
    pub status: StatusCode,
}

// See: https://www.rfc-editor.org/rfc/rfc2068#section-6.1
fn parse_statusline<S: AsRef<str>>(status_line: S) -> Result<StatusCode, InvalidStatusCode> {
    let mut iter = status_line.as_ref().splitn(3, ' ');
    iter.next();
    let code = iter.next().unwrap_or("");
    StatusCode::from_str(code)
}

impl<T> FromXml for PropStat<T>
where
    T: FromXml,
{
    type Data = T::Data;

    fn from_xml<R: BufRead>(reader: &mut NsReader<R>, data: &Self::Data) -> Result<Self, Error> {
        #[derive(Debug)]
        enum State {
            PropStat,
            Status,
        }

        let mut buf = Vec::new();
        let mut state = State::PropStat;
        let mut status = None;
        let mut prop = None;

        loop {
            match (&state, reader.read_resolved_event_into(&mut buf)?) {
                (_, (ResolveResult::Bound(namespace), Event::Start(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::PropStat, DAV, b"status") => state = State::Status,
                        (State::PropStat, DAV, b"prop") => {
                            prop = Some(T::from_xml(reader, data)?);
                        }
                        (state, ns, name) => {
                            debug!("unexpected end: {:?}, {}, {}", state, s(ns), s(name));
                        }
                    }
                }
                (_, (ResolveResult::Bound(namespace), Event::End(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::PropStat, DAV, b"propstat") => break,
                        (State::Status, DAV, b"status") => {
                            state = State::PropStat;
                        }
                        (state, ns, name) => {
                            debug!("unexpected empty: {:?}, {}, {}", state, s(ns), s(name));
                        }
                    }
                }
                (State::Status, (ResolveResult::Unbound, Event::Text(text))) => {
                    status = Some(parse_statusline(text.unescape()?)?);
                }
                (_, (_, Event::Eof)) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (_, (_, event)) => {
                    debug!("unexpected event: {:?}, {:?}", state, event);
                }
            }
        }

        Ok(PropStat {
            prop: prop.ok_or(Error::MissingData("prop"))?,
            status: status.ok_or(Error::MissingData("status"))?,
        })
    }
}

impl<T> FromXml for Response<T>
where
    T: FromXml,
{
    type Data = T::Data;

    fn from_xml<R: BufRead>(reader: &mut NsReader<R>, data: &T::Data) -> Result<Self, Error> {
        #[derive(Debug)]
        enum State {
            Response,
            Href,
            Status, // Only for one variant
        }

        enum VariantBuilder<T: FromXml> {
            None,
            WithProps {
                propstats: Vec<PropStat<T>>,
            },
            WithoutProps {
                hrefs: Vec<String>,
                status: Option<StatusCode>,
            },
        }

        impl<T: FromXml> VariantBuilder<T> {
            fn build(self) -> Result<ResponseVariant<T>, Error> {
                match self {
                    VariantBuilder::None => Ok(ResponseVariant::WithProps {
                        propstats: Vec::new(),
                    }),
                    VariantBuilder::WithProps { propstats } => {
                        Ok(ResponseVariant::WithProps { propstats })
                    }
                    VariantBuilder::WithoutProps { hrefs, status } => {
                        Ok(ResponseVariant::WithoutProps {
                            hrefs,
                            status: status.ok_or(Error::MissingData("status"))?,
                        })
                    }
                }
            }
        }

        let mut buf = Vec::new();
        let mut state = State::Response;
        let mut href = Option::<String>::None;
        let mut variant = VariantBuilder::None;

        loop {
            match (&state, reader.read_resolved_event_into(&mut buf)?) {
                (_, (ResolveResult::Bound(namespace), Event::Start(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Response, DAV, b"href") => {
                            if href.is_some() {
                                match variant {
                                    VariantBuilder::None => {
                                        variant = VariantBuilder::WithoutProps {
                                            hrefs: Vec::new(),
                                            status: None,
                                        };
                                    }
                                    VariantBuilder::WithProps { .. } => {
                                        return Err(Error::Parser(
                                            quick_xml::Error::UnexpectedToken("href".to_string()),
                                        ))
                                    }
                                    VariantBuilder::WithoutProps { .. } => {}
                                }
                            }
                            state = State::Href;
                        }
                        (State::Response, DAV, b"propstat") => {
                            let propstat = PropStat::<T>::from_xml(reader, data)?;

                            match variant {
                                VariantBuilder::None => {
                                    variant = VariantBuilder::WithProps {
                                        propstats: vec![propstat],
                                    };
                                }
                                VariantBuilder::WithProps { ref mut propstats } => {
                                    propstats.push(propstat);
                                }
                                VariantBuilder::WithoutProps { .. } => {
                                    return Err(Error::Parser(quick_xml::Error::UnexpectedToken(
                                        "propstat".to_string(),
                                    )))
                                }
                            }
                        }
                        (State::Response, DAV, b"status") => {
                            match variant {
                                VariantBuilder::None => {
                                    variant = VariantBuilder::WithoutProps {
                                        hrefs: Vec::new(),
                                        status: None,
                                    }
                                }
                                VariantBuilder::WithProps { .. } => {
                                    return Err(Error::Parser(quick_xml::Error::UnexpectedToken(
                                        "status".to_string(),
                                    )))
                                }
                                VariantBuilder::WithoutProps { .. } => {}
                            }
                            state = State::Status;
                        }
                        (state, ns, name) => {
                            debug!("unexpected end: {:?}, {}, {}", state, s(ns), s(name));
                        }
                    }
                }
                (_, (ResolveResult::Bound(namespace), Event::End(element))) => {
                    match (&state, namespace.as_ref(), element.local_name().as_ref()) {
                        (State::Response, DAV, b"response") => break,
                        (State::Status, DAV, b"status") | (State::Href, DAV, b"href") => {
                            state = State::Response;
                        }
                        (state, ns, name) => {
                            debug!("unexpected empty: {:?}, {}, {}", state, s(ns), s(name));
                        }
                    }
                }
                (State::Href, (ResolveResult::Unbound, Event::Text(text))) => {
                    let h = text.unescape()?.to_string();
                    match href {
                        None => href = Some(h),
                        Some(_) => match variant {
                            VariantBuilder::None | VariantBuilder::WithProps { .. } => {
                                unreachable!()
                            }
                            VariantBuilder::WithoutProps { ref mut hrefs, .. } => hrefs.push(h),
                        },
                    }
                }
                (State::Status, (ResolveResult::Unbound, Event::Text(text))) => match variant {
                    VariantBuilder::None | VariantBuilder::WithProps { .. } => unreachable!(),
                    VariantBuilder::WithoutProps { ref mut status, .. } => {
                        *status = Some(parse_statusline(text.unescape()?)?);
                    }
                },
                (_, (_, Event::Eof)) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (state, (_, event)) => {
                    debug!("unexpected event: {:?}, {:?}", state, event);
                }
            }
        }

        Ok(Response {
            href: href.ok_or(Error::MissingData("href"))?,
            variant: variant.build()?,
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

impl From<Response<StringProperty>> for Option<String> {
    fn from(value: Response<StringProperty>) -> Option<String> {
        if let ResponseVariant::WithProps { mut propstats } = value.variant {
            propstats.pop()?.prop.0
        } else {
            None
        }
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
                (_, (_, Event::Eof)) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (state, (_, event)) => {
                    debug!("unexpected event: {:?}, {:?}", state, event);
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

impl Response<HrefProperty> {
    #[must_use]
    pub fn into_maybe_string(self) -> Option<String> {
        if let ResponseVariant::WithProps { mut propstats } = self.variant {
            propstats.pop()?.prop.0
        } else {
            None
        }
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
                (_, (_, Event::Eof)) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (state, (_, event)) => {
                    debug!("unexpected event: {:?}, {:?}", state, event);
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
    #[must_use]
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
                (_, (_, Event::Eof)) => {
                    return Err(Error::from(quick_xml::Error::UnexpectedEof(String::new())));
                }
                (state, (_, event)) => {
                    debug!("unexpected event: {:?}, {:?}", state, event);
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
    let mut reader = NsReader::from_reader(raw);
    reader.trim_text(true);
    Multistatus::from_xml(&mut reader, data)
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

        let parsed = parse_multistatus::<Response<ItemDetails>>(raw, &())
            .unwrap()
            .into_responses();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], Response {
            href: "/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/".to_string(),
            variant: ResponseVariant::WithProps {
                propstats: vec![
                    PropStat {
                        prop: ItemDetails {
                            content_type: Some("text/calendar; charset=utf-8".to_string()),
                            etag: Some("\"1591712486-1-1\"".to_string()),
                            is_collection: true,
                            is_calendar: true,
                            is_address_book: false,
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
                            is_collection: false,
                            is_calendar: false,
                            is_address_book: false,
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
        let parsed = parse_multistatus::<Response<ItemDetails>>(raw, &())
            .unwrap()
            .into_responses();
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
        let parsed = parse_multistatus::<Response<Report>>(raw, &ReportField::CALENDAR_DATA)
            .unwrap()
            .into_responses();
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0],
            Response {
                href: "/user/calendars/Q208cKvMGjAdJFUw/qJJ9Li5DPJYr.ics".to_string(),
                variant: ResponseVariant::WithProps {
                    propstats: vec![PropStat {
                        prop: Report {
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
        let parsed = parse_multistatus::<Response<ItemDetails>>(raw, &())
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
        let parsed = parse_multistatus::<Response<StringProperty>>(raw, &property_data)
            .unwrap()
            .into_responses();
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0],
            Response {
                href: "/path".to_string(),
                variant: ResponseVariant::WithProps {
                    propstats: vec![PropStat {
                        prop: StringProperty(Some("test calendar".to_string())),
                        status: StatusCode::OK,
                    }],
                },
            }
        );
    }
}
