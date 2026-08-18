use crate::lore::DEFAULT_LORE_BASE;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub openai: OpenAIConfig,
    pub git_repo_path: String,
    pub db_path: String,
    /// Legacy site base path (Hugo); not used for lore message links.
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Lore list archive prefix for message citations, e.g.
    /// `https://lore.kernel.org/your-list/`.
    #[serde(default = "default_lore_base_url")]
    pub lore_base_url: String,
    #[serde(default)]
    pub outputs_path: Option<String>,
    /// Optional directory for generated static HTML (mirrors the markdown tree).
    /// Unset or empty: skip HTML export.
    #[serde(default)]
    pub html_outputs_path: Option<String>,
    /// Public prefix of the HTML tree for `og:url` / canonical (e.g.
    /// `https://example.com/weekly/`). Unset: emit title/description tags only.
    #[serde(default)]
    pub html_site_url: Option<String>,
    /// Absolute `http(s)` URL for `og:image`. Ignored if unset or not http(s).
    #[serde(default)]
    pub html_og_image: Option<String>,
    /// Per-list identity and agent briefing (titles, focus).
    #[serde(default)]
    pub list: ListConfig,
}

impl Config {
    /// Public HTML site prefix for unfurl canonical/`og:url`.
    ///
    /// `html_site_url` wins when set. Otherwise `base_url` is used only if it is
    /// an `http://` or `https://` URL. Blank, `"/"`, and other relative values
    /// are treated as unset.
    pub fn html_public_url(&self) -> Option<&str> {
        absolute_http_url(self.html_site_url.as_deref())
            .or_else(|| absolute_http_url(Some(self.base_url.as_str())))
    }

    /// Absolute `og:image` URL, if configured as `http(s)`.
    pub fn html_og_image_url(&self) -> Option<&str> {
        absolute_http_url(self.html_og_image.as_deref())
    }
}

fn absolute_http_url(value: Option<&str>) -> Option<&str> {
    let s = value.map(str::trim).filter(|s| !s.is_empty())?;
    if s == "/" {
        return None;
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        Some(s)
    } else {
        None
    }
}

/// Mailing-list identity used in the catalog, HTML chrome, and agent prompts.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ListConfig {
    /// Root catalog H1, e.g. `"Example Mailing List Weekly Summaries"`.
    #[serde(default = "default_list_title")]
    pub title: String,
    /// HTML header wordmark. Defaults to `title` when omitted or empty.
    #[serde(default)]
    pub short_title: Option<String>,
    /// Phrase used in agent roles: "covering {name}".
    #[serde(default = "default_list_name")]
    pub name: String,
    /// Extra briefing inserted into thread and week agent prompts.
    #[serde(default)]
    pub focus: String,
}

impl Default for ListConfig {
    fn default() -> Self {
        Self {
            title: default_list_title(),
            short_title: None,
            name: default_list_name(),
            focus: String::new(),
        }
    }
}

impl ListConfig {
    /// Header / crumb title: `short_title` if set, otherwise `title`.
    pub fn short_title(&self) -> &str {
        self.short_title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(self.title.as_str())
    }

    pub fn thread_system_prompt(&self) -> String {
        format!(
            "You are a technical journalist covering {}. Adopt a journalistic tone in your responses.",
            self.name.trim()
        )
    }

    pub fn week_system_prompt(&self) -> String {
        let mut s = format!(
            "You are a technical editor covering {}.\n\
             Write a front-page overview of this week's activity: critical bugs, major trends, and ongoing debates.\n",
            self.name.trim()
        );
        let focus = self.focus.trim();
        if !focus.is_empty() {
            s.push_str(focus);
            if !focus.ends_with('\n') {
                s.push('\n');
            }
        }
        s.push_str(
            "Read thread/*.md files via ReadOutputFile as needed. Link discussions with relative paths like thread/<stem>.md.\n\
             Call SubmitWeekOverview exactly once with a non-empty headline (one line) and markdown_body.\n",
        );
        s
    }
}

fn default_list_title() -> String {
    "Mailing List Weekly Summaries".to_string()
}

fn default_list_name() -> String {
    "this mailing list".to_string()
}

fn default_base_url() -> String {
    "/".to_string()
}

fn default_lore_base_url() -> String {
    DEFAULT_LORE_BASE.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_section_defaults_when_omitted() {
        let raw = r#"
git_repo_path = "/repo"
db_path = "db.sqlite"
[openai]
api_base = "https://example"
model_name = "m"
api_key = "k"
"#;
        let c: Config = toml::from_str(raw).unwrap();
        assert_eq!(c.list.title, "Mailing List Weekly Summaries");
        assert_eq!(c.list.name, "this mailing list");
        assert!(c.list.focus.is_empty());
        assert_eq!(c.list.short_title(), "Mailing List Weekly Summaries");
    }

    #[test]
    fn list_section_overrides() {
        let raw = r#"
git_repo_path = "/repo"
db_path = "db.sqlite"
[openai]
api_base = "https://example"
model_name = "m"
api_key = "k"
[list]
title = "Example Mailing List Weekly Summaries"
short_title = "Example Weekly Summaries"
name = "the example mailing list"
focus = "Focus heavily on core development and important bug fixes."
"#;
        let c: Config = toml::from_str(raw).unwrap();
        assert_eq!(c.list.short_title(), "Example Weekly Summaries");
        assert!(
            c.list
                .thread_system_prompt()
                .contains("the example mailing list")
        );
        assert!(c.list.week_system_prompt().contains("core development"));
    }

    #[test]
    fn html_public_url_ignores_slash_base_url() {
        let raw = r#"
git_repo_path = "/repo"
db_path = "db.sqlite"
base_url = "/"
[openai]
api_base = "https://example"
model_name = "m"
api_key = "k"
"#;
        let c: Config = toml::from_str(raw).unwrap();
        assert_eq!(c.html_public_url(), None);
        assert_eq!(c.html_og_image_url(), None);
    }

    #[test]
    fn html_public_url_falls_back_to_absolute_base_url() {
        let raw = r#"
git_repo_path = "/repo"
db_path = "db.sqlite"
base_url = "https://ex/weekly/"
[openai]
api_base = "https://example"
model_name = "m"
api_key = "k"
"#;
        let c: Config = toml::from_str(raw).unwrap();
        assert_eq!(c.html_public_url(), Some("https://ex/weekly/"));
    }

    #[test]
    fn html_site_url_wins_over_base_url() {
        let raw = r#"
git_repo_path = "/repo"
db_path = "db.sqlite"
base_url = "https://ex/weekly/"
html_site_url = "https://public.example/weekly/"
html_og_image = "https://public.example/og.png"
[openai]
api_base = "https://example"
model_name = "m"
api_key = "k"
"#;
        let c: Config = toml::from_str(raw).unwrap();
        assert_eq!(c.html_public_url(), Some("https://public.example/weekly/"));
        assert_eq!(c.html_og_image_url(), Some("https://public.example/og.png"));
    }

    #[test]
    fn html_og_image_rejects_relative() {
        let raw = r#"
git_repo_path = "/repo"
db_path = "db.sqlite"
html_og_image = "og.png"
[openai]
api_base = "https://example"
model_name = "m"
api_key = "k"
"#;
        let c: Config = toml::from_str(raw).unwrap();
        assert_eq!(c.html_og_image_url(), None);
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OpenAIConfig {
    pub api_base: String,
    pub model_name: String,
    pub api_key: String,
}
