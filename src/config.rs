use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub openai: OpenAIConfig,
    pub git_repo_path: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub previous_outputs_path: Option<String>,
}

fn default_base_url() -> String {
    "/".to_string()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OpenAIConfig {
    pub api_base: String,
    pub model_name: String,
    pub api_key: String,
}
