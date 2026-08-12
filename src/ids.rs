//! Message-ID normalization and filesystem-safe stems (design KD4).

use sha2::{Digest, Sha256};

/// Canonical Message-ID form for lookups, tools, front matter, roots, and stems.
///
/// Trims Unicode whitespace only; keeps angle brackets as-is after trim.
pub fn normalize_message_id(id: &str) -> String {
    id.trim().to_string()
}

/// Percent-encode a normalized id (RFC 3986 unreserved characters left alone).
///
/// Hex digits in escapes are uppercase (`%3C`), matching common path-encoding style.
pub fn percent_encode_id(normalized: &str) -> String {
    let mut out = String::with_capacity(normalized.len() * 3);
    for b in normalized.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Lowercase hex SHA-256 digest (64 chars, no `0x` prefix).
pub fn sha256_hex_lower(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Filename stem (no `.md`) for messages and threads.
///
/// Always normalizes first. If percent-encoded length &gt; 200, uses a 64-char
/// lowercase sha256 hex of the **normalized** id bytes.
pub fn file_stem_for_id(id: &str) -> String {
    let n = normalize_message_id(id);
    let enc = percent_encode_id(&n);
    if enc.len() > 200 {
        sha256_hex_lower(n.as_bytes())
    } else {
        enc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_ascii_and_preserves_brackets() {
        assert_eq!(normalize_message_id(" <abc@def.com>"), "<abc@def.com>");
        assert_eq!(normalize_message_id("<abc@def.com> "), "<abc@def.com>");
        assert_eq!(normalize_message_id("  <x@y>  "), "<x@y>");
    }

    #[test]
    fn normalize_trims_unicode_whitespace() {
        // U+00A0 NO-BREAK SPACE
        assert_eq!(
            normalize_message_id("\u{00A0}<id@host>\u{00A0}"),
            "<id@host>"
        );
    }

    #[test]
    fn file_stem_percent_encodes_typical_id() {
        assert_eq!(
            file_stem_for_id(" <abc@def.com>"),
            "%3Cabc%40def.com%3E"
        );
        // Same stem whether raw or already normalized.
        assert_eq!(
            file_stem_for_id("<abc@def.com>"),
            "%3Cabc%40def.com%3E"
        );
    }

    #[test]
    fn long_id_uses_sha256_lowercase_hex() {
        let long = format!("<{}@example.com>", "a".repeat(300));
        let stem = file_stem_for_id(&long);
        assert_eq!(stem.len(), 64);
        assert!(
            stem.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "expected lowercase hex, got {stem}"
        );
        assert_eq!(stem, sha256_hex_lower(normalize_message_id(&long).as_bytes()));
    }

    #[test]
    fn percent_encode_leaves_unreserved() {
        assert_eq!(percent_encode_id("abc-._~XYZ09"), "abc-._~XYZ09");
    }
}
