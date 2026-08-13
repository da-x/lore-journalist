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
    /// `https://lore.kernel.org/linux-nfs/`.
    #[serde(default = "default_lore_base_url")]
    pub lore_base_url: String,
    #[serde(default)]
    pub outputs_path: Option<String>,
    /// Optional directory for generated static HTML (mirrors the markdown tree).
    /// Unset or empty: skip HTML export.
    #[serde(default)]
    pub html_outputs_path: Option<String>,
    /// Per-list identity and agent briefing (titles, focus). Not NFS-specific.
    #[serde(default)]
    pub list: ListConfig,
}

/// Mailing-list identity used in the catalog, HTML chrome, and agent prompts.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ListConfig {
    /// Root catalog H1, e.g. `"NFS Mailing List Weekly Summaries"`.
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
title = "NFS Mailing List Weekly Summaries"
short_title = "NFS Weekly Summaries"
name = "the Linux NFS mailing list"
focus = "Focus heavily on NFS client development and important bug fixes."
"#;
        let c: Config = toml::from_str(raw).unwrap();
        assert_eq!(c.list.short_title(), "NFS Weekly Summaries");
        assert!(
            c.list
                .thread_system_prompt()
                .contains("the Linux NFS mailing list")
        );
        assert!(
            c.list
                .week_system_prompt()
                .contains("NFS client development")
        );
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OpenAIConfig {
    pub api_base: String,
    pub model_name: String,
    pub api_key: String,
}
