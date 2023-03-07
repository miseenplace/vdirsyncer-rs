//! Helpers for DNS-based discovery.

use std::cmp::Ordering;
use std::io;
use std::string::FromUtf8Error;

use domain::base::name::LongChainError;
use domain::base::octets::ParseError;
use domain::base::ToRelativeDname;
use domain::resolv::lookup::srv::SrvError;
use domain::{
    base::{Dname, Question, RelativeDname, Rtype},
    rdata::Txt,
    resolv::StubResolver,
};

#[derive(Debug, Clone, Copy)]
pub enum DiscoverableService {
    CalDavs,
    CalDav,
    CardDavs,
    CardDav,
}

impl DiscoverableService {
    /// Return a relative domain suitable for querying this service type.
    pub fn relative_domain(self) -> &'static RelativeDname<[u8]> {
        match self {
            DiscoverableService::CalDavs => RelativeDname::from_slice(b"\x08_caldavs\x04_tcp"),
            DiscoverableService::CalDav => RelativeDname::from_slice(b"\x07_caldav\x04_tcp"),
            DiscoverableService::CardDavs => RelativeDname::from_slice(b"\x09_carddavs\x04_tcp"),
            DiscoverableService::CardDav => RelativeDname::from_slice(b"\x08_carddav\x04_tcp"),
        }
        .expect("well known relative prefix is valid")
    }
}

/// Resolves SRV to locate the caldav server.
///
/// Returns a `Vec` of host/ports, in the order in which they should be tried.
/// Returns an empty list if no SRV records were found.
///
/// # Errors
///
/// If the underlying DNS request fails or the SRV record cannot be parsed.
///
/// # See also
///
/// - <https://www.rfc-editor.org/rfc/rfc2782>
/// - <https://www.rfc-editor.org/rfc/rfc6764>
pub async fn resolve_srv_record<T: std::convert::AsRef<[u8]>>(
    service: DiscoverableService,
    domain: &Dname<T>,
    port: u16,
) -> Result<Vec<(String, u16)>, SrvError> {
    let response = StubResolver::new()
        .lookup_srv(service.relative_domain(), domain, port)
        .await?;

    let mut srvs: Vec<_> = match response {
        Some(s) => s.into_srvs().collect(),
        None => return Ok(vec![]), // TODO: what to do if "decidedly not availabe?"
    };

    // A client MUST attempt to contact the target host with the lowest-numbered priority it can reach[...]
    // [...] Larger weights SHOULD be given a proportionately higher probability of being selected. [...]
    srvs.sort_unstable_by(|s1, s2| {
        match s1.priority().cmp(&s2.priority()) {
            Ordering::Less => Ordering::Less,
            Ordering::Equal => s2.weight().cmp(&s1.weight()), // Hint: in reverse order!
            Ordering::Greater => Ordering::Greater,
        }
    });

    Ok(srvs
        .iter()
        .map(|s| (s.target().to_string(), s.port()))
        .collect())
}

/// Error returned by [`find_context_path_via_txt_records`].
#[derive(thiserror::Error, Debug)]
pub enum TxtError {
    #[error("I/O error performing DNS request")]
    Network(#[from] io::Error),

    #[error("the domain name is too long and cannot be queried")]
    DomainTooLong(#[from] LongChainError),

    #[error("error parsing DNS response")]
    ParseError(#[from] ParseError),

    #[error("txt record does not contain a valid utf-8 string")]
    NotUtf8Error(#[from] FromUtf8Error),

    #[error("data in txt record does no have the right syntax")]
    BadTxt,
}

impl From<TxtError> for io::Error {
    fn from(value: TxtError) -> Self {
        match value {
            TxtError::Network(err) => err,
            TxtError::DomainTooLong(_) => io::Error::new(io::ErrorKind::InvalidInput, value),
            TxtError::ParseError(_) | TxtError::NotUtf8Error(_) | TxtError::BadTxt => {
                io::Error::new(io::ErrorKind::InvalidData, value)
            }
        }
    }
}

/// Resolves a context path via TXT records.
///
/// This returns a path where the default context path should be used for a given domain.
/// The domain provided should be in the format of `example.com` or `posteo.de`.
///
/// Returns an empty list of no relevant record was found.
///
/// # Errors
///
/// See [`TxtError`]
///
/// # See also
///
/// <https://www.rfc-editor.org/rfc/rfc6764>
pub async fn find_context_path_via_txt_records<T: std::convert::AsRef<[u8]>>(
    service: DiscoverableService,
    domain: &Dname<T>,
) -> Result<Option<String>, TxtError> {
    let resolver = StubResolver::new();
    let full_domain = service.relative_domain().chain(domain)?;
    let question = Question::new_in(full_domain, Rtype::Txt);

    let response = resolver.query(question).await?;
    let record = match response.answer()?.next() {
        Some(r) => r?,
        None => return Ok(None),
    };
    let parsed_record = match record.into_record::<Txt<_>>()? {
        Some(r) => r,
        None => return Ok(None),
    };
    let data = parsed_record.data();
    let bytes = data
        .text::<Vec<u8>>()
        .expect("record fits in newly created buffer");
    let mut text = String::from_utf8(bytes)?;

    if text.starts_with("path=") {
        Ok(Some(text.split_off(5)))
    } else {
        Err(TxtError::BadTxt)
    }
}
