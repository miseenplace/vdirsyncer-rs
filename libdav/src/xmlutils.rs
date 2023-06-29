//! Utilities for handling XML data.
use std::str::FromStr;

use http::status::InvalidStatusCode;
use http::StatusCode;
use roxmltree::ExpandedName;
use roxmltree::Node;

use crate::dav::{check_status, DavError};
use crate::names::STATUS;

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
        (None, Some(t)) => format!(
            "<{0}>{1}</{0}>",
            name.name(),
            quick_xml::escape::partial_escape(t.as_ref())
        ),
        (Some(ns), None) => format!("<{0} xmlns=\"{ns}\"/>", name.name()),
        (Some(ns), Some(t)) => format!(
            "<{0} xmlns=\"{ns}\">{1}</{0}>",
            name.name(),
            quick_xml::escape::partial_escape(t.as_ref())
        ),
    }
}
