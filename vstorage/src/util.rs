//! Miscellaneous helpers.
use sha2::{Digest, Sha256};

/// Return the SHA256 hash of a string.
///
/// This string is expected to be an icalendar or vcard component.
pub(crate) fn hash<S: AsRef<str>>(input: S) -> String {
    // TODO: See (in vdirsyncer-py): hash_item and normalize_item
    let mut hasher = Sha256::new();
    for line in input.as_ref().split_inclusive("\r\n") {
        // Hint: continuation lines always start with a space.
        if line.starts_with("PROD_ID") {
            // These get continuously mutated and result in noise when determining if two
            // components are equivalent.
            continue;
        }
        hasher.update(line);
    }
    format!("{:X}", hasher.finalize())
}

// TODO: add tests for the `hash` method! Compare to the hash of the item with no PRODID.
