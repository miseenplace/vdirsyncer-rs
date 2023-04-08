//! This crate is part of the `vdirsyncer` project, and implements a common API for
//! reading different underlying storages which can contain `icalendar` or `vcard`
//! entries.

pub mod base;
pub mod caldav;
pub mod filesystem;
pub mod readonly;
mod simple_component;
pub mod util;
pub mod webcal;

type Result<T> = std::result::Result<T, crate::Error>;

#[derive(Debug)]
pub enum ErrorKind {
    DoesNotExist,
    NotACollection,
    NotAStorage,
    AccessDenied,
    Io,
    InvalidData,
    InvalidInput,
    ReadOnly,
    CollectionNotEmpty,
    Unsupported,
    // #[deprecated]
    Uncategorised,
}

impl ErrorKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::DoesNotExist => "resource does not exist",
            ErrorKind::NotACollection => "resource exists, but is not a collection",
            ErrorKind::NotAStorage => "resource exists, but is not a storage",
            ErrorKind::AccessDenied => "access to the resource was denied",
            ErrorKind::Io => "input/output error",
            ErrorKind::InvalidData => "operation returned data, but it is not valid",
            ErrorKind::InvalidInput => "input data is invalid",
            ErrorKind::ReadOnly => "the resource is read-only",
            ErrorKind::CollectionNotEmpty => "the collection is not empty",
            ErrorKind::Unsupported => "the operation is not supported",
            ErrorKind::Uncategorised => "uncategorised error",
        }
    }
    // TODO: generate rustdoc for each variant using this method?
}

#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    fn new<E>(kind: ErrorKind, source: E) -> Error
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Error {
            kind,
            source: Some(source.into()),
        }
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Error { kind, source: None }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        let kind = match value.kind() {
            std::io::ErrorKind::NotFound => ErrorKind::DoesNotExist,
            std::io::ErrorKind::PermissionDenied => ErrorKind::AccessDenied,
            std::io::ErrorKind::InvalidInput => ErrorKind::InvalidInput,
            std::io::ErrorKind::InvalidData => ErrorKind::InvalidData,
            _ => ErrorKind::Io,
        };
        Error {
            kind,
            source: Some(value.into()),
        }
    }
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.write_str(self.as_str())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.source {
            Some(ref s) => write!(fmt, "{}: {}", self.kind, s),
            None => self.kind.fmt(fmt),
        }
    }
}

impl std::error::Error for Error {}
