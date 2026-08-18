//! Public lore.kernel.org links for Message-IDs.

use crate::ids::normalize_message_id;

/// Default lore archive prefix when `lore_base_url` is omitted.
/// Override per list, e.g. `https://lore.kernel.org/your-list/`.
pub const DEFAULT_LORE_BASE: &str = "https://lore.kernel.org/";

/// Build a lore permalink for a Message-ID.
///
/// Lore URLs use the Message-ID **without** surrounding angle brackets, e.g.
/// `https://lore.kernel.org/your-list/20260720-tcp-read-sock-v2-6-29545d034f3c@kernel.org/`
///
/// `lore_base` is typically `https://lore.kernel.org/your-list/` (with or without
/// trailing slash). The id may be raw DB form (leading space) or normalized.
pub fn lore_url_for_message_id(lore_base: &str, message_id: &str) -> String {
    let bare = lore_message_id_path_segment(message_id);
    let base = lore_base.trim_end_matches('/');
    format!("{base}/{bare}/")
}

/// Path segment lore uses for a Message-ID: normalized, without `<` / `>`.
pub fn lore_message_id_path_segment(message_id: &str) -> String {
    let n = normalize_message_id(message_id);
    n.trim_start_matches('<').trim_end_matches('>').to_string()
}

/// Markdown link to a lore message: `[label](url)`.
#[allow(dead_code)] // used when writing thread/*.md (PR5+)
pub fn lore_markdown_link(lore_base: &str, message_id: &str, label: &str) -> String {
    let url = lore_url_for_message_id(lore_base, message_id);
    format!("[{label}]({url})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lore_url_strips_brackets_and_space() {
        let url = lore_url_for_message_id(
            "https://lore.kernel.org/list/",
            " <20260720-tcp-read-sock-v2-6-29545d034f3c@kernel.org>",
        );
        assert_eq!(
            url,
            "https://lore.kernel.org/list/20260720-tcp-read-sock-v2-6-29545d034f3c@kernel.org/"
        );
    }

    #[test]
    fn lore_url_base_without_trailing_slash() {
        let url = lore_url_for_message_id("https://lore.kernel.org/list", "<abc@def.com>");
        assert_eq!(url, "https://lore.kernel.org/list/abc@def.com/");
    }

    #[test]
    fn markdown_link() {
        let md = lore_markdown_link(DEFAULT_LORE_BASE, "<x@y>", "2026-07-18 Alice — subject");
        assert_eq!(
            md,
            "[2026-07-18 Alice — subject](https://lore.kernel.org/x@y/)"
        );
    }
}
