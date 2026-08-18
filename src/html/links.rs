//! Post-generation href fixup.
//!
//! Thread filenames are percent-encoded Message-IDs (`%3C…%40…%3E.html`).
//! Putting that name in an `href` as-is makes the browser decode `%3C` / `%40`
//! / `%3E` and request a file that does not exist. After every HTML page is
//! written we rewrite relative hrefs so each path segment is URL-encoded to
//! match a file that is actually on disk.

use crate::ids::{file_stem_for_id, percent_encode_id};
use crate::outputs::write_atomic;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// Rewrite relative `href`s in every generated HTML page so they resolve to
/// files that exist under `html_root`.
pub fn fix_html_links(html_root: &Path, known_rel: &[PathBuf]) -> Result<()> {
    let known: HashSet<PathBuf> = known_rel.iter().cloned().collect();
    for rel in known_rel {
        if rel.extension().and_then(|e| e.to_str()) != Some("html") {
            continue;
        }
        let path = html_root.join(rel);
        let src = std::fs::read_to_string(&path)
            .with_context(|| format!("read generated html {}", path.display()))?;
        let rewritten = rewrite_page_hrefs(&src, rel, &known);
        if rewritten != src {
            write_atomic(&path, &rewritten)
                .with_context(|| format!("rewrite hrefs in {}", path.display()))?;
        }
    }
    Ok(())
}

fn rewrite_page_hrefs(html: &str, page_rel: &Path, known: &HashSet<PathBuf>) -> String {
    let mut out = String::with_capacity(html.len() + 32);
    let mut rest = html;
    while let Some(idx) = rest.find("href=\"") {
        out.push_str(&rest[..idx]);
        out.push_str("href=\"");
        rest = &rest[idx + 6..];
        match rest.find('"') {
            Some(end) => {
                let href = &rest[..end];
                out.push_str(&rewrite_href(href, page_rel, known));
                out.push('"');
                rest = &rest[end + 1..];
            }
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Map one href onto a known generated file, then emit a URL-safe relative path.
pub fn rewrite_href(href: &str, page_rel: &Path, known: &HashSet<PathBuf>) -> String {
    if leave_href_alone(href) {
        return href.to_string();
    }
    let (path_part, suffix) = split_query_hash(href);
    let path_part = match path_part.strip_suffix(".md") {
        Some(stem) => format!("{stem}.html"),
        None => path_part.to_string(),
    };
    if path_part.is_empty() {
        return href.to_string();
    }

    let page_dir = page_rel.parent().unwrap_or(Path::new(""));
    let joined = page_dir.join(&path_part);
    let Some(resolved) = normalize_rel(&joined) else {
        return href.to_string();
    };
    let Some(actual) = match_known(&resolved, known) else {
        return href.to_string();
    };

    let rel = relative_from(page_dir, &actual);
    format!("{}{suffix}", encode_rel_href(&rel))
}

fn leave_href_alone(href: &str) -> bool {
    if href.is_empty() || href.starts_with('#') {
        return true;
    }
    if let Some(colon) = href.find(':') {
        let scheme = &href[..colon];
        if !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphabetic() || b == b'+' || b == b'.' || b == b'-')
        {
            return true;
        }
    }
    false
}

fn split_query_hash(href: &str) -> (&str, &str) {
    let cut = href.find(['?', '#']).unwrap_or(href.len());
    (&href[..cut], &href[cut..])
}

fn normalize_rel(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::Normal(s) => out.push(s),
            _ => return None,
        }
    }
    Some(out)
}

fn map_file_name(path: &Path, f: impl FnOnce(&str) -> String) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    Some(path.with_file_name(f(name)))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(h) = from_hex(bytes[i + 1])
            && let Some(l) = from_hex(bytes[i + 2])
        {
            out.push((h << 4) | l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn encode_component(s: &str) -> String {
    percent_encode_id(s)
}

fn match_known(resolved: &Path, known: &HashSet<PathBuf>) -> Option<PathBuf> {
    if known.contains(resolved) {
        return Some(resolved.to_path_buf());
    }
    if let Some(enc) = map_file_name(resolved, |n| encode_component(n))
        && known.contains(&enc)
    {
        return Some(enc);
    }
    if let Some(dec) = map_file_name(resolved, |n| percent_decode(n))
        && known.contains(&dec)
    {
        return Some(dec);
    }

    let name = resolved.file_name()?.to_str()?;
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s, format!(".{e}")),
        None => (name, String::new()),
    };
    for candidate in [stem, &percent_decode(stem)] {
        let fs = file_stem_for_id(candidate);
        let cand = resolved.with_file_name(format!("{fs}{ext}"));
        if known.contains(&cand) {
            return Some(cand);
        }
    }
    None
}

fn relative_from(from_dir: &Path, to_file: &Path) -> PathBuf {
    let from: Vec<_> = from_dir
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_os_string()),
            _ => None,
        })
        .collect();
    let to: Vec<_> = to_file
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_os_string()),
            _ => None,
        })
        .collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for c in &to[common..] {
        out.push(c);
    }
    if out.as_os_str().is_empty()
        && let Some(name) = to_file.file_name()
    {
        out.push(name);
    }
    out
}

/// Percent-encode each path segment of a relative path (RFC 3986 unreserved
/// left alone). Thread filenames that already contain `%3C` become `%253C`.
pub fn encode_rel_href(rel: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in rel.components() {
        match c {
            Component::ParentDir => parts.push("..".into()),
            Component::CurDir => parts.push(".".into()),
            Component::Normal(s) => parts.push(encode_component(&s.to_string_lossy())),
            _ => {}
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn encodes_percent_in_existing_filename() {
        let stem = "%3C57c6f8f6464f7ba0c0455875d4c53a0f9bf01a2c.camel%40kernel.org%3E.html";
        let file = format!("2026-01-15/thread/{stem}");
        let k = known(&["2026-01-15/index.html", &file]);
        let page = Path::new("2026-01-15/index.html");
        let href = format!("thread/{stem}");
        let out = rewrite_href(&href, page, &k);
        assert_eq!(
            out,
            "thread/%253C57c6f8f6464f7ba0c0455875d4c53a0f9bf01a2c.camel%2540kernel.org%253E.html"
        );
        assert!(!out.contains("thread/%3C"), "{out}");
    }

    #[test]
    fn maps_raw_message_id_href_to_encoded_file() {
        let file = "2026-01-15/thread/%3Cfoo%40bar.com%3E.html";
        let k = known(&["2026-01-15/index.html", file]);
        let page = Path::new("2026-01-15/index.html");
        let out = rewrite_href("thread/<foo@bar.com>.html", page, &k);
        assert_eq!(out, "thread/%253Cfoo%2540bar.com%253E.html");
    }

    #[test]
    fn maps_raw_md_link_to_encoded_file() {
        let file = "2026-01-15/thread/%3Cfoo%40bar.com%3E.html";
        let k = known(&[file]);
        let page = Path::new("2026-01-15/index.html");
        let out = rewrite_href("thread/<foo@bar.com>.md", page, &k);
        assert_eq!(out, "thread/%253Cfoo%2540bar.com%253E.html");
    }

    #[test]
    fn leaves_external_and_plain_paths() {
        let k = known(&["index.html", "2026-01-15/index.html", "style.css"]);
        let page = Path::new("index.html");
        assert_eq!(
            rewrite_href("https://lore.kernel.org/list/x/", page, &k),
            "https://lore.kernel.org/list/x/"
        );
        assert_eq!(
            rewrite_href("2026-01-15/index.html", page, &k),
            "2026-01-15/index.html"
        );
        assert_eq!(rewrite_href("#top", page, &k), "#top");
    }

    #[test]
    fn preserves_hash_on_fixed_href() {
        let file = "2026-01-15/thread/%3Cfoo%40bar.com%3E.html";
        let k = known(&[file]);
        let page = Path::new("2026-01-15/index.html");
        let out = rewrite_href("thread/%3Cfoo%40bar.com%3E.html#sec", page, &k);
        assert_eq!(out, "thread/%253Cfoo%2540bar.com%253E.html#sec");
    }

    #[test]
    fn rewrites_hrefs_in_page_html() {
        let file = "2026-01-15/thread/%3Cfoo%40bar.com%3E.html";
        let k = known(&["2026-01-15/index.html", file]);
        let html = r#"<a href="thread/%3Cfoo%40bar.com%3E.html">x</a>"#;
        let out = rewrite_page_hrefs(html, Path::new("2026-01-15/index.html"), &k);
        assert!(
            out.contains("href=\"thread/%253Cfoo%2540bar.com%253E.html\""),
            "{out}"
        );
    }
}
