//! One canonical persisted UUID spelling and one derived Core Audio UID for every endpoint.
/// Parses only a nonzero lowercase hyphenated UUID; labels and alternate spellings are rejected.
/// Returns `None` for malformed identities. No endpoint is synthesized or looked up by name.
#[must_use]
pub fn parse_endpoint_id(value: &str) -> Option<[u8; 16]> {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    let mut id = [0_u8; 16];
    let mut index = 0;
    for (position, byte) in bytes.iter().copied().enumerate() {
        if matches!(position, 8 | 13 | 18 | 23) {
            if byte != b'-' {
                return None;
            }
            continue;
        }
        let nibble = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => return None,
        };
        id[index / 2] = (id[index / 2] << 4) | nibble;
        index += 1;
    }
    (id != [0; 16]).then_some(id)
}

/// Formats stable binary identity into the only supported persisted UUID spelling.
#[must_use]
pub fn format_endpoint_id(id: [u8; 16]) -> String {
    let mut value = String::with_capacity(36);
    for (index, byte) in id.into_iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            value.push('-');
        }
        push_hex_byte(&mut value, byte);
    }
    value
}

/// Derives the system UID directly from identity bytes, independent of graph/name/defaults.
#[must_use]
pub fn endpoint_uid(id: [u8; 16]) -> String {
    let mut uid = String::with_capacity(59);
    uid.push_str("io.mixrelay.audio.endpoint.");
    for byte in id {
        push_hex_byte(&mut uid, byte);
    }
    uid
}

fn push_hex_byte(out: &mut String, byte: u8) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    out.push(char::from(DIGITS[usize::from(byte >> 4)]));
    out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_uuid_and_system_uid_are_stable_without_a_default_identity_alias() {
        let text = "17e59d73-42c1-4f11-a8b4-6c2d913357e0";
        let id = parse_endpoint_id(text).unwrap();
        assert_eq!(format_endpoint_id(id), text);
        assert_eq!(
            endpoint_uid(id),
            "io.mixrelay.audio.endpoint.17e59d7342c14f11a8b46c2d913357e0"
        );
        for invalid in [
            "default",
            "17e59d7342c14f11a8b46c2d913357e0",
            "17E59D73-42c1-4f11-a8b4-6c2d913357e0",
            "00000000-0000-0000-0000-000000000000",
        ] {
            assert!(parse_endpoint_id(invalid).is_none());
        }
    }
}
