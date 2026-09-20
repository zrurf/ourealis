//! Base64 for the bytes that travel inside JSON.
//!
//! An OMF image appears in a request body as base64 (`omf/edit`, `omf/patch`,
//! `map.inline`), and comes back out the same way. Written out rather than pulled
//! in: the crate needs exactly this and nothing else from a base64 library, and the
//! decoding rules are worth stating in one place — a permissive decoder that
//! silently drops the tail bits turns a truncated upload into a *shorter* image
//! that may still parse, which is a data-integrity bug rather than a convenience.
//!
//! Accepted: the standard alphabet with `+` and `/`, the URL-safe `-` and `_`,
//! optional padding, and surrounding or embedded whitespace (a client that wraps a
//! long payload is common). Rejected: any other character, a length that cannot be
//! a base64 payload, and padding anywhere but at the end.

use crate::error::{Result, ServiceError};

/// The standard alphabet, index = value.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes bytes as standard, padded base64.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let triple = ((group[0] as u32) << 16) | ((group[1] as u32) << 8) | group[2] as u32;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

/// Decodes base64, rejecting anything that is not a complete payload.
///
/// `what` names the field the payload came from, so the message tells the caller
/// which part of its request to fix.
pub fn decode(text: &str, what: &str) -> Result<Vec<u8>> {
    // Whichever character is out of place, the report has to point at it.
    let mut digits: Vec<u8> = Vec::with_capacity(text.len());
    let mut padding = 0usize;
    let mut seen_padding = false;
    for (index, character) in text.bytes().enumerate() {
        if character == b'=' {
            padding += 1;
            seen_padding = true;
            if padding > 2 {
                return Err(invalid(what, format!("more than two '=' at byte {index}")));
            }
            continue;
        }
        if matches!(character, b' ' | b'\n' | b'\r' | b'\t') {
            continue;
        }
        if seen_padding {
            return Err(invalid(
                what,
                format!("data after the padding, at byte {index}"),
            ));
        }
        let value = match character {
            b'A'..=b'Z' => character - b'A',
            b'a'..=b'z' => character - b'a' + 26,
            b'0'..=b'9' => character - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            other => {
                return Err(invalid(
                    what,
                    format!("byte {other:#04x} at position {index} is not base64"),
                ));
            }
        };
        digits.push(value);
    }

    // A base64 payload carries whole 24-bit groups; four characters per group, and a
    // final group of two or three characters when the length is not a multiple of
    // three. One leftover character encodes six bits and nothing else, so it can only
    // be a truncated payload — accepting it would hand the caller a prefix of its own
    // bytes with no error.
    if digits.len() % 4 == 1 {
        return Err(invalid(
            what,
            format!(
                "{} digit(s) is not a whole number of base64 groups",
                digits.len()
            ),
        ));
    }
    let expected_padding = match digits.len() % 4 {
        0 if !digits.is_empty() => 0,
        2 => 2,
        3 => 1,
        _ => 0,
    };
    if padding != 0 && padding != expected_padding {
        return Err(invalid(
            what,
            format!(
                "{padding} padding character(s) for {} digit(s)",
                digits.len()
            ),
        ));
    }

    let mut out = Vec::with_capacity(digits.len() / 4 * 3);
    for group in digits.chunks(4) {
        let mut accumulator: u32 = 0;
        for (offset, digit) in group.iter().enumerate() {
            accumulator |= (*digit as u32) << (18 - 6 * offset);
        }
        for offset in 0..group.len().saturating_sub(1) {
            out.push((accumulator >> (16 - 8 * offset)) as u8);
        }
    }
    Ok(out)
}

fn invalid(what: &str, reason: String) -> ServiceError {
    ServiceError::Invalid(format!("{what} is not base64: {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_group_length() {
        for length in 0..40usize {
            let bytes: Vec<u8> = (0..length).map(|index| (index * 7 + 3) as u8).collect();
            let text = encode(&bytes);
            let decoded = decode(&text, "test").expect("decodes");
            assert_eq!(decoded, bytes, "length {length}");
        }
    }

    #[test]
    fn accepts_whitespace_and_the_url_safe_alphabet() {
        let bytes = b"the quick brown fox";
        let standard = encode(bytes);
        let wrapped = format!("\n  {}\t\n", standard.replace('\n', ""));
        assert_eq!(decode(&wrapped, "test").expect("decodes"), bytes);

        let url_safe = encode(&[0xfb, 0xff, 0xbf]);
        assert!(url_safe.contains('+') || url_safe.contains('/'));
        let swapped = url_safe.replace('+', "-").replace('/', "_");
        assert_eq!(
            decode(&swapped, "test").expect("decodes"),
            vec![0xfb, 0xff, 0xbf]
        );
    }

    #[test]
    fn rejects_payloads_that_are_not_whole_groups() {
        // Six bits is not a byte: accepting it would silently return a prefix.
        assert!(decode("A", "test").is_err());
        assert!(decode("QUJDR", "test").is_err());
        assert!(decode("", "test").expect("empty decodes").is_empty());
    }

    #[test]
    fn rejects_misplaced_or_excess_padding() {
        assert!(decode("=TQ==", "test").is_err());
        assert!(decode("TQ=", "test").is_err());
        assert!(decode("TQ===", "test").is_err());
        assert!(decode("TQ==TQ==", "test").is_err());
        // Correct padding is accepted with and without the padding itself.
        assert_eq!(decode("TQ==", "test").expect("decodes"), b"M");
        assert_eq!(decode("TQ", "test").expect("decodes"), b"M");
        assert_eq!(decode("TWE=", "test").expect("decodes"), b"Ma");
        assert_eq!(decode("TWE", "test").expect("decodes"), b"Ma");
    }

    #[test]
    fn rejects_characters_outside_the_alphabet() {
        let error = decode("not base64 at all!", "inline map").expect_err("rejected");
        assert!(error.to_string().contains("inline map"), "{error}");
        assert!(error.to_string().contains("base64"), "{error}");
    }
}
