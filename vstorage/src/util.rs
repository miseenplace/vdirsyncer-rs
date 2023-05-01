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
        if line.starts_with("PRODID") {
            // These get continuously mutated and result in noise when determining if two
            // components are equivalent.
            continue;
        }
        hasher.update(line);
    }
    format!("{:X}", hasher.finalize())
}

#[cfg(test)]
mod test {
    use crate::util::hash;

    #[test]
    fn compare_hashing_with_and_without_prodid() {
        let without_prodid = vec![
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n");
        let with_prodid = vec![
            "PRODID:test-client",
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n");

        assert_eq!(hash(without_prodid), hash(with_prodid));
    }
}
