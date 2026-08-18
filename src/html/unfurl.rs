//! Open Graph / Twitter Card / description tags for classic link unfurls.

use super::links::encode_rel_href;
use std::path::Path;

/// Escape text for an HTML attribute value (double-quoted).
pub(super) fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Strip trailing slashes from an `http(s)` prefix. Returns `None` if the
/// value is not a usable absolute URL.
pub fn normalize_site_prefix(site_url: &str) -> Option<String> {
    let s = site_url.trim().trim_end_matches('/');
    if !(s.starts_with("http://") || s.starts_with("https://")) {
        return None;
    }
    if s == "http:" || s == "https:" || s == "http://" || s == "https://" {
        return None;
    }
    Some(s.to_string())
}

/// Absolute public URL for a markdown-relative page (`*.md` → `*.html`).
/// Path segments are encoded the same way as in-page hrefs.
pub fn absolute_page_url(site_url: &str, rel_md: &Path) -> Option<String> {
    let prefix = normalize_site_prefix(site_url)?;
    let rel_html = rel_md.with_extension("html");
    let encoded = encode_rel_href(&rel_html);
    if encoded.is_empty() {
        Some(prefix)
    } else {
        Some(format!("{prefix}/{encoded}"))
    }
}

/// `YYYY-MM-DD` → `YYYY-MM-DDT00:00:00Z`. Other values are ignored.
pub fn article_published_time(week_ending: Option<&str>) -> Option<String> {
    let w = week_ending.map(str::trim).filter(|s| !s.is_empty())?;
    if w.len() == 10
        && w.as_bytes()[4] == b'-'
        && w.as_bytes()[7] == b'-'
        && w.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
    {
        Some(format!("{w}T00:00:00Z"))
    } else {
        None
    }
}

fn http_url(s: &str) -> Option<&str> {
    let s = s.trim();
    if s.starts_with("http://") || s.starts_with("https://") {
        Some(s)
    } else {
        None
    }
}

/// `<head>` snippet: description, Open Graph, Twitter Card, optional canonical.
pub fn unfurl_meta_html(
    title: &str,
    description: &str,
    site_name: &str,
    og_type: &str,
    page_url: Option<&str>,
    og_image: Option<&str>,
    published_time: Option<&str>,
) -> String {
    let title_esc = escape_html(title);
    let desc_esc = escape_html(description);
    let site_esc = escape_html(site_name);
    let type_esc = escape_html(og_type);
    let image = og_image.and_then(http_url);
    let card = if image.is_some() {
        "summary_large_image"
    } else {
        "summary"
    };

    let mut out = String::new();
    out.push_str(&format!(
        "  <meta name=\"description\" content=\"{desc_esc}\">\n"
    ));
    out.push_str(&format!(
        "  <meta property=\"og:title\" content=\"{title_esc}\">\n"
    ));
    out.push_str(&format!(
        "  <meta property=\"og:description\" content=\"{desc_esc}\">\n"
    ));
    out.push_str(&format!(
        "  <meta property=\"og:type\" content=\"{type_esc}\">\n"
    ));
    out.push_str(&format!(
        "  <meta property=\"og:site_name\" content=\"{site_esc}\">\n"
    ));
    if let Some(url) = page_url.and_then(http_url) {
        let url_esc = escape_html(url);
        out.push_str(&format!("  <link rel=\"canonical\" href=\"{url_esc}\">\n"));
        out.push_str(&format!(
            "  <meta property=\"og:url\" content=\"{url_esc}\">\n"
        ));
    }
    if let Some(img) = image {
        let img_esc = escape_html(img);
        out.push_str(&format!(
            "  <meta property=\"og:image\" content=\"{img_esc}\">\n"
        ));
        out.push_str(&format!(
            "  <meta name=\"twitter:image\" content=\"{img_esc}\">\n"
        ));
    }
    if let Some(ts) = published_time.map(str::trim).filter(|s| !s.is_empty()) {
        let ts_esc = escape_html(ts);
        out.push_str(&format!(
            "  <meta property=\"article:published_time\" content=\"{ts_esc}\">\n"
        ));
    }
    out.push_str(&format!(
        "  <meta name=\"twitter:card\" content=\"{card}\">\n"
    ));
    out.push_str(&format!(
        "  <meta name=\"twitter:title\" content=\"{title_esc}\">\n"
    ));
    out.push_str(&format!(
        "  <meta name=\"twitter:description\" content=\"{desc_esc}\">\n"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn joins_and_encodes_thread_path() {
        let url = absolute_page_url(
            "http://host/weekly/",
            Path::new("2026-07-20/thread/%3Cfoo%40bar.com%3E.md"),
        )
        .unwrap();
        assert_eq!(
            url,
            "http://host/weekly/2026-07-20/thread/%253Cfoo%2540bar.com%253E.html"
        );
    }

    #[test]
    fn joins_root_index() {
        let url = absolute_page_url("https://ex/weekly", Path::new("index.md")).unwrap();
        assert_eq!(url, "https://ex/weekly/index.html");
    }

    #[test]
    fn rejects_non_http_prefix() {
        assert!(absolute_page_url("/", Path::new("index.md")).is_none());
        assert!(absolute_page_url("weekly/", Path::new("index.md")).is_none());
    }

    #[test]
    fn omits_url_and_image_when_unset() {
        let html = unfurl_meta_html("Title", "Desc", "Site", "website", None, None, None);
        assert!(html.contains("property=\"og:title\""));
        assert!(html.contains("property=\"og:description\""));
        assert!(html.contains("name=\"twitter:card\" content=\"summary\""));
        assert!(!html.contains("og:url"));
        assert!(!html.contains("rel=\"canonical\""));
        assert!(!html.contains("og:image"));
        assert!(!html.contains("article:published_time"));
    }

    #[test]
    fn emits_canonical_image_and_article_time() {
        let html = unfurl_meta_html(
            "Title",
            "Desc",
            "Site",
            "article",
            Some("https://ex/weekly/2026-07-20/index.html"),
            Some("https://ex/og.png"),
            Some("2026-07-20T00:00:00Z"),
        );
        assert!(
            html.contains("rel=\"canonical\" href=\"https://ex/weekly/2026-07-20/index.html\"")
        );
        assert!(
            html.contains(
                "property=\"og:url\" content=\"https://ex/weekly/2026-07-20/index.html\""
            )
        );
        assert!(html.contains("property=\"og:image\" content=\"https://ex/og.png\""));
        assert!(html.contains("name=\"twitter:card\" content=\"summary_large_image\""));
        assert!(
            html.contains("property=\"article:published_time\" content=\"2026-07-20T00:00:00Z\"")
        );
        assert!(html.contains("property=\"og:type\" content=\"article\""));
    }

    #[test]
    fn escapes_quotes_in_meta() {
        let html = unfurl_meta_html(
            r#"Say "hello""#,
            r#"A & B <tag>"#,
            "Site",
            "website",
            None,
            None,
            None,
        );
        assert!(html.contains("Say &quot;hello&quot;"));
        assert!(html.contains("A &amp; B &lt;tag&gt;"));
        assert!(!html.contains(r#"content="Say "hello""#));
    }

    #[test]
    fn published_time_only_for_iso_date() {
        assert_eq!(
            article_published_time(Some("2026-07-20")).as_deref(),
            Some("2026-07-20T00:00:00Z")
        );
        assert_eq!(article_published_time(Some("soon")), None);
        assert_eq!(article_published_time(None), None);
    }
}
