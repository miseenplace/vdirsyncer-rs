// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Utilities for handling XML data.
use std::borrow::Cow;
use std::str::FromStr;

use http::status::InvalidStatusCode;
use http::StatusCode;
use percent_encoding::percent_encode;
use percent_encoding::{percent_decode_str, AsciiSet, NON_ALPHANUMERIC};
use roxmltree::ExpandedName;
use roxmltree::Node;

use crate::dav::{check_status, DavError};
use crate::names::STATUS;

/// Characters that are escaped for hrefs.
pub(crate) const DISALLOWED_FOR_HREF: &AsciiSet = &NON_ALPHANUMERIC.remove(b'/').remove(b'.');

/// Check all the statuses in a `multistatus` response.
///
/// # Errors
///
/// - If any of the `<DAV:status>` nodes is missing the status text, returns
/// [`DavError::InvalidResponse`].
///
/// - If the text inside a `<DAV:status>` node is not a valid status line, returns
/// [`DavError::InvalidStatusCode`].
///
/// - If any of the statuses are non-success, returns [`DavError::BadStatusCode`].
pub fn check_multistatus(root: Node) -> Result<(), DavError> {
    let statuses = root.descendants().filter(|node| node.tag_name() == STATUS);
    for status in statuses {
        let status = status.text().ok_or(DavError::InvalidResponse(
            "missing text inside 'DAV:status'".into(),
        ))?;
        check_status(parse_statusline(status)?)?;
    }

    Ok(())
}

/// Parses a status line string into a [`StatusCode`].
///
/// Example input string: `HTTP/1.1 200 OK`.
///
/// # See also
///
/// - The [status element](https://www.rfc-editor.org/rfc/rfc2518#section-12.9.1.2)
/// - [Status-Line](https://www.rfc-editor.org/rfc/rfc2068#section-6.1)
///
/// # Errors
///
/// If the input string does not match a status line.
pub fn parse_statusline<S: AsRef<str>>(status_line: S) -> Result<StatusCode, InvalidStatusCode> {
    let mut iter = status_line.as_ref().splitn(3, ' ');
    iter.next();
    let code = iter.next().unwrap_or("");
    StatusCode::from_str(code)
}

/// Render an empty XML node.
pub(crate) fn render_xml(name: &ExpandedName) -> String {
    if let Some(ns) = name.namespace() {
        format!("<{0} xmlns=\"{1}\"/>", name.name(), ns)
    } else {
        format!("<{0}/>", name.name())
    }
}

/// Render an XML node with optional text.
pub fn render_xml_with_text<S: AsRef<str>>(name: &ExpandedName, text: Option<S>) -> String {
    match (name.namespace(), text) {
        (None, None) => format!("<{}/>", name.name()),
        (None, Some(t)) => format!("<{0}>{1}</{0}>", name.name(), escape_text(t.as_ref())),
        (Some(ns), None) => format!("<{0} xmlns=\"{ns}\"/>", name.name()),
        (Some(ns), Some(t)) => format!(
            "<{0} xmlns=\"{ns}\">{1}</{0}>",
            name.name(),
            escape_text(t.as_ref())
        ),
    }
}

/// Replaces characters that need to be escaped in texts.
///
/// `<` --> `&lt;`
/// `>` --> `&gt;`
/// `&` --> `&amp;`
///
/// This IS NOT usable in other contexts of XML encoding.
#[must_use]
pub fn escape_text(raw: &str) -> Cow<str> {
    // This function is strongly based on `escape_partial` from `quick-xml`:
    {
        // The MIT License (MIT)
        //
        // Copyright (c) 2016 Johann Tuffe
        //
        // Permission is hereby granted, free of charge, to any person obtaining a copy
        // of this software and associated documentation files (the "Software"), to deal
        // in the Software without restriction, including without limitation the rights
        // to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
        // copies of the Software, and to permit persons to whom the Software is
        // furnished to do so, subject to the following conditions:
        //
        //
        // The above copyright notice and this permission notice shall be included in
        // all copies or substantial portions of the Software.
        //
        //
        // THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
        // IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
        // FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL THE
        // AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
        // LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
        // OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
        // THE SOFTWARE.
        let bytes = raw.as_bytes();
        let mut escaped = None;
        let mut iter = bytes.iter();
        let mut pos = 0;
        while let Some(i) = iter.position(|&b| matches!(b, b'<' | b'>' | b'&')) {
            let escaped = escaped.get_or_insert_with(|| Vec::with_capacity(raw.len()));
            let new_pos = pos + i;
            escaped.extend_from_slice(&bytes[pos..new_pos]);
            match bytes[new_pos] {
                b'<' => escaped.extend_from_slice(b"&lt;"),
                b'>' => escaped.extend_from_slice(b"&gt;"),
                b'&' => escaped.extend_from_slice(b"&amp;"),
                _ => unreachable!("Only '<', '>' and '&', are escaped"),
            }
            pos = new_pos + 1;
        }

        if let Some(mut escaped) = escaped {
            if let Some(raw) = bytes.get(pos..) {
                escaped.extend_from_slice(raw);
            }
            // SAFETY: we operate on UTF-8 input and search for an one byte chars only,
            // so all slices that was put to the `escaped` is a valid UTF-8 encoded strings
            // TODO: Can be replaced with the following unsafe snippet:
            // Cow::Owned(unsafe { String::from_utf8_unchecked(escaped) })
            Cow::Owned(
                String::from_utf8(escaped).expect("manually escaped string must be valid utf-8"),
            )
        } else {
            Cow::Borrowed(raw)
        }
    }
    // End copied code.
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use crate::xmlutils::escape_text;

    #[test]
    fn test_escape_text() {
        match escape_text("HELLO THERE") {
            Cow::Borrowed(s) => assert_eq!(s, "HELLO THERE"),
            Cow::Owned(_) => panic!("expected Borrowed, got Owned"),
        }
        match escape_text("HELLO <") {
            Cow::Borrowed(_) => panic!("expected Owned, got Borrowed"),
            Cow::Owned(s) => assert_eq!(s, "HELLO &lt;"),
        }
        match escape_text("HELLO &lt;") {
            Cow::Borrowed(_) => panic!("expected Owned, got Borrowed"),
            Cow::Owned(s) => assert_eq!(s, "HELLO &amp;lt;"),
        }
        match escape_text("你吃过了吗？") {
            Cow::Borrowed(s) => assert_eq!(s, "你吃过了吗？"),
            Cow::Owned(_) => panic!("expected Borrowed, got Owned"),
        }
    }
}

/// Find an `href` node and return its unescaped text value.
//
// TODO: document that all input to libdav should be unescaped, and that all output is unescaped.
pub(crate) fn get_unquoted_href<'a>(node: &'a Node) -> Result<Cow<'a, str>, DavError> {
    Ok(node
        .descendants()
        .find(|node| node.tag_name() == crate::names::HREF)
        .ok_or(DavError::InvalidResponse("missing href in response".into()))?
        .text()
        .map(percent_decode_str)
        .ok_or(DavError::InvalidResponse("missing text in href".into()))?
        .decode_utf8()?)
}

// URL-encodes an href.
//
// Obviously the input parameter MUST NOT be url-encoded.
pub(crate) fn quote_href<'a>(href: &'a [u8]) -> Cow<'a, str> {
    Cow::from(percent_encode(href, DISALLOWED_FOR_HREF))
}
