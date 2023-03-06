//! Miscellaneous helpers.
use sha2::{Digest, Sha256};

/// Return the SHA256 hash of a string.
///
/// This is used as a fallback when a storage backend doesn't provide [`Etag`] values, or when an
/// item is missing its `UID`.
///
/// [`Etag`]: crate::base::Etag
pub fn hash<S: AsRef<str>>(input: S) -> String {
    // TODO: vdirsyner-py ignores lines that start with PRODID, since this gets mutated all the
    // time when entries don't change.
    // See: hash_item and normalize_item
    format!("{:X}", Sha256::digest(input.as_ref()))
}
