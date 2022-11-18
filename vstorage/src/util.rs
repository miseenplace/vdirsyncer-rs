use sha2::{Digest, Sha256};

/// Return the SHA256 hash of a string.
///
/// This is used as a fallback when a storage backend doesn't provide [`Etag`] values, or when an
/// item is missing its `UID`.
pub fn hash<S: AsRef<str>>(input: S) -> String {
    format!("{:X}", Sha256::digest(input.as_ref()))
}
