use crate::config::{Config, ProviderKind};
use crate::llm::anthropic;
use crate::llm::types::{
    AssistantTurn, ChatRequest, FunctionCall, Message, StreamChunk, StreamOptions, ToolCall,
    ToolSpec, Usage,
};
use anyhow::{Context, Result};
use futures::StreamExt;
use std::collections::BTreeMap;

pub struct LlmClient {
    http: reqwest::Client,
    config: Config,
}

#[derive(Debug, Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, serde::Deserialize)]
struct ModelList {
    data: Vec<ModelEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct ModelEntry {
    id: String,
}

impl LlmClient {
    pub fn new(config: Config) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
        }
    }

    /// Lists the model ids the provider exposes, sorted and de-duplicated.
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/models", self.config.base_url);
        let request = match self.config.provider_kind() {
            ProviderKind::Anthropic => self
                .http
                .get(&url)
                .header("x-api-key", self.config.require_api_key()?)
                .header("anthropic-version", anthropic::API_VERSION),
            ProviderKind::OpenAi => self
                .http
                .get(&url)
                .bearer_auth(self.config.require_api_key()?),
        };

        let response = request
            .send()
            .await
            .with_context(|| format!("requesting {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("provider returned {status}: {}", body.trim());
        }

        let list: ModelList = response.json().await.context("parsing model list")?;
        let mut models: Vec<String> = list.data.into_iter().map(|model| model.id).collect();
        models.sort();
        models.dedup();
        Ok(models)
    }

    /// Stream a chat completion. `on_text` is invoked synchronously for every
    /// content delta. Returns the assembled assistant turn (text + tool calls).
    pub async fn stream_chat<F>(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        on_text: F,
    ) -> Result<AssistantTurn>
    where
        F: FnMut(String),
    {
        match self.config.provider_kind() {
            ProviderKind::Anthropic => self.stream_anthropic(messages, tools, on_text).await,
            ProviderKind::OpenAi => self.stream_openai(messages, tools, on_text).await,
        }
    }

    async fn stream_openai<F>(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        mut on_text: F,
    ) -> Result<AssistantTurn>
    where
        F: FnMut(String),
    {
        let url = format!("{}/chat/completions", self.config.base_url);
        let messages = messages
            .iter()
            .cloned()
            .map(|mut message| {
                message.thinking = None;
                message
            })
            .collect();
        let request = ChatRequest {
            model: self.config.model.clone(),
            messages,
            stream: true,
            max_tokens: self.config.max_tokens,
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.to_vec())
            },
            reasoning_effort: self
                .config
                .effective_reasoning()
                .effort()
                .map(str::to_string),
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
        };

        let response = self
            .http
            .post(&url)
            .bearer_auth(self.config.require_api_key()?)
            .json(&request)
            .send()
            .await
            .with_context(|| format!("requesting {url}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("provider returned {status}: {}", body.trim());
        }

        let mut turn = AssistantTurn::default();
        let mut partials: Vec<PartialToolCall> = Vec::new();

        read_sse(response, |data| {
            let Ok(parsed) = serde_json::from_str::<StreamChunk>(data) else {
                return Ok(());
            };
            if let Some(usage) = &parsed.usage {
                turn.usage = Usage {
                    input: usage.prompt_tokens,
                    output: usage.completion_tokens,
                };
            }
            let Some(choice) = parsed.choices.into_iter().next() else {
                return Ok(());
            };

            if let Some(text) = choice.delta.content {
                if !text.is_empty() {
                    on_text(text.clone());
                    turn.content.push_str(&text);
                }
            }

            if let Some(calls) = choice.delta.tool_calls {
                for call in calls {
                    while partials.len() <= call.index {
                        partials.push(PartialToolCall::default());
                    }
                    let partial = &mut partials[call.index];
                    if let Some(id) = call.id {
                        if !id.is_empty() {
                            partial.id = id;
                        }
                    }
                    if let Some(function) = call.function {
                        if let Some(name) = function.name {
                            if !name.is_empty() {
                                partial.name = name;
                            }
                        }
                        if let Some(args) = function.arguments {
                            partial.arguments.push_str(&args);
                        }
                    }
                }
            }
            Ok(())
        })
        .await?;

        turn.tool_calls = partials
            .into_iter()
            .filter(|p| !p.name.is_empty())
            .enumerate()
            .map(|(i, p)| ToolCall {
                id: if p.id.is_empty() {
                    format!("call_{i}")
                } else {
                    p.id
                },
                kind: "function".to_string(),
                function: FunctionCall {
                    name: p.name,
                    arguments: p.arguments,
                },
            })
            .collect();

        Ok(turn)
    }

    async fn stream_anthropic<F>(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        mut on_text: F,
    ) -> Result<AssistantTurn>
    where
        F: FnMut(String),
    {
        let url = format!("{}/messages", self.config.base_url);
        let body = anthropic::request_body(&self.config, messages, tools);

        let response = self
            .http
            .post(&url)
            .header("x-api-key", self.config.require_api_key()?)
            .header("anthropic-version", anthropic::API_VERSION)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("requesting {url}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("provider returned {status}: {}", body.trim());
        }

        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();

        read_sse(response, |data| {
            anthropic::apply_event(data, &mut turn, &mut partials, &mut on_text)
        })
        .await?;

        turn.tool_calls = anthropic::into_tool_calls(partials);
        Ok(turn)
    }
}

async fn read_sse<F>(response: reqwest::Response, mut on_data: F) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading response stream")?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find('\n') {
            let line: String = buffer.drain(..=pos).collect();
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            on_data(data)?;
        }
    }

    Ok(())
}
