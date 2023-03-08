//! Miscellaneous helpers.
use sha2::{Digest, Sha256};

/// Return the SHA256 hash of a string.
///
/// This string is expected to be an icalendar or vcard component. This does some
/// minimal normalisations of the items before hashing:
///
/// - Ignores the `PROPID` field. Two item where only this field varies are
///   considered equivalent.
///
/// This is used as a fallback when a storage backend doesn't provide [`Etag`] values, or when an
/// item is missing its `UID`.
///
/// [`Etag`]: crate::base::Etag
pub fn hash<S: AsRef<str>>(input: S) -> String {
    // TODO: vdirsyner-py ignores lines that start with PRODID, since this gets mutated all the
    // time when entries don't change.
    // See: hash_item and normalize_item
    let mut hasher = Sha256::new();
    for line in input.as_ref().split_inclusive("\r\n") {
        // Hint: continuation lines always start with a space.
        if line.starts_with("PROD_ID") {
            continue;
        }
        hasher.update(line);
    }
    format!("{:X}", hasher.finalize())
}

// TODO: add tests for the `hash` method! Compare to the hash of the item with no PRODID.
