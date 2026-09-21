use crate::config::{
    glm_forces_thinking, supports_adaptive_thinking, Config, ProviderKind, Reasoning,
};
use crate::llm::anthropic;
use crate::llm::types::{
    push_thinking, AssistantTurn, ChatRequest, FunctionCall, Message, StreamChunk, StreamOptions,
    ToolCall, ToolSpec, Usage,
};
use anyhow::{Context, Result};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MODEL_CACHE_TTL_SECS: u64 = 24 * 60 * 60;
const MODEL_CACHE_FILE: &str = "model-cache.json";
/// How many times a transient stream failure is retried before giving up.
const MAX_STREAM_ATTEMPTS: u32 = 3;
/// Cap how long a connect or a single streamed read may stall before the
/// request fails. Without this a dead proxy or dropped connection leaves the
/// agent waiting forever with no output.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(120);
static MODEL_CACHE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

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

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct ModelCache {
    entries: BTreeMap<String, CachedModels>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedModels {
    updated_at: u64,
    models: Vec<String>,
}

/// Callbacks the client invokes while a turn streams, so the caller can show
/// the work in progress: reasoning text as the model emits it, and transient
/// failures that are about to be retried.
///
/// A retried attempt re-sends its reasoning from the start, so `thinking` may
/// repeat fragments it already delivered; consumers reset their view when
/// `retry` fires. Text is never repeated, because an attempt that emitted any
/// is not retried.
pub struct StreamHooks<'a> {
    pub text: &'a mut (dyn FnMut(String) + Send),
    pub thinking: &'a mut (dyn FnMut(String) + Send),
    pub retry: &'a mut (dyn FnMut(Retry) + Send),
}

/// A transient provider failure the client is about to retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    pub attempt: u32,
    pub max: u32,
    pub delay: Duration,
}

impl LlmClient {
    pub fn new(config: Config) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { http, config }
    }

    /// Lists the model ids the provider exposes, sorted and de-duplicated.
    pub async fn list_models(&self) -> Result<Vec<String>> {
        if !self.config.model_catalog.is_empty() {
            return Ok(self.config.model_catalog());
        }
        let cache_key = model_cache_key(&self.config);
        let cached = cached_models(&cache_key);
        if let Some(entry) = cached.as_ref().filter(|entry| entry.is_fresh()) {
            return Ok(self.config.merge_model_catalog(entry.models.clone()));
        }

        match self.fetch_models().await {
            Ok(models) => {
                if !models.is_empty() {
                    store_cached_models(&cache_key, &models);
                }
                Ok(self.config.merge_model_catalog(models))
            }
            Err(err) => match cached {
                Some(entry) => Ok(self.config.merge_model_catalog(entry.models)),
                None => Err(err),
            },
        }
    }

    async fn fetch_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/models", self.config.base_url);
        let request = match self.config.provider_kind() {
            ProviderKind::Anthropic => self
                .http
                .get(&url)
                .header("x-api-key", self.config.require_api_key()?)
                .header("anthropic-version", anthropic::API_VERSION),
            ProviderKind::OpenAi => self.authenticate_openai(self.http.get(&url))?,
        };

        let response = request
            .send()
            .await
            .with_context(|| format!("requesting {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if self.config.is_portkey() && status == reqwest::StatusCode::FORBIDDEN {
                return Ok(self.config.model_catalog());
            }
            // Z.AI does not document a model listing endpoint, so a missing or
            // restricted one falls back to the bundled catalog.
            if self.config.is_zai()
                && matches!(
                    status,
                    reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::NOT_FOUND
                )
            {
                return Ok(self.config.model_catalog());
            }
            anyhow::bail!("provider returned {status}: {}", body.trim());
        }

        let list: ModelList = match response.json().await {
            Ok(list) => list,
            // The listing endpoint is undocumented at Z.AI, so an unexpected
            // shape falls back to the bundled catalog like a missing one.
            Err(_) if self.config.is_zai() => return Ok(self.config.model_catalog()),
            Err(err) => return Err(err).context("parsing model list"),
        };
        let mut models: Vec<String> = list.data.into_iter().map(|model| model.id).collect();
        models.sort();
        models.dedup();
        Ok(models)
    }

    /// Stream a chat completion, reporting text, reasoning, and retries through
    /// `hooks`. Returns the assembled assistant turn (text, thinking, and tool
    /// calls).
    ///
    /// Transient failures (network errors, truncated streams, rate limits and
    /// 5xx responses) are retried with backoff, but only while the attempt has
    /// not emitted any text yet, so a partial response is never duplicated.
    pub async fn stream_chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        hooks: &mut StreamHooks<'_>,
    ) -> Result<AssistantTurn> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let mut emitted = false;
            let result = {
                let mut attempt_text = |delta: String| {
                    emitted = true;
                    (hooks.text)(delta);
                };
                let mut attempt_hooks = StreamHooks {
                    text: &mut attempt_text,
                    thinking: &mut *hooks.thinking,
                    retry: &mut *hooks.retry,
                };
                match self.config.provider_kind() {
                    ProviderKind::Anthropic => {
                        self.stream_anthropic(messages, tools, &mut attempt_hooks)
                            .await
                    }
                    ProviderKind::OpenAi => {
                        self.stream_openai(messages, tools, &mut attempt_hooks)
                            .await
                    }
                }
            };
            match result {
                Ok(mut turn) => {
                    // A turn with neither text nor tool calls carries nothing
                    // the agent can act on. It is usually a transient provider
                    // hiccup, so retry it like a dropped stream instead of
                    // failing the whole turn on the first empty response.
                    if assistant_turn_is_empty(&turn) {
                        if attempt < MAX_STREAM_ATTEMPTS {
                            let delay = Duration::from_millis(500 * 2u64.pow(attempt - 1));
                            (hooks.retry)(Retry {
                                attempt,
                                max: MAX_STREAM_ATTEMPTS,
                                delay,
                            });
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        anyhow::bail!("the model returned an empty response");
                    }
                    turn.usage.cost = self.config.usage_cost(&turn.usage);
                    return Ok(turn);
                }
                Err(err) => {
                    if emitted || attempt >= MAX_STREAM_ATTEMPTS || !is_retryable(&err) {
                        return Err(err);
                    }
                    let delay = Duration::from_millis(500 * 2u64.pow(attempt - 1));
                    (hooks.retry)(Retry {
                        attempt,
                        max: MAX_STREAM_ATTEMPTS,
                        delay,
                    });
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    async fn stream_openai(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        hooks: &mut StreamHooks<'_>,
    ) -> Result<AssistantTurn> {
        let url = format!("{}/chat/completions", self.config.base_url);
        let request = openai_request(&self.config, messages, tools);

        let response = self
            .authenticate_openai(self.http.post(&url))?
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

        let completed = read_sse(response, |data| {
            let Ok(parsed) = serde_json::from_str::<StreamChunk>(data) else {
                return Ok(());
            };
            if let Some(error) = parsed.error {
                anyhow::bail!("provider error: {}", error.describe());
            }
            if let Some(usage) = &parsed.usage {
                let cached = usage
                    .prompt_tokens_details
                    .as_ref()
                    .map(|details| details.cached_tokens)
                    .unwrap_or(0);
                turn.usage = Usage {
                    input: usage.prompt_tokens.saturating_sub(cached),
                    output: usage.completion_tokens,
                    cache_read: cached,
                    cache_write: 0,
                    cost: 0.0,
                };
            }
            let Some(choice) = parsed.choices.into_iter().next() else {
                return Ok(());
            };

            if let Some(text) = choice.delta.reasoning() {
                (hooks.thinking)(text.to_string());
                push_thinking(&mut turn.thinking, text);
            }

            if let Some(text) = choice.delta.content {
                if !text.is_empty() {
                    (hooks.text)(text.clone());
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

        if !completed && turn.content.is_empty() && turn.tool_calls.is_empty() {
            anyhow::bail!("provider stream ended before completing the response");
        }

        Ok(turn)
    }

    fn authenticate_openai(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder> {
        let key = self.config.require_api_key()?;
        if self.config.is_portkey() {
            let request = request.header("x-portkey-api-key", key);
            if self.config.portkey_config.trim().is_empty() {
                Ok(request)
            } else {
                Ok(request.header("x-portkey-config", self.config.portkey_config.trim()))
            }
        } else {
            Ok(request.bearer_auth(key))
        }
    }

    async fn stream_anthropic(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        hooks: &mut StreamHooks<'_>,
    ) -> Result<AssistantTurn> {
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

        let completed = read_sse(response, |data| {
            anthropic::apply_event(data, &mut turn, &mut partials, hooks.text, hooks.thinking)
        })
        .await?;

        turn.tool_calls = anthropic::into_tool_calls(partials);
        if !completed && assistant_turn_is_empty(&turn) {
            anyhow::bail!("provider stream ended before completing the response");
        }
        Ok(turn)
    }
}

impl CachedModels {
    fn is_fresh(&self) -> bool {
        now_secs().saturating_sub(self.updated_at) < MODEL_CACHE_TTL_SECS
    }
}

fn model_cache_key(config: &Config) -> String {
    let identity = format!(
        "{}\n{}\n{}",
        config.provider, config.base_url, config.portkey_config
    );
    format!("{:x}", Sha256::digest(identity.as_bytes()))
}

fn model_cache_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("oxide").join(MODEL_CACHE_FILE))
}

fn cached_models(key: &str) -> Option<CachedModels> {
    let path = model_cache_path()?;
    let _guard = MODEL_CACHE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .ok()?;
    load_model_cache(&path).entries.get(key).cloned()
}

fn store_cached_models(key: &str, models: &[String]) {
    let Some(path) = model_cache_path() else {
        return;
    };
    let Ok(_guard) = MODEL_CACHE_LOCK.get_or_init(|| Mutex::new(())).lock() else {
        return;
    };
    let mut cache = load_model_cache(&path);
    cache.entries.insert(
        key.to_string(),
        CachedModels {
            updated_at: now_secs(),
            models: models.to_vec(),
        },
    );
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(text) = serde_json::to_string_pretty(&cache) else {
        return;
    };
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    if std::fs::write(&temporary, text).is_err() {
        return;
    }
    if std::fs::rename(&temporary, &path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

fn load_model_cache(path: &Path) -> ModelCache {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn openai_request(config: &Config, messages: &[Message], tools: &[ToolSpec]) -> ChatRequest {
    let messages = messages
        .iter()
        .cloned()
        .map(|mut message| {
            message.thinking = None;
            message
        })
        .collect();
    let (reasoning_effort, thinking, output_config) = openai_reasoning(config);
    ChatRequest {
        model: config.model.clone(),
        messages,
        stream: true,
        max_tokens: config.max_tokens,
        tools: if tools.is_empty() {
            None
        } else {
            Some(tools.to_vec())
        },
        reasoning_effort,
        thinking,
        output_config,
        stream_options: Some(StreamOptions {
            include_usage: true,
        }),
    }
}

fn openai_reasoning(
    config: &Config,
) -> (
    Option<String>,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
) {
    if config.is_zai() {
        return glm_reasoning(config);
    }
    let adaptive = config.is_portkey() && supports_adaptive_thinking(&config.model);
    match config.reasoning {
        Reasoning::Off => (None, None, None),
        Reasoning::Auto if adaptive => {
            (None, Some(serde_json::json!({ "type": "adaptive" })), None)
        }
        Reasoning::Auto => (None, None, None),
        level if adaptive => (
            None,
            Some(serde_json::json!({ "type": "adaptive" })),
            Some(serde_json::json!({ "effort": level.effort() })),
        ),
        level => (level.effort().map(str::to_string), None, None),
    }
}

/// GLM enables or disables thinking with `thinking.type`, and its
/// `reasoning_effort` scale is `low`/`high`/`max` on the current flagship, so
/// oxide's medium and high map onto high and max. Models that always think get
/// the lowest effort instead of an unsupported `disabled`.
fn glm_reasoning(
    config: &Config,
) -> (
    Option<String>,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
) {
    let effort = |level: &str| Some(level.to_string());
    let enabled = || Some(serde_json::json!({ "type": "enabled" }));
    match config.reasoning {
        Reasoning::Auto => (None, None, None),
        Reasoning::Off if glm_forces_thinking(&config.model) => (effort("low"), enabled(), None),
        Reasoning::Off => (None, Some(serde_json::json!({ "type": "disabled" })), None),
        Reasoning::Low => (effort("low"), enabled(), None),
        Reasoning::Medium => (effort("high"), enabled(), None),
        Reasoning::High => (effort("max"), enabled(), None),
    }
}

async fn read_sse<F>(response: reqwest::Response, mut on_data: F) -> Result<bool>
where
    F: FnMut(&str) -> Result<()>,
{
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    let mut completed = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading response stream")?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        drain_lines(&mut buffer, |line| {
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else {
                return Ok(());
            };
            let data = data.trim();
            if data.is_empty() {
                return Ok(());
            }
            if data == "[DONE]" {
                completed = true;
                return Ok(());
            }
            on_data(data)
        })?;
    }

    Ok(completed)
}

/// Hands every complete line in `buffer` to `on_line`, keeping the trailing
/// partial line for the next chunk.
///
/// The consumed prefix is removed once per chunk rather than once per line:
/// draining after every line shifts the whole remainder down, which makes a
/// chunk carrying many events quadratic in the number of events it holds.
fn drain_lines<F>(buffer: &mut String, mut on_line: F) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    let mut consumed = 0;
    while let Some(offset) = buffer[consumed..].find('\n') {
        let end = consumed + offset;
        on_line(&buffer[consumed..end])?;
        consumed = end + 1;
    }
    buffer.drain(..consumed);
    Ok(())
}

/// Whether a completed turn carries nothing the agent can act on.
fn assistant_turn_is_empty(turn: &AssistantTurn) -> bool {
    turn.content.trim().is_empty() && turn.tool_calls.is_empty()
}

/// Whether a failed model request is worth retrying. Network failures, stream
/// truncation, rate limits, and 5xx responses are retryable; other 4xx provider
/// errors (bad request, auth, not found) are not.
fn is_retryable(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}");
    if let Some(rest) = text.split("provider returned ").nth(1) {
        if let Some(code) = rest
            .split_whitespace()
            .next()
            .and_then(|code| code.parse::<u16>().ok())
        {
            return code == 429 || code >= 500;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_turns_are_detected_for_retry() {
        let mut turn = AssistantTurn::default();
        assert!(assistant_turn_is_empty(&turn));

        turn.content = "   ".into();
        assert!(assistant_turn_is_empty(&turn));

        turn.content = "hello".into();
        assert!(!assistant_turn_is_empty(&turn));

        turn.content.clear();
        turn.tool_calls = vec![ToolCall {
            id: "call_0".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read".into(),
                arguments: "{}".into(),
            },
        }];
        assert!(!assistant_turn_is_empty(&turn));
    }

    #[test]
    fn retries_transient_errors_but_not_client_errors() {
        assert!(is_retryable(&anyhow::anyhow!(
            "provider stream ended before completing the response"
        )));
        assert!(is_retryable(&anyhow::anyhow!(
            "requesting https://example.test: connection reset"
        )));
        assert!(is_retryable(&anyhow::anyhow!(
            "provider returned 429 Too Many Requests: slow down"
        )));
        assert!(is_retryable(&anyhow::anyhow!(
            "provider returned 503 Service Unavailable: overloaded"
        )));
        assert!(!is_retryable(&anyhow::anyhow!(
            "provider returned 400 Bad Request: bad messages"
        )));
        assert!(!is_retryable(&anyhow::anyhow!(
            "provider returned 401 Unauthorized: bad key"
        )));
    }

    #[test]
    fn portkey_uses_native_api_key_header() {
        let config = Config {
            provider: "portkey".into(),
            api_key: "pk-test".into(),
            portkey_config: "pc-test".into(),
            ..Config::default()
        };
        let client = LlmClient::new(config);
        let request = client
            .authenticate_openai(client.http.get("https://example.test/models"))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(request.headers()["x-portkey-api-key"], "pk-test");
        assert_eq!(request.headers()["x-portkey-config"], "pc-test");
        assert!(!request.headers().contains_key("authorization"));
    }

    #[test]
    fn portkey_fallback_catalog_includes_requested_models_and_active_model() {
        let config = Config {
            provider: "portkey".into(),
            model: "account-specific-model".into(),
            ..Config::default()
        };
        let models = config.model_catalog();
        assert!(models.contains(&"claude-sonnet-5".to_string()));
        assert!(models.contains(&"gpt-5.6-sol".to_string()));
        assert!(models.contains(&"account-specific-model".to_string()));
    }

    #[test]
    fn custom_portkey_catalog_replaces_fallback_catalog() {
        let config = Config {
            provider: "portkey".into(),
            model: "custom-b".into(),
            model_catalog: vec!["custom-a".into(), "custom-b".into()],
            ..Config::default()
        };
        assert_eq!(config.model_catalog(), vec!["custom-a", "custom-b"]);
    }

    #[test]
    fn auto_uses_provider_native_reasoning() {
        for provider in ["openai", "deepseek", "custom"] {
            let config = Config {
                provider: provider.into(),
                model: "reasoning-model".into(),
                reasoning: Reasoning::Auto,
                ..Config::default()
            };
            assert_eq!(openai_reasoning(&config), (None, None, None));
            let body =
                serde_json::to_value(openai_request(&config, &[Message::user("hi")], &[])).unwrap();
            assert!(body.get("reasoning_effort").is_none());
            assert!(body.get("thinking").is_none());
            assert!(body.get("output_config").is_none());
        }
    }

    #[test]
    fn portkey_adaptive_claude_uses_native_thinking() {
        let config = Config {
            provider: "portkey".into(),
            model: "claude-sonnet-5".into(),
            reasoning: Reasoning::Auto,
            ..Config::default()
        };
        let (effort, thinking, output) = openai_reasoning(&config);
        assert_eq!(effort, None);
        assert_eq!(thinking.unwrap()["type"], "adaptive");
        assert_eq!(output, None);
        let body =
            serde_json::to_value(openai_request(&config, &[Message::user("hi")], &[])).unwrap();
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("output_config").is_none());
        assert_eq!(body["stream_options"]["include_usage"], true);

        let config = Config {
            reasoning: Reasoning::High,
            ..config
        };
        let (effort, thinking, output) = openai_reasoning(&config);
        assert_eq!(effort, None);
        assert_eq!(thinking.unwrap()["type"], "adaptive");
        assert_eq!(output.unwrap()["effort"], "high");
    }

    #[test]
    fn explicit_reasoning_uses_openai_compatible_effort() {
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-reasoner".into(),
            reasoning: Reasoning::Low,
            ..Config::default()
        };
        assert_eq!(openai_reasoning(&config), (Some("low".into()), None, None));
    }

    #[test]
    fn glm_uses_thinking_type_and_its_own_effort_levels() {
        let config = |model: &str, reasoning: Reasoning| Config {
            provider: "zai".into(),
            model: model.into(),
            reasoning,
            ..Config::default()
        };
        assert_eq!(
            openai_reasoning(&config("glm-5.3", Reasoning::Auto)),
            (None, None, None)
        );
        assert_eq!(
            openai_reasoning(&config("glm-5.3", Reasoning::Medium)),
            (
                Some("high".into()),
                Some(serde_json::json!({ "type": "enabled" })),
                None
            )
        );
        assert_eq!(
            openai_reasoning(&config("glm-5.3", Reasoning::High)),
            (
                Some("max".into()),
                Some(serde_json::json!({ "type": "enabled" })),
                None
            )
        );
        assert_eq!(
            openai_reasoning(&config("glm-5.2", Reasoning::Off)),
            (None, Some(serde_json::json!({ "type": "disabled" })), None)
        );
        assert_eq!(
            openai_reasoning(&config("glm-5.3", Reasoning::Off)),
            (
                Some("low".into()),
                Some(serde_json::json!({ "type": "enabled" })),
                None
            )
        );

        let body = serde_json::to_value(openai_request(
            &config("glm-5.3", Reasoning::Low),
            &[Message::user("hi")],
            &[],
        ))
        .unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "low");
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn openai_keeps_bearer_authentication() {
        let config = Config {
            api_key: "sk-test".into(),
            ..Config::default()
        };
        let client = LlmClient::new(config);
        let request = client
            .authenticate_openai(client.http.get("https://example.test/models"))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(request.headers()["authorization"], "Bearer sk-test");
        assert!(!request.headers().contains_key("x-portkey-api-key"));
    }

    #[test]
    fn model_cache_keys_include_provider_endpoint_and_gateway_config() {
        let base = Config {
            provider: "portkey".into(),
            base_url: "https://gateway.example/v1".into(),
            portkey_config: "team-a".into(),
            ..Config::default()
        };
        let mut changed = base.clone();
        changed.portkey_config = "team-b".into();
        assert_ne!(model_cache_key(&base), model_cache_key(&changed));
        assert!(!model_cache_key(&base).contains("gateway.example"));
    }

    #[test]
    fn model_cache_freshness_expires_after_ttl() {
        let current = now_secs();
        assert!(CachedModels {
            updated_at: current,
            models: vec!["model".into()]
        }
        .is_fresh());
        assert!(!CachedModels {
            updated_at: current.saturating_sub(MODEL_CACHE_TTL_SECS + 1),
            models: vec!["model".into()]
        }
        .is_fresh());
    }

    #[test]
    fn backoff_grows_with_each_attempt() {
        assert_eq!(MAX_STREAM_ATTEMPTS, 3);
    }

    fn drained(chunks: &[&str]) -> (Vec<String>, String) {
        let mut buffer = String::new();
        let mut lines = Vec::new();
        for chunk in chunks {
            buffer.push_str(chunk);
            drain_lines(&mut buffer, |line| {
                lines.push(line.to_string());
                Ok(())
            })
            .unwrap();
        }
        (lines, buffer)
    }

    #[test]
    fn sse_lines_are_emitted_whole_whatever_the_chunk_boundaries() {
        let whole = "data: one\ndata: two\n\ndata: three\n";
        let (lines, tail) = drained(&[whole]);
        assert_eq!(lines, ["data: one", "data: two", "", "data: three"]);
        assert!(tail.is_empty());

        let split = drained(&["data: on", "e\ndata:", " two\n\ndata: th", "ree\n"]);
        assert_eq!(split.0, lines);
        assert!(split.1.is_empty());
    }

    #[test]
    fn an_incomplete_line_stays_buffered_without_shifting_the_rest() {
        let (lines, tail) = drained(&["data: complete\ndata: incomp"]);
        assert_eq!(lines, ["data: complete"]);
        assert_eq!(tail, "data: incomp");

        let (lines, tail) = drained(&["a\nb\nc\n"]);
        assert_eq!(lines, ["a", "b", "c"]);
        assert!(tail.is_empty());
    }

    #[test]
    fn a_failing_handler_stops_the_drain() {
        let mut buffer = String::from("a\nb\nc\n");
        let err = drain_lines(&mut buffer, |line| {
            if line == "b" {
                anyhow::bail!("boom")
            }
            Ok(())
        })
        .unwrap_err();
        assert_eq!(err.to_string(), "boom");
    }
}
