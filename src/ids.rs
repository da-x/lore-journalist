//! Message-ID normalization and filesystem-safe stems (design KD4).

use sha2::{Digest, Sha256};

/// Canonical Message-ID form for lookups, tools, front matter, roots, and stems.
///
/// Trims Unicode whitespace, then strips trailing list junk (commas, semicolons,
/// quotes, pipes) that appears in some `References` / LLM outputs — e.g.
/// `"<id@host>,"` from broken mail clients. Keeps angle brackets as-is after
/// cleanup. Does **not** alter the interior of a well-formed id.
pub fn normalize_message_id(id: &str) -> String {
    let mut s = id.trim().to_string();
    // Some MUAs append trailing punctuation to Message-IDs in References chains;
    // without this, the same discussion splits into two roots (`<id>` vs `<id>,`).
    loop {
        let before = s.clone();
        s = s
            .trim_matches(|c: char| {
                c.is_whitespace()
                    || c == ','
                    || c == ';'
                    || c == '"'
                    || c == '\''
                    || c == '`'
                    || c == '|'
            })
            .to_string();
        if s.ends_with("...") {
            s.truncate(s.len() - 3);
            s = s.trim_end().to_string();
        }
        if s == before {
            break;
        }
    }
    s
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
    fn normalize_strips_trailing_comma_junk() {
        // Observed in Neil Brown replies' References chains for this corpus.
        assert_eq!(
            normalize_message_id("<20260108004016.3907158-1-cel@kernel.org>,"),
            "<20260108004016.3907158-1-cel@kernel.org>"
        );
        assert_eq!(
            normalize_message_id(" <abc@def.com>, "),
            "<abc@def.com>"
        );
        assert_eq!(normalize_message_id("<a@b>;"), "<a@b>");
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
