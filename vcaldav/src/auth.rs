//! Authentication-related types.

use base64::{prelude::BASE64_STANDARD, write::EncoderWriter};
use http::{HeaderValue, Request};
use std::io::Write;

/// Authentication schemes supported by [`DavClient`](crate::dav::DavClient).
#[non_exhaustive]
#[derive(Debug)]
pub enum Auth {
    None,
    Basic {
        username: String,
        password: Option<String>,
    },
}

/// Internal error resolving authentication.
///
/// This error is returned when there is an internal error handling authentication (e.g.: the input
/// is invalid). It IS NOT returned when authentication was rejected by the server.
#[derive(thiserror::Error, Debug)]
#[error(transparent)]
pub struct AuthError(Box<dyn std::error::Error + Sync + Send>);

impl AuthError {
    fn from<E: std::error::Error + Sync + Send + 'static>(err: E) -> Self {
        Self(Box::from(err))
    }
}

impl Auth {
    /// Apply this authentication to a request builder.
    pub(crate) fn new_request(&self) -> Result<http::request::Builder, AuthError> {
        let request = Request::builder();
        match self {
            Auth::None => Ok(request),
            Auth::Basic { username, password } => {
                let mut sequence = b"Basic ".to_vec();
                let mut encoder = EncoderWriter::new(&mut sequence, &BASE64_STANDARD);
                if let Some(pwd) = password {
                    write!(encoder, "{username}:{pwd}").map_err(AuthError::from)?;
                } else {
                    write!(encoder, "{username}:").map_err(AuthError::from)?;
                }
                drop(encoder); // Releases the mutable borrow for `sequence`.

                let mut header = HeaderValue::from_bytes(&sequence).map_err(AuthError::from)?;
                header.set_sensitive(true);
                Ok(request.header(hyper::header::AUTHORIZATION, header.clone()))
            }
        }
    }
}
