//! Markdown → HTML body fragment (no SSG). Rewrites relative `.md` links to `.html`.

use pulldown_cmark::{Event, Options, Parser, Tag, html};

const FALLBACK_TITLE: &str = "Mailing List Weekly Summaries";

const DESC_MAX_CHARS: usize = 240;

#[derive(Debug, Default, Clone)]
pub struct FrontMatter {
    pub title: Option<String>,
    pub headline: Option<String>,
    pub subject: Option<String>,
    pub week_ending: Option<String>,
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
        } else if let Some(rest) = line.strip_prefix("subject:") {
            fm.subject = Some(unquote_yaml_scalar(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("week_ending:") {
            fm.week_ending = Some(unquote_yaml_scalar(rest.trim()));
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
pub fn page_title(fm: &FrontMatter, body_md: &str, fallback: &str) -> String {
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
    first_atx_h1(body_md).unwrap_or_else(|| {
        let fb = fallback.trim();
        if fb.is_empty() {
            FALLBACK_TITLE.to_string()
        } else {
            fb.to_string()
        }
    })
}

/// Unfurl / meta description: first candidate that is non-empty and not equal
/// to `title`, else first non-empty candidate, else `site_title`.
///
/// Candidates, in order: front-matter `headline`, first substantial paragraph,
/// front-matter `subject`.
pub fn page_description(fm: &FrontMatter, body_md: &str, title: &str, site_title: &str) -> String {
    let headline = nonempty_trimmed(fm.headline.as_deref());
    let paragraph = first_substantial_paragraph(body_md);
    let subject = nonempty_trimmed(fm.subject.as_deref());
    let candidates = [headline, paragraph.as_deref(), subject];

    let title = title.trim();
    let pick = candidates
        .into_iter()
        .flatten()
        .find(|s| *s != title)
        .or_else(|| candidates.into_iter().flatten().next());

    let raw = pick.unwrap_or_else(|| {
        let fb = site_title.trim();
        if fb.is_empty() { FALLBACK_TITLE } else { fb }
    });
    truncate_desc(raw, DESC_MAX_CHARS)
}

fn nonempty_trimmed(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

fn first_substantial_paragraph(md: &str) -> Option<String> {
    let mut para = String::new();
    let flush = |buf: &mut String| -> Option<String> {
        let stripped = strip_inline_markdown(buf);
        buf.clear();
        if is_usable_paragraph(&stripped) {
            Some(stripped)
        } else {
            None
        }
    };

    for line in md.lines() {
        let t = line.trim();
        if t.is_empty() {
            if !para.is_empty()
                && let Some(s) = flush(&mut para)
            {
                return Some(s);
            }
            continue;
        }
        if is_block_skip(t) {
            if !para.is_empty()
                && let Some(s) = flush(&mut para)
            {
                return Some(s);
            }
            continue;
        }
        if let Some(item) = list_item_text(t) {
            if para.is_empty() {
                let stripped = strip_inline_markdown(item);
                if is_usable_paragraph(&stripped) {
                    return Some(stripped);
                }
            }
            continue;
        }
        if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(t);
    }
    if !para.is_empty() {
        return flush(&mut para);
    }
    None
}

fn is_block_skip(t: &str) -> bool {
    t.starts_with('#') || t == "---" || t == "***" || t == "___" || t == "* * *"
}

fn list_item_text(t: &str) -> Option<&str> {
    t.strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))
}

fn is_usable_paragraph(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    !is_week_ending_only(s)
}

fn is_week_ending_only(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("week ending ") else {
        return false;
    };
    let rest = rest.trim();
    rest.len() == 10
        && rest.as_bytes()[4] == b'-'
        && rest.as_bytes()[7] == b'-'
        && rest.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

fn strip_inline_markdown(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut tmp = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if (chars[i] == '!' && i + 1 < chars.len() && chars[i + 1] == '[') || chars[i] == '[' {
            let start = if chars[i] == '!' { i + 1 } else { i };
            if let Some((text, next)) = parse_md_link(&chars, start) {
                tmp.push_str(&text);
                i = next;
                continue;
            }
        }
        tmp.push(chars[i]);
        i += 1;
    }
    let stripped = tmp.replace("**", "").replace("__", "").replace('`', "");
    let stripped = stripped.replace('*', "");
    collapse_ws(&stripped)
}

fn parse_md_link(chars: &[char], start: usize) -> Option<(String, usize)> {
    if start >= chars.len() || chars[start] != '[' {
        return None;
    }
    let mut i = start + 1;
    let mut text = String::new();
    while i < chars.len() && chars[i] != ']' {
        text.push(chars[i]);
        i += 1;
    }
    if i >= chars.len() || chars[i] != ']' {
        return None;
    }
    i += 1;
    if i >= chars.len() || chars[i] != '(' {
        return None;
    }
    i += 1;
    while i < chars.len() && chars[i] != ')' {
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }
    Some((text, i + 1))
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

fn truncate_desc(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let mut n = 0;
    let mut last_space = None;
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if n >= max {
            break;
        }
        if c.is_whitespace() {
            last_space = Some(i);
        }
        end = i + c.len_utf8();
        n += 1;
    }
    let cut = last_space.filter(|&i| i > 0).unwrap_or(end);
    format!("{}…", s[..cut].trim_end())
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

pub fn convert_markdown_document(md: &str, fallback_title: &str) -> (FrontMatter, String, String) {
    let (fm, body) = strip_front_matter(md);
    let title = page_title(&fm, body, fallback_title);
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
        let lore = "https://lore.kernel.org/list/abc@def/";
        assert_eq!(rewrite_internal_href(lore), lore);
        assert_eq!(rewrite_internal_href("mailto:a@b"), "mailto:a@b");
        assert_eq!(rewrite_internal_href("#section"), "#section");
    }

    #[test]
    fn converted_body_rewrites_md_hrefs() {
        let html = markdown_to_html_body("[Week](2026-07-20/index.md)");
        assert!(html.contains("href=\"2026-07-20/index.html\""), "{html}");
        assert!(!html.contains(".md\""));
        let html = markdown_to_html_body("[x](https://lore.kernel.org/list/id/)");
        assert!(html.contains("https://lore.kernel.org/list/id/"), "{html}");
    }

    #[test]
    fn strips_front_matter_and_uses_headline() {
        let md = "---\nheadline: \"Quiet week\"\nempty: false\n---\n\n# Quiet week\n\nHello.\n";
        let (fm, title, html) = convert_markdown_document(md, FALLBACK_TITLE);
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
        let (_, title, _) = convert_markdown_document(md, FALLBACK_TITLE);
        assert_eq!(title, "Thread title");
    }

    #[test]
    fn raw_html_is_escaped() {
        let html = markdown_to_html_body("before <script>alert(1)</script> after");
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }

    #[test]
    fn description_skips_headline_when_it_is_the_title() {
        let md = "---\nheadline: \"Quiet week\"\nweek_ending: \"2026-07-20\"\n---\n\n# Quiet week\n\n*Week ending 2026-07-20*\n\nTrond submitted a pull of twelve fixes.\n";
        let (fm, title, _) = convert_markdown_document(md, FALLBACK_TITLE);
        let desc = page_description(&fm, strip_front_matter(md).1, &title, FALLBACK_TITLE);
        assert_eq!(fm.week_ending.as_deref(), Some("2026-07-20"));
        assert_eq!(title, "Quiet week");
        assert_eq!(desc, "Trond submitted a pull of twelve fixes.");
        assert_ne!(desc, title);
    }

    #[test]
    fn description_uses_subject_when_no_usable_paragraph() {
        let md = "---\nsubject: \"[GIT PULL] client bugfixes\"\n---\n\n# Client Bugfixes\n\n## Summary\n";
        let (fm, title, _) = convert_markdown_document(md, FALLBACK_TITLE);
        let desc = page_description(&fm, strip_front_matter(md).1, &title, FALLBACK_TITLE);
        assert_eq!(desc, "[GIT PULL] client bugfixes");
    }

    #[test]
    fn description_strips_markdown_links() {
        let md = "See [the thread](https://lore.kernel.org/list/id/) on the list.\n";
        let (fm, title, _) = convert_markdown_document(md, FALLBACK_TITLE);
        let desc = page_description(&fm, md, &title, FALLBACK_TITLE);
        assert!(desc.contains("the thread"), "{desc}");
        assert!(!desc.contains("]("), "{desc}");
        assert!(!desc.contains("https://"), "{desc}");
    }

    #[test]
    fn description_truncates_long_paragraph() {
        let word = "abcdefghij ";
        let long = word.repeat(30); // 330 chars
        let md = format!("# Title\n\n{long}\n");
        let (fm, title, _) = convert_markdown_document(&md, FALLBACK_TITLE);
        let desc = page_description(&fm, strip_front_matter(&md).1, &title, FALLBACK_TITLE);
        assert!(desc.ends_with('…'), "{desc}");
        assert!(
            desc.chars().count() <= DESC_MAX_CHARS + 1,
            "{}",
            desc.chars().count()
        );
        assert!(!desc.contains("headline:"));
    }

    #[test]
    fn description_falls_back_to_site_title() {
        let md = "# Heading only\n";
        let (fm, title, _) = convert_markdown_document(md, FALLBACK_TITLE);
        assert_eq!(title, "Heading only");
        let desc = page_description(&fm, strip_front_matter(md).1, &title, "Weekly Summaries");
        assert_eq!(desc, "Weekly Summaries");
    }
}
