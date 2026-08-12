//! OpenAI client construction from app config.

use crate::config::Config;
use da_harness::{LLMConfig, OpenAIClient};

pub fn client_from_config(config: &Config) -> OpenAIClient {
    OpenAIClient::with_config(LLMConfig {
        api_base: config.openai.api_base.clone(),
        model_name: config.openai.model_name.clone(),
        api_key: config.openai.api_key.clone(),
        max_context_tokens: None,
        extra_headers: Default::default(),
    })
}
