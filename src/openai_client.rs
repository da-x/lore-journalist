#![allow(unused)]

use crate::config::OpenAIConfig;
use crate::models::Thread;
use anyhow::{Result, anyhow};
use async_openai::{
    Client,
    config::OpenAIConfig as AsyncOpenAIConfig,
    types::{
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
        CreateChatCompletionRequestArgs,
    },
};
use tracing::info;

pub struct OpenAIClient {
    config: OpenAIConfig,
    client: Client<AsyncOpenAIConfig>,
}

impl OpenAIClient {
    pub fn new(config: OpenAIConfig) -> Self {
        let mut async_config = AsyncOpenAIConfig::default().with_api_key(config.api_key.clone());

        if !config.api_base.is_empty() {
            // Handle case where user might provide full URL with /chat/completions
            let base = config.api_base.replace("/chat/completions", "");
            async_config = async_config.with_api_base(base);
        }

        let client = Client::with_config(async_config);
        Self { config, client }
    }

    pub async fn summarize_thread(&self, thread: &Thread) -> Result<String> {
        let mut thread_content = String::new();
        for msg in &thread.messages {
            thread_content.push_str(&format!(
                "From: {}\nDate: {}\nMessage-ID: {}\nSubject: {}\n\n{}\n",
                msg.from, msg.date, msg.message_id, msg.subject, msg.body
            ));
            thread_content.push_str("\n---Next Message---\n");
        }

        let system_prompt = "You are a technical journalist specializing in Linux Kernel development. Adopt a journalistic tone in your responses.";

        let user_prompt = format!(
            "Provide a detailed summary of the following mailing list thread.
            Highlight the key technical arguments, the evolution of the discussion, and the final conclusions.
            Focus heavily on NFS client development and important bug fixes.

            IMPORTANT:
            - Quote specific conclusions and significant intermediate remarks from the participants to provide context and flavor. Use double quotes and Markdown blockquote syntax (e.g., > \"Quote content\") for these quotes.
            - Identify if the discussion is about a new feature, a protocol change, or a bug fix.
            - When referring to a specific message, use the following markup: [text](id://message-id).
            Example: [As mentioned by Chuck Lever](id://example-msg-id).
            - The message-id should be taken exactly from the Message-ID header provided in the text.

            Summarize this thread:\n\n{}",
            thread_content
        );

        info!(
            "Sending summarization request to LLM for thread: {}",
            thread.subject
        );

        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.config.model_name)
            .messages([
                ChatCompletionRequestSystemMessageArgs::default()
                    .content(system_prompt)
                    .build()?
                    .into(),
                ChatCompletionRequestUserMessageArgs::default()
                    .content(user_prompt)
                    .build()?
                    .into(),
            ])
            .temperature(0.3)
            .build()?;

        let response = self.client.chat().create(request).await?;

        let summary = response.choices[0]
            .message
            .content
            .clone()
            .ok_or_else(|| anyhow!("Failed to parse summary from response"))?;

        Ok(summary)
    }

    pub async fn summarize_week(&self, summaries: &[(String, String, String)]) -> Result<String> {
        let mut combined = String::new();
        for (id, title, summary) in summaries {
            combined.push_str(&format!(
                "### Discussion ID: {}\nTitle: {}\n\n{}\n\n",
                id, title, summary
            ));
        }

        let system_prompt = "You are a technical editor specializing in Linux Kernel development.";

        let user_prompt = format!(
            "Given the following summaries of this week's Linux NFS mailing list activity,
            provide a high-level overview for the front page. Highlight major trends, critical bug fixes (especially in the NFS client),
            and important ongoing discussions. Use markdown.

            IMPORTANT: When referring to a discussion, use the following markup: [text](id://discussion-id).
            Example: [Discussion on nfsd simplification](id://example-id).
            The discussion-id must be taken exactly from the 'Discussion ID' field provided.

            Title format of the document I want you to produce:

            (bigtitle)
            (date)

            Example:

            ```
            <center><h1>Critical Client Regressions and Architectural Shifts</h1></center>
            <center><h3>In week ending 2026-03-15</h3></center>
            ```

            These are the only centered titles that should be produced.

            Summaries:\n\n{}",
            combined
        );

        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.config.model_name)
            .messages([
                ChatCompletionRequestSystemMessageArgs::default()
                    .content(system_prompt)
                    .build()?
                    .into(),
                ChatCompletionRequestUserMessageArgs::default()
                    .content(user_prompt)
                    .build()?
                    .into(),
            ])
            .temperature(0.3)
            .build()?;

        let response = self.client.chat().create(request).await?;

        let summary = response.choices[0]
            .message
            .content
            .clone()
            .ok_or_else(|| anyhow!("Failed to parse weekly summary"))?;

        Ok(summary)
    }
}
