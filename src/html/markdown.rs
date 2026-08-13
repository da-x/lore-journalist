//! Markdown → HTML body fragment (no SSG). Rewrites relative `.md` links to `.html`.

use pulldown_cmark::{Event, Options, Parser, Tag, html};

const FALLBACK_TITLE: &str = "NFS Weekly Summaries";

#[derive(Debug, Default, Clone)]
pub struct FrontMatter {
    pub title: Option<String>,
    pub headline: Option<String>,
}

/// Split leading YAML front matter from the markdown body.
pub fn strip_front_matter(md: &str) -> (FrontMatter, &str) {
    let rest = match md.strip_prefix("---") {
        Some(r) => r,
        None => return (FrontMatter::default(), md),
    };
    let rest = if let Some(r) = rest.strip_prefix("\r\n") {
        r
    } else if let Some(r) = rest.strip_prefix('\n') {
        r
    } else {
        return (FrontMatter::default(), md);
    };

    let close = rest
        .find("\n---\n")
        .map(|i| (i, 5))
        .or_else(|| rest.find("\n---\r\n").map(|i| (i, 6)))
        .or_else(|| rest.find("\r\n---\r\n").map(|i| (i, 8)))
        .or_else(|| rest.find("\r\n---\n").map(|i| (i, 7)));

    let Some((end, delim_len)) = close else {
        return (FrontMatter::default(), md);
    };

    let yaml = &rest[..end];
    let body = &rest[end + delim_len..];
    (parse_front_matter(yaml), body)
}

fn parse_front_matter(yaml: &str) -> FrontMatter {
    let mut fm = FrontMatter::default();
    for line in yaml.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("title:") {
            fm.title = Some(unquote_yaml_scalar(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("headline:") {
            fm.headline = Some(unquote_yaml_scalar(rest.trim()));
        }
    }
    fm
}

fn unquote_yaml_scalar(v: &str) -> String {
    if let Some(inner) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        inner.replace("\\\"", "\"").replace("\\\\", "\\")
    } else if let Some(inner) = v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        inner.to_string()
    } else {
        v.to_string()
    }
}

/// Rewrite a markdown href for HTML output. Relative `*.md` → `*.html`; keep
/// schemes, hashes, and directory-less names as-is. Never collapse `index.html`
/// to a trailing-slash directory URL.
pub fn rewrite_internal_href(href: &str) -> String {
    if href.is_empty() || href.starts_with('#') {
        return href.to_string();
    }
    if let Some(colon) = href.find(':') {
        let scheme = &href[..colon];
        if !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphabetic() || b == b'+' || b == b'.' || b == b'-')
        {
            return href.to_string();
        }
    }

    let (path_and_query, hash) = match href.find('#') {
        Some(i) => (&href[..i], &href[i..]),
        None => (href, ""),
    };
    let (path, query) = match path_and_query.find('?') {
        Some(i) => (&path_and_query[..i], &path_and_query[i..]),
        None => (path_and_query, ""),
    };

    let new_path = if let Some(stem) = path.strip_suffix(".md") {
        format!("{stem}.html")
    } else {
        path.to_string()
    };
    format!("{new_path}{query}{hash}")
}

fn first_atx_h1(md: &str) -> Option<String> {
    for line in md.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
}

/// Page title: front-matter `title`, then `headline`, then first ATX H1, then fallback.
pub fn page_title(fm: &FrontMatter, body_md: &str) -> String {
    if let Some(t) = fm.title.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return t.to_string();
    }
    if let Some(t) = fm
        .headline
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return t.to_string();
    }
    first_atx_h1(body_md).unwrap_or_else(|| FALLBACK_TITLE.to_string())
}

/// Convert markdown body to an HTML fragment. Raw HTML from the source is
/// emitted as escaped text (via Text events), not live tags.
pub fn markdown_to_html_body(md: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);

    let parser = Parser::new_ext(md, options).map(|event| match event {
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: rewrite_internal_href(&dest_url).into(),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: rewrite_internal_href(&dest_url).into(),
            title,
            id,
        }),
        // Drop raw HTML to Text so the writer escapes it.
        Event::Html(s) | Event::InlineHtml(s) => Event::Text(s),
        other => other,
    });

    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

pub fn convert_markdown_document(md: &str) -> (FrontMatter, String, String) {
    let (fm, body) = strip_front_matter(md);
    let title = page_title(&fm, body);
    let html = markdown_to_html_body(body);
    (fm, title, html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_code_becomes_code_element() {
        let html = markdown_to_html_body("Use `[start, end)` as the window.");
        assert!(
            html.contains("<code>[start, end)</code>"),
            "inline code missing: {html}"
        );
        assert!(!html.contains("`<code>"));
    }

    #[test]
    fn rewrites_relative_md_links() {
        assert_eq!(rewrite_internal_href("thread/foo.md"), "thread/foo.html");
        assert_eq!(
            rewrite_internal_href("2026-07-20/index.md"),
            "2026-07-20/index.html"
        );
        assert_eq!(
            rewrite_internal_href("../index.md#top"),
            "../index.html#top"
        );
        assert_eq!(rewrite_internal_href("index.md"), "index.html");
    }

    #[test]
    fn does_not_collapse_index_to_directory_url() {
        let href = rewrite_internal_href("2026-07-20/index.md");
        assert!(!href.ends_with('/'));
        assert!(!href.ends_with("2026-07-20"));
        assert!(href.ends_with("index.html"));
    }

    #[test]
    fn leaves_external_and_hash_links() {
        let lore = "https://lore.kernel.org/linux-nfs/abc@def/";
        assert_eq!(rewrite_internal_href(lore), lore);
        assert_eq!(rewrite_internal_href("mailto:a@b"), "mailto:a@b");
        assert_eq!(rewrite_internal_href("#section"), "#section");
    }

    #[test]
    fn converted_body_rewrites_md_hrefs() {
        let html = markdown_to_html_body("[Week](2026-07-20/index.md)");
        assert!(html.contains("href=\"2026-07-20/index.html\""), "{html}");
        assert!(!html.contains(".md\""));
        let html = markdown_to_html_body("[x](https://lore.kernel.org/linux-nfs/id/)");
        assert!(
            html.contains("https://lore.kernel.org/linux-nfs/id/"),
            "{html}"
        );
    }

    #[test]
    fn strips_front_matter_and_uses_headline() {
        let md = "---\nheadline: \"Quiet week\"\nempty: false\n---\n\n# Quiet week\n\nHello.\n";
        let (fm, title, html) = convert_markdown_document(md);
        assert_eq!(fm.headline.as_deref(), Some("Quiet week"));
        assert_eq!(title, "Quiet week");
        assert!(!html.contains("headline:"));
        assert!(!html.contains("---"));
        assert!(html.contains("<h1>Quiet week</h1>"));
        assert!(html.contains("<p>Hello.</p>"));
    }

    #[test]
    fn title_prefers_title_field() {
        let md = "---\ntitle: \"Thread title\"\nheadline: \"Week headline\"\n---\n\n# Heading\n";
        let (_, title, _) = convert_markdown_document(md);
        assert_eq!(title, "Thread title");
    }

    #[test]
    fn raw_html_is_escaped() {
        let html = markdown_to_html_body("before <script>alert(1)</script> after");
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }
}
