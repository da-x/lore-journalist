//! OpenAI client construction from app config.

use crate::config::{Config, OpenAIConfig};
use anyhow::Context;
use da_harness::{LLMConfig, OpenAIClient};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Config sentinel: use the Grok CLI chat proxy instead of `[openai].api_base`.
const GROK_BUILD_SENTINEL: &str = "grok-build";
/// Predetermined Grok CLI proxy endpoint (`LLMConfig::GROK_BUILD_API_BASE` in kbuild).
const GROK_BUILD_API_BASE: &str = "https://cli-chat-proxy.grok.com/v1";
/// Request model for the grok-build endpoint (`default_grok_build` in kbuild).
const GROK_BUILD_REQUEST_MODEL: &str = "grok-4.6";

/// Injected headers for `cli-chat-proxy.grok.com` (Authorization Bearer comes from
/// [`LLMConfig::api_key`] via the OpenAI client — not duplicated here):
/// ```text
/// X-XAI-Token-Auth: xai-grok-cli
/// x-grok-model-override: <model_name>
/// x-grok-client-version: <GROK_CLIENT_VERSION>
/// x-grok-client-identifier: grok-shell
/// ```
const GROK_TOKEN_AUTH_HEADER: &str = "X-XAI-Token-Auth";
const GROK_TOKEN_AUTH_VALUE: &str = "xai-grok-cli";
const GROK_MODEL_OVERRIDE_HEADER: &str = "x-grok-model-override";
const GROK_CLIENT_VERSION_HEADER: &str = "x-grok-client-version";
const GROK_CLIENT_VERSION_VALUE: &str = "0.2.118";
const GROK_CLIENT_IDENTIFIER_HEADER: &str = "x-grok-client-identifier";
const GROK_CLIENT_IDENTIFIER_VALUE: &str = "grok-shell";

pub fn client_from_config(config: &Config) -> anyhow::Result<OpenAIClient> {
    Ok(OpenAIClient::with_config(llm_config_from_openai(
        &config.openai,
    )?))
}

fn llm_config_from_openai(openai: &OpenAIConfig) -> anyhow::Result<LLMConfig> {
    if openai.model_name == GROK_BUILD_SENTINEL {
        grok_build_llm_config_from_auth_file(&default_grok_auth_path()?)
    } else {
        Ok(LLMConfig {
            api_base: openai.api_base.clone(),
            model_name: openai.model_name.clone(),
            api_key: openai.api_key.clone(),
            max_context_tokens: None,
            extra_headers: Default::default(),
        })
    }
}

fn grok_build_llm_config_from_auth_file(path: &Path) -> anyhow::Result<LLMConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read Grok auth file {}", path.display()))?;
    let api_key = grok_build_key_from_auth_json(&raw)?;
    Ok(grok_build_llm_config(api_key))
}

fn grok_build_llm_config(api_key: String) -> LLMConfig {
    let mut extra_headers = HashMap::new();
    ensure_grok_cli_headers(GROK_BUILD_REQUEST_MODEL, &mut extra_headers);
    LLMConfig {
        api_base: GROK_BUILD_API_BASE.to_string(),
        model_name: GROK_BUILD_REQUEST_MODEL.to_string(),
        api_key,
        max_context_tokens: None,
        extra_headers,
    }
}

/// Headers required by `cli-chat-proxy.grok.com` for session tokens.
fn ensure_grok_cli_headers(model_name: &str, headers: &mut HashMap<String, String>) {
    headers
        .entry(GROK_TOKEN_AUTH_HEADER.to_string())
        .or_insert_with(|| GROK_TOKEN_AUTH_VALUE.to_string());
    if !model_name.is_empty() {
        headers
            .entry(GROK_MODEL_OVERRIDE_HEADER.to_string())
            .or_insert_with(|| model_name.to_string());
    }
    headers
        .entry(GROK_CLIENT_VERSION_HEADER.to_string())
        .or_insert_with(|| GROK_CLIENT_VERSION_VALUE.to_string());
    headers
        .entry(GROK_CLIENT_IDENTIFIER_HEADER.to_string())
        .or_insert_with(|| GROK_CLIENT_IDENTIFIER_VALUE.to_string());
}

fn default_grok_auth_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set; cannot locate ~/.grok/auth.json"))?;
    Ok(home.join(".grok").join("auth.json"))
}

/// Extract the Grok API key from a Grok CLI `auth.json` object.
///
/// Looks for a top-level key starting with `http` and a nested string `"key"`.
/// Does not interpret the top-level key name.
fn grok_build_key_from_auth_json(raw: &str) -> anyhow::Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| anyhow::anyhow!("parse auth.json: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("auth.json must be a JSON object"))?;

    let mut found: Vec<String> = Vec::new();
    for (k, v) in obj {
        if !k.starts_with("http") {
            continue;
        }
        let Some(key) = v
            .get("key")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        found.push(key.to_string());
    }

    match found.len() {
        0 => {
            anyhow::bail!("no nested \"key\" under a top-level key starting with http in auth.json")
        }
        1 => Ok(found.pop().expect("len == 1")),
        n => anyhow::bail!(
            "ambiguous auth.json: {n} top-level keys starting with http have a nested \"key\""
        ),
    }
}
