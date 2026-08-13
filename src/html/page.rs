//! HTML5 document shell with relative CSS and breadcrumb hrefs.

use std::path::{Component, Path};

const SITE_TITLE: &str = "NFS Weekly Summaries";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageKind {
    Root,
    Week { week: String },
    Thread { week: String },
    Other,
}

/// Depth of the file's parent directory (normal components only).
pub fn path_depth(rel: &Path) -> usize {
    rel.parent()
        .map(|p| {
            p.components()
                .filter(|c| matches!(c, Component::Normal(_)))
                .count()
        })
        .unwrap_or(0)
}

/// Relative href to `{html_root}/style.css` from a markdown-relative path.
pub fn stylesheet_href(rel_md: &Path) -> String {
    let depth = path_depth(rel_md);
    if depth == 0 {
        "style.css".to_string()
    } else {
        format!("{}style.css", "../".repeat(depth))
    }
}

pub fn classify_page(rel_md: &Path) -> PageKind {
    let comps: Vec<&str> = rel_md
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    match comps.as_slice() {
        ["index.md"] => PageKind::Root,
        [week, "index.md"] => PageKind::Week {
            week: (*week).to_string(),
        },
        [week, "thread", _] => PageKind::Thread {
            week: (*week).to_string(),
        },
        _ => PageKind::Other,
    }
}

fn escape_html(s: &str) -> String {
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

fn crumb_items(kind: &PageKind) -> Vec<(Option<String>, String)> {
    match kind {
        PageKind::Root => Vec::new(),
        PageKind::Week { .. } => {
            vec![(Some("../index.html".into()), "Summaries".into())]
        }
        PageKind::Thread { week } => vec![
            (Some("../../index.html".into()), "Summaries".into()),
            (Some("../index.html".into()), format!("Week ending {week}")),
        ],
        PageKind::Other => {
            // Best-effort link to the catalog; still an explicit index.html.
            vec![(Some("index.html".into()), "Summaries".into())]
        }
    }
}

/// Wrap a body fragment in a full HTML5 document.
pub fn wrap_page(title: &str, rel_md: &Path, body: &str) -> String {
    let css = stylesheet_href(rel_md);
    let kind = classify_page(rel_md);
    let title_esc = escape_html(title);
    let site_esc = escape_html(SITE_TITLE);
    let home_href = match path_depth(rel_md) {
        0 => "index.html".to_string(),
        n => format!("{}index.html", "../".repeat(n)),
    };

    let crumbs = crumb_items(&kind);
    let mut nav = String::new();
    if !crumbs.is_empty() {
        nav.push_str("      <ol class=\"crumbs\">\n");
        for (i, (href, label)) in crumbs.iter().enumerate() {
            if i > 0 {
                nav.push_str("        <li class=\"crumbs__sep\" aria-hidden=\"true\">/</li>\n");
            }
            let label_esc = escape_html(label);
            match href {
                Some(h) => nav.push_str(&format!(
                    "        <li><a href=\"{}\">{label_esc}</a></li>\n",
                    escape_html(h)
                )),
                None => nav.push_str(&format!("        <li>{label_esc}</li>\n")),
            }
        }
        nav.push_str("      </ol>\n");
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title_esc}</title>
  <link rel="stylesheet" href="{css}">
</head>
<body>
  <header class="site-header">
    <div class="site-header__inner">
      <p class="site-title"><a href="{home_href}">{site_esc}</a></p>
{nav}    </div>
  </header>
  <main class="content">
{body}  </main>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn stylesheet_href_by_depth() {
        assert_eq!(stylesheet_href(Path::new("index.md")), "style.css");
        assert_eq!(
            stylesheet_href(Path::new("2026-07-20/index.md")),
            "../style.css"
        );
        assert_eq!(
            stylesheet_href(Path::new("2026-07-20/thread/foo.md")),
            "../../style.css"
        );
    }

    #[test]
    fn crumbs_use_explicit_index_html() {
        let week = wrap_page("Week", Path::new("2026-07-20/index.md"), "<p>x</p>\n");
        assert!(week.contains("href=\"../index.html\""), "{week}");
        assert!(!week.contains("href=\"../\""), "{week}");
        assert!(week.contains("href=\"../style.css\""), "{week}");

        let thread = wrap_page(
            "Thread",
            Path::new("2026-07-20/thread/foo.md"),
            "<p>x</p>\n",
        );
        assert!(thread.contains("href=\"../../index.html\""), "{thread}");
        assert!(thread.contains("href=\"../index.html\""), "{thread}");
        assert!(thread.contains("href=\"../../style.css\""), "{thread}");
        assert!(!thread.contains("href=\"/"), "{thread}");
    }

    #[test]
    fn classifies_layout_paths() {
        assert_eq!(classify_page(Path::new("index.md")), PageKind::Root);
        assert_eq!(
            classify_page(Path::new("2026-07-20/index.md")),
            PageKind::Week {
                week: "2026-07-20".into()
            }
        );
        assert_eq!(
            classify_page(Path::new("2026-07-20/thread/abc.md")),
            PageKind::Thread {
                week: "2026-07-20".into()
            }
        );
    }
}
