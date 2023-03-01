use std::io;

/// A generic error for `WebDav` operations.
#[derive(thiserror::Error, Debug)]
pub enum DavError {
    #[error("http error executing request")]
    Network(#[from] reqwest::Error),

    #[error("failure parsing XML response")]
    Xml(#[from] crate::xml::Error),

    #[error("failed to parse a URL returned by the server")]
    BadUrl(#[from] url::ParseError),
}

impl From<DavError> for io::Error {
    fn from(value: DavError) -> Self {
        match value {
            DavError::Network(_) => io::Error::new(io::ErrorKind::Other, value),
            DavError::BadUrl(_) => io::Error::new(io::ErrorKind::InvalidInput, value),
            DavError::Xml(e) => io::Error::new(io::ErrorKind::InvalidData, e),
        }
    }
}
