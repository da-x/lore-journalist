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
}

fn default_base_url() -> String {
    "/".to_string()
}

fn default_lore_base_url() -> String {
    DEFAULT_LORE_BASE.to_string()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OpenAIConfig {
    pub api_base: String,
    pub model_name: String,
    pub api_key: String,
}
