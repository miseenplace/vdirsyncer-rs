//! Authentication-related types.

use base64::{prelude::BASE64_STANDARD, write::EncoderWriter};
use http::{request::Builder, HeaderValue};
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

pub(crate) trait AuthExt: Sized {
    /// Apply this authentication to an object.
    fn authenticate(self, auth: &Auth) -> Result<Self, AuthError>;
}

impl AuthExt for Builder {
    /// Apply this authentication to a request builder.
    fn authenticate(self, auth: &Auth) -> Result<Builder, AuthError> {
        match auth {
            Auth::None => Ok(self),
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
                Ok(self.header(hyper::header::AUTHORIZATION, header.clone()))
            }
        }
    }
}
