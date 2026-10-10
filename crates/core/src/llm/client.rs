use crate::config::{
    glm_forces_thinking, supports_adaptive_thinking, AuthStyle, Config, ModelReasoning,
    ProviderKind, Reasoning,
};
use crate::llm::anthropic;
use crate::llm::types::{
    push_thinking, AssistantTurn, ChatRequest, FunctionCall, Message, StreamChunk, StreamOptions,
    ToolCall, ToolSpec, Usage,
};
use crate::llm::{aws, bedrock, copilot, gemini, gitlab, vertex};
use anyhow::{Context, Result};
use futures::StreamExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MODEL_CACHE_TTL_SECS: u64 = 24 * 60 * 60;
/// Bumped when the cached shape changes, so an entry written by an older build
/// is refetched instead of served without the newer fields (a pre-reasoning
/// cache deserializes `reasoning` as empty but would otherwise stay fresh for
/// the whole TTL, hiding a model's advertised levels).
const MODEL_CACHE_VERSION: u32 = 2;
const MODEL_CACHE_FILE: &str = "model-cache.json";
/// How many times a transient stream failure is retried before giving up.
const MAX_STREAM_ATTEMPTS: u32 = 3;
/// Ceiling for escalating `max_tokens` when a model reaches the output budget
/// before completing its response.
const MAX_ESCALATED_TOKENS: u32 = 32_768;
/// Cap how long a connect or a single streamed read may stall before the
/// request fails. Without this a dead proxy or dropped connection leaves the
/// agent waiting forever with no output.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(120);
static MODEL_CACHE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub struct LlmClient {
    http: reqwest::Client,
    config: Config,
    /// Session id used as a provider cache-affinity hint (`prompt_cache_key` for
    /// OpenAI, `x-session-id` for gateways). `None` disables the hint, as for
    /// one-off catalog and compaction requests.
    session_id: Option<String>,
}

/// The provider finished a turn without an answer: it either emitted nothing at
/// all, or spent the whole output budget on reasoning. `stream_chat` retries
/// before reporting this, so a caller that sent a best-effort request — the
/// hidden Definition-of-Done nudge — can recognize it and finish with the
/// answer the model already gave instead of failing the turn.
#[derive(Debug)]
pub struct NoAnswer {
    /// The output budget in force when reasoning consumed it.
    limit: Option<u32>,
}

impl NoAnswer {
    pub(crate) fn empty() -> Self {
        Self { limit: None }
    }

    pub(crate) fn reasoning(limit: u32) -> Self {
        Self { limit: Some(limit) }
    }
}

impl std::fmt::Display for NoAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.limit {
            Some(limit) => write!(
                f,
                "the model spent the entire output budget on reasoning and returned no answer \
                 (max_tokens = {limit})"
            ),
            None => f.write_str("the model returned an empty response"),
        }
    }
}

impl std::error::Error for NoAnswer {}

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
    /// DeepSeek (and other OpenAI-compatible catalogs) advertise the reasoning
    /// levels a model accepts beside its id. Missing on providers that do not.
    #[serde(default)]
    effort: Option<ModelEffort>,
}

#[derive(Debug, serde::Deserialize)]
struct ModelEffort {
    #[serde(default)]
    supported_levels: Vec<String>,
    #[serde(default)]
    default_level: Option<String>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct ModelCache {
    entries: BTreeMap<String, CachedModels>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedModels {
    updated_at: u64,
    models: Vec<String>,
    /// The shape this entry was written with (see [`MODEL_CACHE_VERSION`]).
    #[serde(default)]
    version: u32,
    /// Reasoning levels advertised per model id, when the listing carried them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    reasoning: BTreeMap<String, ModelReasoning>,
}

/// Callbacks the client invokes while a turn streams, so the caller can show
/// the work in progress: reasoning text as the model emits it, and transient
/// failures that are about to be retried.
///
/// A retried attempt re-sends its output from the start, so `text` and
/// `thinking` may repeat fragments a failed attempt already delivered;
/// consumers reset their view when `retry` fires. That is why a stream that
/// drops after emitting text is retried too, instead of failing the turn: the
/// failed attempt is discarded and the retry streams fresh.
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
        Self {
            http,
            config,
            session_id: None,
        }
    }

    /// Attaches the session id so providers can keep the conversation prefix in
    /// their prompt cache across turns.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
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
            Ok((models, reasoning)) => {
                if !models.is_empty() {
                    store_cached_models(&cache_key, &models, &reasoning);
                }
                Ok(self.config.merge_model_catalog(models))
            }
            Err(err) => match cached {
                Some(entry) => Ok(self.config.merge_model_catalog(entry.models)),
                None => Err(err),
            },
        }
    }

    /// Fetches the provider's catalog and writes it to the cache even when a
    /// cached entry is still fresh. The reasoning warmers use it, since a fresh
    /// entry that carries no effort metadata (a cache written before the field
    /// existed, or one the provider answered without it) must not keep them from
    /// asking again.
    pub async fn refresh_models(&self) -> Result<Vec<String>> {
        let (models, reasoning) = self.fetch_models().await?;
        if !models.is_empty() {
            store_cached_models(&model_cache_key(&self.config), &models, &reasoning);
        }
        Ok(self.config.merge_model_catalog(models))
    }

    /// The reasoning levels the provider advertised for `model`, read from the
    /// model cache. `None` when the listing carried none, the cache is cold, or
    /// the model is not in it.
    pub fn model_reasoning(&self, model: &str) -> Option<ModelReasoning> {
        cached_model_reasoning(&self.config, model)
    }

    async fn fetch_models(&self) -> Result<(Vec<String>, BTreeMap<String, ModelReasoning>)> {
        // A Copilot account is served from the endpoint its own session names,
        // and that session is what authorizes the listing too.
        let copilot = match self.config.auth_style() {
            AuthStyle::Copilot => {
                Some(copilot::session(&self.http, self.config.require_api_key()?).await?)
            }
            _ => None,
        };
        // The gateway token is what lists Duo's models, and the proxy in front
        // of them is the one place they are listed from.
        let gitlab = match self.config.auth_style() {
            AuthStyle::Gitlab => Some(
                gitlab::direct_access(
                    &self.http,
                    &self.config.gitlab_instance(),
                    self.config.require_api_key()?,
                )
                .await?,
            ),
            _ => None,
        };
        let mut url = match self.config.provider_kind() {
            // Bedrock lists foundation models over the control plane, which is a
            // different host from the one a turn streams from.
            ProviderKind::Bedrock => {
                let region = aws::region_from(&|name| std::env::var(name).ok());
                format!(
                    "https://{}/foundation-models",
                    aws::host(&region, "bedrock")
                )
            }
            _ => self.config.models_url()?,
        };
        if let Some(session) = &copilot {
            url = format!("{}/models", session.api);
        }
        // Duo lists its models through the gateway token, on the proxy the
        // model speaks — so a Claude model's listing is Anthropic's, with the
        // version header that wire asks for and without the instance token as
        // an `x-api-key`.
        let request = if let Some(access) = &gitlab {
            let request = gitlab_headers(self.http.get(&url), access);
            match self.config.provider_kind() {
                ProviderKind::Anthropic => {
                    request.header("anthropic-version", anthropic::API_VERSION)
                }
                _ => request,
            }
        } else {
            match self.config.provider_kind() {
                ProviderKind::Anthropic => self
                    .http
                    .get(&url)
                    .header("x-api-key", self.config.require_api_key()?)
                    .header("anthropic-version", anthropic::API_VERSION),
                ProviderKind::Gemini => {
                    let request = self.http.get(&url);
                    let request = match self.config.auth_style() {
                        AuthStyle::Vertex => {
                            let token =
                                vertex::access_token(&self.http, &self.config.api_key).await?;
                            request.bearer_auth(token)
                        }
                        _ => request.header("x-goog-api-key", self.config.require_api_key()?),
                    };
                    // The Gemini API puts the version in a header rather than the
                    // path, and refuses a call without one.
                    request.header("x-goog-api-version", "v1beta")
                }
                ProviderKind::Bedrock => {
                    self.sign_aws(self.http.get(&url), "GET", &url, None, b"", "bedrock")?
                }
                ProviderKind::OpenAi => match &copilot {
                    Some(session) => copilot_headers(self.http.get(&url), &session.token),
                    None => self.authenticate_openai(self.http.get(&url))?,
                },
            }
        };

        let response = request
            .send()
            .await
            .with_context(|| format!("requesting {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if self.config.is_portkey() && status == reqwest::StatusCode::FORBIDDEN {
                return Ok((self.config.model_catalog(), BTreeMap::new()));
            }
            // Z.AI does not document a model listing endpoint, so a missing or
            // restricted one falls back to the bundled catalog.
            if self.config.is_zai()
                && matches!(
                    status,
                    reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::NOT_FOUND
                )
            {
                return Ok((self.config.model_catalog(), BTreeMap::new()));
            }
            // Vertex has no listing an ordinary account can call, and a Bedrock
            // account without `bedrock:ListFoundationModels` answers the same
            // way, so both fall back to the bundled catalog rather than failing
            // the picker.
            if matches!(self.config.provider_kind(), ProviderKind::Bedrock)
                || self.config.auth_style() == AuthStyle::Vertex
            {
                return Ok((self.config.model_catalog(), BTreeMap::new()));
            }
            anyhow::bail!("provider returned {status}: {}", body.trim());
        }

        let value: Value = match response.json().await {
            Ok(value) => value,
            // The listing endpoint is undocumented at Z.AI, so an unexpected
            // shape falls back to the bundled catalog like a missing one.
            Err(_) if self.config.is_zai() => {
                return Ok((self.config.model_catalog(), BTreeMap::new()))
            }
            Err(err) => return Err(err).context("parsing model list"),
        };
        let mut models: Vec<String>;
        let mut reasoning: BTreeMap<String, ModelReasoning> = BTreeMap::new();
        match self.config.provider_kind() {
            ProviderKind::Gemini => models = gemini::parse_model_list(&value),
            ProviderKind::Bedrock => models = bedrock::parse_model_list(&value),
            _ => {
                let list: ModelList = serde_json::from_value(value)?;
                models = Vec::with_capacity(list.data.len());
                for model in list.data {
                    if let Some(effort) = model.effort {
                        if let Some(meta) = ModelReasoning::from_effort(
                            &effort.supported_levels,
                            effort.default_level.as_deref(),
                        ) {
                            reasoning.insert(model.id.clone(), meta);
                        }
                    }
                    models.push(model.id);
                }
            }
        }
        models.sort();
        models.dedup();
        Ok((models, reasoning))
    }

    /// Signs a request with AWS SigV4 for `service`, or attaches the Bedrock
    /// bearer token when one is configured instead — the two ways a Bedrock
    /// request is authorized. `url` is the request's own URL, whose host and
    /// path are what the signature covers.
    fn sign_aws(
        &self,
        request: reqwest::RequestBuilder,
        method: &str,
        url: &str,
        content_type: Option<&str>,
        payload: &[u8],
        service: &str,
    ) -> Result<reqwest::RequestBuilder> {
        if let Some(token) = non_empty_token(&self.config.api_key).or_else(|| {
            non_empty_token(&std::env::var("AWS_BEARER_TOKEN_BEDROCK").unwrap_or_default())
        }) {
            return Ok(request.bearer_auth(token));
        }
        let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
        let (host, path_and_query) = match rest.split_once('/') {
            Some((host, path)) => (host, format!("/{path}")),
            None => (rest, "/".to_string()),
        };
        let (path, query) = match path_and_query.split_once('?') {
            Some((path, query)) => (path, parse_query(query)),
            None => (path_and_query.as_str(), Vec::new()),
        };
        let credentials = aws::credentials_from(&|name| std::env::var(name).ok())
            .context("the Bedrock provider signs its requests with AWS SigV4")?;
        let region = aws::region_from(&|name| std::env::var(name).ok());
        let amz_date = aws::amz_timestamp(SystemTime::now());
        let headers = aws::sign(&aws::SigningRequest {
            method,
            uri: path,
            query: &query,
            host,
            content_type,
            payload,
            credentials: &credentials,
            region: &region,
            service,
            amz_date: &amz_date,
        });
        let mut request = request;
        for (name, value) in headers {
            request = request.header(name, value);
        }
        Ok(request)
    }

    /// Stream a chat completion, reporting text, reasoning, and retries through
    /// `hooks`. Returns the assembled assistant turn (text, thinking, and tool
    /// calls).
    ///
    /// Transient failures (network errors, truncated streams, rate limits and
    /// 5xx responses) are retried with backoff. A retry may follow an attempt
    /// that already streamed text or reasoning: the caller resets its view when
    /// `retry` fires, so the failed attempt is discarded rather than left to be
    /// extended by the new one.
    ///
    /// A turn whose provider stop reason is the output limit is retried with a
    /// larger `max_tokens` instead, since its text or tool arguments may have
    /// been cut off. A partial turn is never returned for the agent to act on.
    pub async fn stream_chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        hooks: &mut StreamHooks<'_>,
    ) -> Result<AssistantTurn> {
        let mut attempt = 0u32;
        let mut max_tokens = self.config.max_tokens;
        let mut escalated_from: Option<u32> = None;
        let mut discarded_usage = Usage::default();
        loop {
            attempt += 1;
            let result = {
                let mut attempt_hooks = StreamHooks {
                    text: &mut *hooks.text,
                    thinking: &mut *hooks.thinking,
                    retry: &mut *hooks.retry,
                };
                match self.config.provider_kind() {
                    ProviderKind::Anthropic => {
                        self.stream_anthropic(messages, tools, max_tokens, &mut attempt_hooks)
                            .await
                    }
                    ProviderKind::Gemini => {
                        self.stream_gemini(messages, tools, max_tokens, &mut attempt_hooks)
                            .await
                    }
                    ProviderKind::Bedrock => {
                        self.stream_bedrock(messages, tools, max_tokens, &mut attempt_hooks)
                            .await
                    }
                    ProviderKind::OpenAi => {
                        self.stream_openai(messages, tools, max_tokens, &mut attempt_hooks)
                            .await
                    }
                }
            }
            .map_err(|error| media_request_error(error, messages));
            match result {
                Ok(mut turn) => {
                    // A provider can hit the output limit after streaming part
                    // of an answer or tool call. The latter is especially
                    // dangerous: its arguments are incomplete JSON, but it
                    // still looks like a tool call to the agent. Discard and
                    // replay the whole response with a larger budget rather
                    // than executing a truncated call or displaying a partial
                    // answer as complete.
                    if turn_reached_output_limit(&turn, max_tokens) {
                        if max_tokens < MAX_ESCALATED_TOKENS && attempt < MAX_STREAM_ATTEMPTS {
                            add_usage(&mut discarded_usage, turn.usage);
                            escalated_from.get_or_insert(max_tokens);
                            max_tokens = (max_tokens * 2).min(MAX_ESCALATED_TOKENS);
                            let delay = Duration::from_millis(500 * 2u64.pow(attempt - 1));
                            (hooks.retry)(Retry {
                                attempt,
                                max: MAX_STREAM_ATTEMPTS,
                                delay,
                            });
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        if assistant_turn_is_empty(&turn) {
                            return Err(NoAnswer::reasoning(max_tokens).into());
                        }
                        anyhow::bail!(
                            "the model reached the output budget before completing its response \
                             (max_tokens = {max_tokens})"
                        );
                    }

                    // A turn with neither text nor tool calls carries nothing
                    // the agent can act on. It is usually a transient provider
                    // hiccup, so retry it like a dropped stream instead of
                    // failing the whole turn on the first empty response.
                    if assistant_turn_is_empty(&turn) {
                        // A reasoning model can spend the whole output budget on
                        // hidden reasoning and stop before it writes anything
                        // (`length` on OpenAI-compatible APIs, `max_tokens` on
                        // Anthropic). That is deterministic, so retrying the
                        // same budget just burns tokens again; raise it instead
                        // and only report failure once the ceiling is reached.
                        //
                        // Some gateways end such a turn with a normal stop
                        // reason, and OpenAI-family models never stream the
                        // reasoning text at all. A turn that produced nothing
                        // but reasoning - streamed or only billed in the usage -
                        // is treated as truncated too: it was the reasoning
                        // that consumed the budget.
                        let truncated = !turn.thinking.is_empty() || turn.usage.reasoning > 0;
                        if truncated
                            && max_tokens < MAX_ESCALATED_TOKENS
                            && attempt < MAX_STREAM_ATTEMPTS
                        {
                            add_usage(&mut discarded_usage, turn.usage);
                            escalated_from.get_or_insert(max_tokens);
                            max_tokens = (max_tokens * 2).min(MAX_ESCALATED_TOKENS);
                            let delay = Duration::from_millis(500 * 2u64.pow(attempt - 1));
                            (hooks.retry)(Retry {
                                attempt,
                                max: MAX_STREAM_ATTEMPTS,
                                delay,
                            });
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        if attempt < MAX_STREAM_ATTEMPTS && !truncated {
                            add_usage(&mut discarded_usage, turn.usage);
                            let delay = Duration::from_millis(500 * 2u64.pow(attempt - 1));
                            (hooks.retry)(Retry {
                                attempt,
                                max: MAX_STREAM_ATTEMPTS,
                                delay,
                            });
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        if truncated {
                            return Err(NoAnswer::reasoning(max_tokens).into());
                        }
                        return Err(NoAnswer::empty().into());
                    }
                    add_usage(&mut turn.usage, discarded_usage);
                    turn.usage.cost = self.config.usage_cost(&turn.usage);
                    return Ok(turn);
                }
                Err(err) => {
                    if !is_retryable(&err) {
                        // An escalated budget some models reject should still
                        // explain the truncation that triggered it.
                        if let Some(original) = escalated_from {
                            return Err(err.context(format!(
                                "the model exhausted the original output budget (max_tokens = \
                                 {original}) before completing its response"
                            )));
                        }
                        return Err(err);
                    }
                    if attempt >= MAX_STREAM_ATTEMPTS {
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
        max_tokens: u32,
        hooks: &mut StreamHooks<'_>,
    ) -> Result<AssistantTurn> {
        let mut url = self.config.chat_url()?;
        let mut config = self.config.clone();
        config.max_tokens = max_tokens;
        let request = openai_request(&config, messages, tools, self.session_id.as_deref());

        let mut builder = self.http.post(&url);
        if self.config.auth_style() == AuthStyle::Copilot {
            // The session names the endpoint this account is served from, which
            // is not always the public one.
            let session = copilot::session(&self.http, self.config.require_api_key()?).await?;
            url = format!("{}/chat/completions", session.api);
            builder = copilot_headers(builder, &session.token);
        } else if self.config.auth_style() == AuthStyle::Gitlab {
            // The stored token opens the instance, which mints the gateway
            // token the proxy accepts.
            let access = gitlab::direct_access(
                &self.http,
                &self.config.gitlab_instance(),
                self.config.require_api_key()?,
            )
            .await?;
            builder = gitlab_headers(builder, &access);
        } else {
            builder = self.authenticate_openai(builder)?;
        }
        if let Some(session) = self.session_id.as_deref() {
            // Cache-affinity hints for OpenAI-compatible gateways (OpenRouter,
            // litellm, ...). Direct OpenAI uses `prompt_cache_key` in the body.
            if !url.contains("api.openai.com") {
                builder = builder
                    .header("x-session-id", session)
                    .header("x-client-request-id", session);
            }
        }
        let response = builder
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

        let outcome = read_sse(response, |data| {
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
                    reasoning: usage
                        .completion_tokens_details
                        .as_ref()
                        .map(|details| details.reasoning_tokens)
                        .unwrap_or(0),
                    cost: 0.0,
                };
            }
            let Some(choice) = parsed.choices.into_iter().next() else {
                return Ok(());
            };
            if let Some(reason) = choice.finish_reason {
                if !reason.is_empty() {
                    turn.finish_reason = Some(reason);
                }
            }

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
                signature: None,
                function: FunctionCall {
                    name: p.name,
                    arguments: p.arguments,
                },
            })
            .collect();

        if stream_incomplete(
            &outcome,
            turn.finish_reason.as_deref(),
            !turn.content.is_empty() || !turn.tool_calls.is_empty(),
        ) {
            anyhow::bail!("provider stream ended before completing the response");
        }

        Ok(turn)
    }

    fn authenticate_openai(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder> {
        let key = self.config.require_api_key()?;
        match self.config.auth_style() {
            AuthStyle::Portkey => {
                let request = request.header("x-portkey-api-key", key);
                if self.config.portkey_config.trim().is_empty() {
                    Ok(request)
                } else {
                    Ok(request.header("x-portkey-config", self.config.portkey_config.trim()))
                }
            }
            // Azure names its key in a header of its own, and carries the
            // version in the URL rather than an `OpenAI-Beta` one.
            AuthStyle::Azure => Ok(request.header("api-key", key)),
            _ => Ok(request.bearer_auth(key)),
        }
    }

    async fn stream_anthropic(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        max_tokens: u32,
        hooks: &mut StreamHooks<'_>,
    ) -> Result<AssistantTurn> {
        let url = self.config.chat_url()?;
        let mut config = self.config.clone();
        config.max_tokens = max_tokens;
        let body = anthropic::request_body(&config, messages, tools);

        let mut request = self.http.post(&url);
        if self.config.auth_style() == AuthStyle::Gitlab {
            let access = gitlab::direct_access(
                &self.http,
                &self.config.gitlab_instance(),
                self.config.require_api_key()?,
            )
            .await?;
            request = gitlab_headers(request, &access)
                .header("anthropic-version", anthropic::API_VERSION);
        } else {
            request = request
                .header("x-api-key", self.config.require_api_key()?)
                .header("anthropic-version", anthropic::API_VERSION);
        }
        let response = request
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

        let outcome = read_sse(response, |data| {
            anthropic::apply_event(data, &mut turn, &mut partials, hooks.text, hooks.thinking)
        })
        .await?;

        turn.tool_calls = anthropic::into_tool_calls(partials);
        // Anthropic ends on `message_stop` rather than a `[DONE]` sentinel, and
        // its `message_delta` stop reason is what proves the turn finished. Only
        // a stream that ended without one is a dropped connection.
        if stream_incomplete(
            &outcome,
            turn.finish_reason.as_deref(),
            !assistant_turn_is_empty(&turn),
        ) {
            anyhow::bail!("provider stream ended before completing the response");
        }
        Ok(turn)
    }

    /// Stream a turn from Google's `generateContent` API, as served by the
    /// Gemini API and by Vertex AI. The two differ in host, path prefix and
    /// credential only, so the body and the event handling are shared.
    async fn stream_gemini(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        max_tokens: u32,
        hooks: &mut StreamHooks<'_>,
    ) -> Result<AssistantTurn> {
        let url = self.config.chat_url()?;
        let mut config = self.config.clone();
        config.max_tokens = max_tokens;
        let body = gemini::request_body(&config, messages, tools);

        let request = self.http.post(&url);
        let request = match self.config.auth_style() {
            AuthStyle::Vertex => {
                let token = vertex::access_token(&self.http, &self.config.api_key).await?;
                request.bearer_auth(token)
            }
            _ => request.header("x-goog-api-key", self.config.require_api_key()?),
        };
        let response = request
            .header("x-goog-api-version", "v1beta")
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

        let outcome = read_sse(response, |data| {
            gemini::apply_event(data, &mut turn, &mut partials, hooks.text, hooks.thinking)
        })
        .await?;

        turn.tool_calls = gemini::into_tool_calls(partials);
        // The stream ends with a `finishReason`, which is the same proof of
        // completion Anthropic's stop reason gives.
        if stream_incomplete(
            &outcome,
            turn.finish_reason.as_deref(),
            !assistant_turn_is_empty(&turn),
        ) {
            anyhow::bail!("provider stream ended before completing the response");
        }
        Ok(turn)
    }

    /// Stream a turn from AWS Bedrock's Converse API, signed with SigV4 (or a
    /// Bedrock bearer token when one is configured). The model is named in the
    /// path and the region in the host, so neither can come from `base_url`
    /// alone.
    async fn stream_bedrock(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        max_tokens: u32,
        hooks: &mut StreamHooks<'_>,
    ) -> Result<AssistantTurn> {
        let region = aws::region_from(&|name| std::env::var(name).ok());
        // The region names the host unless this run was pointed at an endpoint of
        // its own — a VPC endpoint, a gateway, a local emulator — which the
        // preset's base URL is for. The model is in the path either way.
        let base = match self.config.base_url.trim_end_matches('/') {
            "" => format!("https://{}", aws::host(&region, "bedrock-runtime")),
            endpoint => endpoint.to_string(),
        };
        let model = aws::encode_path_segment(&self.config.model);
        let url = format!("{base}/model/{model}/converse-stream");
        let mut config = self.config.clone();
        config.max_tokens = max_tokens;
        let body = bedrock::request_body(&config, messages, tools);
        let payload = serde_json::to_vec(&body)?;

        let request = self
            .http
            .post(&url)
            .header("accept", bedrock::EVENT_STREAM)
            .header("content-type", "application/json");
        let request = self.sign_aws(
            request,
            "POST",
            &url,
            Some("application/json"),
            &payload,
            "bedrock",
        )?;
        let response = request
            .body(payload)
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
        let outcome = read_event_stream(response, |event, payload| {
            bedrock::apply_event(
                event,
                payload,
                &mut turn,
                &mut partials,
                hooks.text,
                hooks.thinking,
            )?;
            Ok(event == bedrock::STOP_EVENT)
        })
        .await?;

        turn.tool_calls = bedrock::into_tool_calls(partials);
        if stream_incomplete(
            &outcome,
            turn.finish_reason.as_deref(),
            !assistant_turn_is_empty(&turn),
        ) {
            anyhow::bail!("provider stream ended before completing the response");
        }
        Ok(turn)
    }
}

impl CachedModels {
    fn is_fresh(&self) -> bool {
        self.version == MODEL_CACHE_VERSION
            && now_secs().saturating_sub(self.updated_at) < MODEL_CACHE_TTL_SECS
    }
}

/// A credential that is actually there, so a blank one falls through to the
/// next source rather than being sent as a bearer token.
fn non_empty_token(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The headers a GitHub Copilot request is authorized and identified with. The
/// session token is a bearer token of its own, and Copilot refuses a request
/// that does not say which client made it.
fn copilot_headers(request: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    request
        .header("authorization", format!("Bearer {token}"))
        .header("copilot-integration-id", copilot::INTEGRATION_ID)
        .header("editor-version", copilot::EDITOR_VERSION)
        .header("editor-plugin-version", copilot::PLUGIN_VERSION)
        .header("x-github-api-version", "2025-04-01")
}

/// The headers a GitLab gateway request carries: the token the instance minted,
/// and whatever headers came back beside it, since the proxy asks for those by
/// name rather than accepting any client.
fn gitlab_headers(
    mut request: reqwest::RequestBuilder,
    access: &gitlab::DirectAccess,
) -> reqwest::RequestBuilder {
    for (name, value) in &access.headers {
        request = request.header(name.as_str(), value.as_str());
    }
    request.bearer_auth(&access.token)
}

/// The query of a URL as the pairs SigV4 canonicalizes, so a signed request and
/// the URL it is sent to agree on what it asks for.
fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (name.to_string(), value.to_string()),
            None => (pair.to_string(), String::new()),
        })
        .collect()
}

/// The reasoning levels `model` advertised in the provider's listing, read
/// from the model cache without making a request. `None` when the listing
/// carried none, the cache is cold, or the model is not in it.
pub fn cached_model_reasoning(config: &Config, model: &str) -> Option<ModelReasoning> {
    cached_models(&model_cache_key(config)).and_then(|entry| entry.reasoning.get(model).cloned())
}

fn model_cache_key(config: &Config) -> String {
    let identity = format!(
        "{}\n{}\n{}",
        config.provider, config.base_url, config.portkey_config
    );
    format!("{:x}", Sha256::digest(identity.as_bytes()))
}

fn model_cache_path() -> Option<PathBuf> {
    Some(crate::config::config_dir()?.join(MODEL_CACHE_FILE))
}

fn cached_models(key: &str) -> Option<CachedModels> {
    let path = model_cache_path()?;
    let _guard = MODEL_CACHE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .ok()?;
    load_model_cache(&path).entries.get(key).cloned()
}

fn store_cached_models(key: &str, models: &[String], reasoning: &BTreeMap<String, ModelReasoning>) {
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
            version: MODEL_CACHE_VERSION,
            reasoning: reasoning.clone(),
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

fn openai_request(
    config: &Config,
    messages: &[Message],
    tools: &[ToolSpec],
    session_id: Option<&str>,
) -> ChatRequest {
    // DeepSeek's thinking mode requires every assistant message in the history
    // to carry `reasoning_content` -- not only turns that made a tool call but
    // plain final answers too, and not only turns that thought. An assistant
    // turn missing the field earns a 400 ("The `reasoning_content` in the
    // thinking mode must be passed back to the API."), which `deepseek-flash`
    // hits routinely because it skips reasoning on quick tool calls and short
    // answers. So the field is set on every assistant message: the captured
    // reasoning when there is one, an empty string when the turn thought
    // nothing. Other OpenAI-compatible providers get the thinking blocks
    // stripped instead, since the field would be an unknown argument there.
    let replay_reasoning = config.is_deepseek();
    // Every provider rejects an assistant tool-call turn without its results,
    // and a stored thread can hold one (a run killed mid-tool, a truncated
    // file), so the wire copy is repaired before it is serialized.
    let paired = crate::llm::repair_tool_pairs(messages);
    let messages: Vec<Value> = paired
        .iter()
        .map(|message| {
            let mut value = serde_json::to_value(message).unwrap_or(Value::Null);
            let Some(object) = value.as_object_mut() else {
                return value;
            };
            object.remove("thinking");
            if replay_reasoning && message.role == "assistant" {
                object.insert(
                    "reasoning_content".to_string(),
                    Value::String(message.reasoning_content().unwrap_or_default()),
                );
            }
            value
        })
        .collect();
    // Anthropic prompt caching behind an OpenAI-compatible gateway is opt-in
    // via `cache_control` blocks, matching the native Anthropic path.
    let anthropic_cache = anthropic_cache_control(config);
    let messages = openai_messages(messages, anthropic_cache);
    let (reasoning_effort, thinking, output_config) = openai_reasoning(config);
    // Direct OpenAI routes a cached prefix by `prompt_cache_key`; gateways use
    // the session headers instead, so only send it for `api.openai.com`.
    let prompt_cache_key = session_id
        .filter(|_| config.base_url.contains("api.openai.com"))
        .map(|id| id.chars().take(64).collect());
    ChatRequest {
        model: config.model.clone(),
        messages,
        stream: true,
        max_tokens: config.max_tokens,
        tools: openai_tools(tools, anthropic_cache),
        reasoning_effort,
        thinking,
        output_config,
        prompt_cache_key,
        stream_options: Some(StreamOptions {
            include_usage: true,
        }),
    }
}

/// `cache_control` markers are only understood by providers that proxy Anthropic
/// (OpenRouter/Portkey with a Claude model). A plain OpenAI-compatible provider
/// may reject an unknown field, so they are limited to a Claude model on a
/// non-OpenAI endpoint.
fn anthropic_cache_control(config: &Config) -> bool {
    config.model.to_ascii_lowercase().contains("claude")
        && !config.base_url.contains("api.openai.com")
}

fn cache_control() -> Value {
    serde_json::json!({ "type": "ephemeral" })
}

/// Serializes messages for OpenAI, optionally marking the system prompt and the
/// newest turn so a gateway can cache the prefix up to them.
fn openai_messages(messages: Vec<Value>, cache: bool) -> Vec<Value> {
    let mut messages = messages;
    if !cache {
        return messages;
    }
    if let Some(system) = messages
        .iter_mut()
        .find(|message| message["role"] == "system")
    {
        let text = system["content"].as_str().map(str::to_string);
        if let Some(text) = text {
            system["content"] = serde_json::json!([{
                "type": "text",
                "text": text,
                "cache_control": cache_control(),
            }]);
        }
    }
    if let Some(last) = messages.iter_mut().rev().find(|message| {
        matches!(
            message["role"].as_str(),
            Some("user" | "assistant" | "tool")
        )
    }) {
        cache_control_message(last);
    }
    messages
}

/// Marks the last text block of a message, converting string content to the
/// block form a gateway needs for a cache breakpoint.
fn cache_control_message(message: &mut Value) {
    match message.get_mut("content") {
        Some(Value::String(text)) => {
            let text = std::mem::take(text);
            message["content"] = serde_json::json!([{
                "type": "text",
                "text": text,
                "cache_control": cache_control(),
            }]);
        }
        Some(Value::Array(parts)) => {
            if let Some(block) = parts.iter_mut().rev().find(|block| block["type"] == "text") {
                block["cache_control"] = cache_control();
            }
        }
        _ => {}
    }
}

fn openai_tools(tools: &[ToolSpec], cache: bool) -> Option<Vec<Value>> {
    if tools.is_empty() {
        return None;
    }
    let mut specs: Vec<Value> = tools
        .iter()
        .map(|tool| serde_json::to_value(tool).unwrap_or(Value::Null))
        .collect();
    if cache {
        if let Some(last) = specs.last_mut() {
            last["cache_control"] = cache_control();
        }
    }
    Some(specs)
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
    if config.is_deepseek_api() {
        return deepseek_reasoning(config);
    }
    let adaptive = config.is_portkey() && supports_adaptive_thinking(&config.model);
    match config.effective_reasoning() {
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

/// DeepSeek's own API toggles thinking with `thinking.type` and accepts only
/// `low`, `high` and `max` as a `reasoning_effort`: `off` has to disable the
/// object explicitly (omitting it leaves the model's default, which thinks),
/// and oxide's `medium` maps onto `high` rather than being sent as a level the
/// API does not know and silently falling back to that default.
fn deepseek_reasoning(
    config: &Config,
) -> (
    Option<String>,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
) {
    let effort = |level: &str| Some(level.to_string());
    let enabled = || Some(serde_json::json!({ "type": "enabled" }));
    match config.effective_reasoning() {
        Reasoning::Auto => (None, None, None),
        Reasoning::Off => (None, Some(serde_json::json!({ "type": "disabled" })), None),
        Reasoning::Minimal | Reasoning::Low => (effort("low"), enabled(), None),
        Reasoning::Medium | Reasoning::High => (effort("high"), enabled(), None),
        Reasoning::Xhigh | Reasoning::Max => (effort("max"), enabled(), None),
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
    match config.effective_reasoning() {
        Reasoning::Auto => (None, None, None),
        Reasoning::Off if glm_forces_thinking(&config.model) => (effort("low"), enabled(), None),
        Reasoning::Off => (None, Some(serde_json::json!({ "type": "disabled" })), None),
        Reasoning::Minimal | Reasoning::Low => (effort("low"), enabled(), None),
        Reasoning::Medium | Reasoning::High => (effort("high"), enabled(), None),
        Reasoning::Xhigh | Reasoning::Max => (effort("max"), enabled(), None),
    }
}

async fn read_sse<F>(response: reqwest::Response, mut on_data: F) -> Result<SseOutcome>
where
    F: FnMut(&str) -> Result<()>,
{
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    let mut completed = false;
    let mut abrupt = false;

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(err) => {
                // A `[DONE]` sentinel proves the turn finished, so a connection
                // dropped afterwards is not a failure. A rustls unexpected EOF
                // (a TLS close without `close_notify`, which several providers
                // and proxies do) is otherwise remembered so the caller can tell
                // a complete turn from one the connection cut short.
                if completed {
                    break;
                }
                if is_abrupt_stream_close(&err) {
                    abrupt = true;
                    break;
                }
                return Err(err).context("reading response stream");
            }
        };
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

    Ok(SseOutcome { completed, abrupt })
}

/// Read one response body framed as binary event-stream messages (AWS's
/// `application/vnd.amazon.eventstream`), handing every whole frame's event name
/// and JSON payload to `on_event`. The counterpart of `read_sse` for a provider
/// whose stream is not made of lines: `on_event` reports whether the event it
/// was handed ends the stream, which is what a `[DONE]` sentinel is to SSE.
async fn read_event_stream<F>(response: reqwest::Response, mut on_event: F) -> Result<SseOutcome>
where
    F: FnMut(&str, &serde_json::Value) -> Result<bool>,
{
    let mut buffer: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    let mut completed = false;
    let mut abrupt = false;

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(err) => {
                if completed {
                    break;
                }
                if is_abrupt_stream_close(&err) {
                    abrupt = true;
                    break;
                }
                return Err(err).context("reading response stream");
            }
        };
        buffer.extend_from_slice(&chunk);
        bedrock::drain_frames(&mut buffer, |event, payload| {
            completed |= on_event(event, payload)?;
            Ok(())
        })?;
    }

    // Bytes still in the buffer are a frame that never finished arriving, so the
    // stream was cut short even if it did not say so on the way out.
    Ok(SseOutcome {
        completed,
        abrupt: abrupt || !buffer.is_empty(),
    })
}

/// How an SSE body ended: whether the `[DONE]` sentinel arrived, and whether
/// the connection dropped before the body was framed complete.
struct SseOutcome {
    completed: bool,
    abrupt: bool,
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

/// Whether a dropped response stream is a connection that ended without a TLS
/// `close_notify` shutdown. Several providers and proxies close an SSE response
/// this way; rustls reports the missing shutdown as an unexpected EOF. Only that
/// TLS wording counts: a generic truncated body (`Content-Length` unmet, a
/// missing chunk) is a real truncation and stays an error unless a `[DONE]`
/// sentinel or stop reason already proved the turn finished.
fn is_abrupt_stream_close(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(error) = current {
        // rustls reports a missing TLS shutdown as "peer closed connection
        // without sending TLS close_notify". Matching that wording keeps a
        // generic truncated body (a `Content-Length` that was not met) an error.
        if error
            .to_string()
            .to_ascii_lowercase()
            .contains("close_notify")
        {
            return true;
        }
        current = error.source();
    }
    false
}

/// Whether a stream that ended without `[DONE]` and without a stop reason is
/// incomplete. An abrupt TLS close after a few content chunks is as truncated as
/// an empty turn; a clean close with content is tolerated for providers that
/// omit the sentinel.
fn stream_incomplete(outcome: &SseOutcome, finish_reason: Option<&str>, has_payload: bool) -> bool {
    finish_reason.is_none() && !outcome.completed && (outcome.abrupt || !has_payload)
}

/// Whether a completed turn carries nothing the agent can act on.
fn assistant_turn_is_empty(turn: &AssistantTurn) -> bool {
    turn.content.trim().is_empty() && turn.tool_calls.is_empty()
}

/// Whether the provider exhausted the response budget. Gateways do not always
/// preserve the upstream stop reason, so a billed output count at the configured
/// ceiling is the fallback signal that a seemingly normal response was cut off.
fn turn_reached_output_limit(turn: &AssistantTurn, max_tokens: u32) -> bool {
    turn.finish_reason.as_deref().is_some_and(is_output_limit)
        || (max_tokens > 0 && turn.usage.output >= u64::from(max_tokens))
}

fn add_usage(total: &mut Usage, usage: Usage) {
    total.input += usage.input;
    total.output += usage.output;
    total.cache_read += usage.cache_read;
    total.cache_write += usage.cache_write;
    total.reasoning += usage.reasoning;
}

/// Adds an actionable hint to any failed request that carried native media.
/// Capability is deliberately not inferred from a provider or endpoint name:
/// the selected LLM is the source of truth, and its capabilities can change
/// independently of Oxide.
fn media_request_error(error: anyhow::Error, messages: &[Message]) -> anyhow::Error {
    let has_media = messages.iter().any(|message| {
        matches!(
            message.content.as_ref(),
            Some(crate::llm::MessageContent::Parts(parts))
                if parts.iter().any(|part| matches!(
                    part,
                    crate::llm::ContentPart::ImageUrl { .. } | crate::llm::ContentPart::File { .. }
                ))
        )
    });
    if has_media {
        error.context(
            "the request included rich media attachments; confirm that the selected LLM supports them",
        )
    } else {
        error
    }
}

/// Whether the provider stopped because the output token budget ran out
/// (OpenAI's `length`, Anthropic's `max_tokens`) rather than reaching a
/// natural stop.
fn is_output_limit(reason: &str) -> bool {
    matches!(reason, "length" | "max_tokens")
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
            signature: None,
            function: FunctionCall {
                name: "read".into(),
                arguments: "{}".into(),
            },
        }];
        assert!(!assistant_turn_is_empty(&turn));
    }

    #[test]
    fn output_limit_stop_reasons_are_recognized() {
        assert!(is_output_limit("length"));
        assert!(is_output_limit("max_tokens"));
        assert!(!is_output_limit("stop"));
        assert!(!is_output_limit("tool_calls"));
        assert!(!is_output_limit("end_turn"));
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
    fn media_failures_explain_that_the_selected_llm_may_not_support_attachments() {
        let image = crate::llm::ContentPart::ImageUrl {
            image_url: crate::llm::ImageUrl {
                url: "data:image/png;base64,AAAA".into(),
                detail: None,
            },
        };
        let messages = vec![Message::user_parts("look", vec![image])];
        let error = media_request_error(
            anyhow::anyhow!("provider returned 400 Bad Request: unsupported content"),
            &messages,
        );
        assert!(error.to_string().contains("confirm that the selected LLM"));
        assert!(!is_retryable(&error));

        let plain = media_request_error(
            anyhow::anyhow!("provider unavailable"),
            &[Message::user("hi")],
        );
        assert_eq!(plain.to_string(), "provider unavailable");
    }

    #[test]
    fn provider_selection_does_not_change_native_image_parts() {
        let image = crate::llm::ContentPart::ImageUrl {
            image_url: crate::llm::ImageUrl {
                url: "data:image/png;base64,AAAA".into(),
                detail: Some("high".into()),
            },
        };
        let messages = vec![Message::user_parts("look", vec![image])];

        for provider in ["openai", "deepseek", "portkey", "zai", "custom"] {
            let config = Config {
                provider: provider.into(),
                model: "model".into(),
                ..Config::default()
            };
            let body = serde_json::to_value(openai_request(&config, &messages, &[], None)).unwrap();
            let content = body["messages"][0]["content"].as_array().unwrap();
            assert_eq!(content[1]["type"], "image_url", "provider: {provider}");
            assert_eq!(
                content[1]["image_url"]["url"], "data:image/png;base64,AAAA",
                "provider: {provider}"
            );
        }
    }

    #[test]
    fn a_dangling_tool_call_is_repaired_before_the_request_is_sent() {
        // A log that holds an assistant call with no result would be rejected
        // by the API, so the wire copy carries a synthetic result for it.
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-chat".into(),
            ..Config::default()
        };
        let call = ToolCall {
            id: "call_0".into(),
            kind: "function".into(),
            signature: None,
            function: FunctionCall {
                name: "read".into(),
                arguments: "{}".into(),
            },
        };
        let body = serde_json::to_value(openai_request(
            &config,
            &[
                Message::user("do it"),
                Message::assistant("", vec![call]),
                Message::user("again"),
            ],
            &[],
            None,
        ))
        .unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "call_0");
        assert_eq!(messages[3]["role"], "user");
    }

    #[test]
    fn deepseek_replays_reasoning_content() {
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            ..Config::default()
        };
        let call = ToolCall {
            id: "call_0".into(),
            kind: "function".into(),
            signature: None,
            function: FunctionCall {
                name: "read".into(),
                arguments: "{}".into(),
            },
        };
        let assistant = Message::assistant("answer", vec![call]).with_thinking(vec![
            serde_json::json!({"type": "thinking", "thinking": "step one "}),
            serde_json::json!({"type": "thinking", "thinking": "step two"}),
        ]);
        // A final answer that made no tool call still has to replay its
        // reasoning: DeepSeek requires every earlier assistant turn to carry
        // `reasoning_content` while the request uses tools.
        let answer = Message::assistant("done", vec![]).with_thinking(vec![
            serde_json::json!({"type": "thinking", "thinking": "final trace"}),
        ]);
        let body = serde_json::to_value(openai_request(
            &config,
            &[
                Message::user("hi"),
                assistant,
                Message::tool("call_0", "contents"),
                answer,
            ],
            &[],
            None,
        ))
        .unwrap();
        let call_turn = &body["messages"][1];
        let answer_turn = &body["messages"][3];
        assert_eq!(call_turn["reasoning_content"], "step one step two");
        assert_eq!(answer_turn["reasoning_content"], "final trace");
        assert!(call_turn.get("thinking").is_none());
        assert!(answer_turn.get("thinking").is_none());

        // A turn that produced no reasoning still sends the field, empty:
        // DeepSeek rejects an assistant message that leaves it out even when
        // the turn thought nothing (a quick tool call, a short answer).
        let body = serde_json::to_value(openai_request(
            &config,
            &[Message::assistant("plain", vec![])],
            &[],
            None,
        ))
        .unwrap();
        assert_eq!(body["messages"][0]["reasoning_content"], "");
    }

    #[test]
    fn deepseek_replays_reasoning_through_a_gateway_too() {
        // A DeepSeek model behind a gateway is served by another provider but
        // is the same thinking mode, so the same field applies.
        let config = Config {
            provider: "openrouter".into(),
            model: "deepseek/deepseek-v4-flash".into(),
            ..Config::default()
        };
        let body = serde_json::to_value(openai_request(
            &config,
            &[Message::assistant("hi", vec![])],
            &[],
            None,
        ))
        .unwrap();
        assert_eq!(body["messages"][0]["reasoning_content"], "");
    }

    #[test]
    fn deepseek_replays_reasoning_for_every_assistant_turn() {
        // The thinking mode requires every earlier assistant turn's reasoning
        // back, so a conversation that mixes a tool-call turn and a plain
        // answer replays each one on the next request.
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-v4-pro".into(),
            ..Config::default()
        };
        let call = ToolCall {
            id: "call_0".into(),
            kind: "function".into(),
            signature: None,
            function: FunctionCall {
                name: "bash".into(),
                arguments: "{}".into(),
            },
        };
        let messages = vec![
            Message::user("do the thing"),
            Message::assistant("", vec![call]).with_thinking(vec![
                serde_json::json!({"type": "thinking", "thinking": "first"}),
            ]),
            Message::tool("call_0", "done"),
            Message::assistant("all set", vec![]).with_thinking(vec![
                serde_json::json!({"type": "thinking", "thinking": "second"}),
            ]),
        ];
        let body = serde_json::to_value(openai_request(&config, &messages, &[], None)).unwrap();
        let assistants: Vec<&Value> = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "assistant")
            .collect();
        assert_eq!(assistants.len(), 2);
        assert_eq!(assistants[0]["reasoning_content"], "first");
        assert_eq!(assistants[1]["reasoning_content"], "second");
    }

    #[test]
    fn other_providers_strip_thinking_blocks() {
        let config = Config {
            provider: "openai".into(),
            ..Config::default()
        };
        let assistant = Message::assistant("answer", vec![]).with_thinking(vec![
            serde_json::json!({"type": "thinking", "thinking": "trace"}),
        ]);
        let body = serde_json::to_value(openai_request(&config, &[assistant], &[], None)).unwrap();
        assert!(body["messages"][0].get("reasoning_content").is_none());
        assert!(body["messages"][0].get("thinking").is_none());
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

    /// The Portkey listing is a gateway call like any other, so `/models` is
    /// authenticated with Portkey's own header — plus the Config ID, so the
    /// catalog offered is the one that config routes to.
    #[tokio::test]
    async fn a_portkey_model_listing_uses_the_gateways_own_key() {
        let (addr, server) = json_server(vec![serde_json::json!({"data": [
            {"id": "gpt-5.4", "effort": {
                "supported_levels": ["minimal", "low", "medium", "high"],
                "default_level": "medium",
            }},
            {"id": "claude-sonnet-5"},
        ]})
        .to_string()])
        .await;
        let config = Config {
            provider: "portkey".into(),
            model: "claude-sonnet-5".into(),
            base_url: format!("http://{addr}/v1"),
            api_key: "pk-test".into(),
            portkey_config: "pc-test".into(),
            ..Config::default()
        };
        let client = LlmClient::new(config);
        let (models, reasoning) = client.fetch_models().await.unwrap();

        assert_eq!(models, ["claude-sonnet-5", "gpt-5.4"]);
        let advertised = reasoning.get("gpt-5.4").unwrap();
        assert_eq!(
            advertised.supported,
            vec![
                Reasoning::Minimal,
                Reasoning::Low,
                Reasoning::Medium,
                Reasoning::High
            ]
        );
        assert_eq!(advertised.default, Some(Reasoning::Medium));
        assert!(!reasoning.contains_key("claude-sonnet-5"));
        let sent = server.await.unwrap().remove(0);
        assert!(sent.starts_with("GET /v1/models"), "{sent}");
        let lower = sent.to_ascii_lowercase();
        assert!(lower.contains("x-portkey-api-key: pk-test"), "{sent}");
        assert!(lower.contains("x-portkey-config: pc-test"), "{sent}");
        assert!(!lower.contains("authorization"), "{sent}");
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
                serde_json::to_value(openai_request(&config, &[Message::user("hi")], &[], None))
                    .unwrap();
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
        let body = serde_json::to_value(openai_request(&config, &[Message::user("hi")], &[], None))
            .unwrap();
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
            provider: "custom".into(),
            model: "reasoning-model".into(),
            reasoning: Reasoning::Low,
            ..Config::default()
        };
        assert_eq!(openai_reasoning(&config), (Some("low".into()), None, None));
    }

    #[test]
    fn advertised_levels_clamp_the_effort_a_request_sends() {
        let config = Config {
            provider: "custom".into(),
            model: "reasoning-model".into(),
            reasoning: Reasoning::Medium,
            reasoning_supported: Some(ModelReasoning {
                supported: vec![Reasoning::Low, Reasoning::High],
                default: Some(Reasoning::High),
            }),
            ..Config::default()
        };
        // The model advertised no `medium`, so the request walks up to `high`.
        assert_eq!(openai_reasoning(&config), (Some("high".into()), None, None));
        let body = serde_json::to_value(openai_request(&config, &[Message::user("hi")], &[], None))
            .unwrap();
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn deepseek_toggles_thinking_and_maps_its_own_levels() {
        let config = |reasoning: Reasoning| Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            reasoning,
            ..Config::default()
        };
        // `off` has to disable thinking explicitly: a request that omits the
        // object leaves the model's own default, which thinks.
        assert_eq!(
            openai_reasoning(&config(Reasoning::Off)),
            (None, Some(serde_json::json!({ "type": "disabled" })), None)
        );
        assert_eq!(
            openai_reasoning(&config(Reasoning::Low)),
            (
                Some("low".into()),
                Some(serde_json::json!({ "type": "enabled" })),
                None
            )
        );
        // DeepSeek accepts only low/high/max, so `medium` maps onto `high`.
        assert_eq!(
            openai_reasoning(&config(Reasoning::Medium)),
            (
                Some("high".into()),
                Some(serde_json::json!({ "type": "enabled" })),
                None
            )
        );
        assert_eq!(
            openai_reasoning(&config(Reasoning::High)),
            (
                Some("high".into()),
                Some(serde_json::json!({ "type": "enabled" })),
                None
            )
        );
        assert_eq!(
            openai_reasoning(&config(Reasoning::Auto)),
            (None, None, None)
        );

        let body = serde_json::to_value(openai_request(
            &config(Reasoning::Off),
            &[Message::user("hi")],
            &[],
            None,
        ))
        .unwrap();
        assert_eq!(body["thinking"]["type"], "disabled");
        assert!(body.get("reasoning_effort").is_none());
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
                Some("high".into()),
                Some(serde_json::json!({ "type": "enabled" })),
                None
            )
        );
        assert_eq!(
            openai_reasoning(&config("glm-5.3", Reasoning::Max)),
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
            None,
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

    /// A Bedrock credential the reader stored is a bearer token, and it must be
    /// the one that authorizes the request — signing with SigV4 instead would
    /// ignore the only credential on the machine.
    #[test]
    fn a_stored_bedrock_credential_is_sent_as_a_bearer_token() {
        let config = Config {
            provider: "bedrock".into(),
            api_key: "bedrock-token".into(),
            model: "anthropic.claude-3-5-sonnet-20241022-v2:0".into(),
            ..Config::default()
        };
        let client = LlmClient::new(config);
        let url = "https://bedrock-runtime.us-east-1.amazonaws.com/model/m/converse-stream";
        let request = client
            .sign_aws(
                client.http.post(url),
                "POST",
                url,
                Some("application/json"),
                b"{}",
                "bedrock",
            )
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(request.headers()["authorization"], "Bearer bedrock-token");
        assert!(!request.headers().contains_key("x-amz-date"));
    }

    /// `converse-stream` answers with AWS's binary event-stream framing rather
    /// than SSE, so a turn read as lines would come back empty however well the
    /// events themselves parse. Each frame's event name and payload are read here
    /// however the body is chunked, the stop event is what marks the stream
    /// complete, and the events reach the turn the way the wire sends them.
    #[tokio::test]
    async fn a_bedrock_turn_is_read_from_its_event_stream_frames() {
        let body: Vec<u8> = [
            ("messageStart", r#"{"role":"assistant"}"#),
            (
                "contentBlockDelta",
                r#"{"contentBlockIndex":0,"delta":{"text":"hel"}}"#,
            ),
            (
                "contentBlockDelta",
                r#"{"contentBlockIndex":0,"delta":{"text":"lo"}}"#,
            ),
            ("contentBlockStop", r#"{"contentBlockIndex":0}"#),
            (bedrock::STOP_EVENT, r#"{"stopReason":"end_turn"}"#),
            (
                "metadata",
                r#"{"usage":{"inputTokens":10,"outputTokens":4}}"#,
            ),
        ]
        .into_iter()
        .flat_map(|(event, payload)| bedrock::frame(event, payload))
        .collect();
        let (addr, server) = framed_server(body).await;
        let config = Config {
            provider: "bedrock".into(),
            api_key: "bedrock-token".into(),
            model: "anthropic.claude-3-5-sonnet-20241022-v2:0".into(),
            base_url: format!("http://{addr}"),
            ..Config::default()
        };
        let client = LlmClient::new(config);

        let turn = {
            let mut hooks = StreamHooks {
                text: &mut |_| {},
                thinking: &mut |_| {},
                retry: &mut |_| {},
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap()
        };

        assert_eq!(turn.content, "hello");
        assert_eq!(turn.finish_reason.as_deref(), Some("end_turn"));
        assert_eq!(turn.usage.input, 10);
        assert_eq!(turn.usage.output, 4);
        let request = server.await.unwrap();
        assert!(
            request.starts_with(
                "POST /model/anthropic.claude-3-5-sonnet-20241022-v2%3A0/converse-stream"
            ),
            "{request}"
        );
        // The stream asked for is the one it answers with, and the model id is
        // encoded in the path the signature was made over, so a colon in it is
        // not the one difference between what is signed and what is sent.
        assert!(
            request
                .to_ascii_lowercase()
                .contains("accept: application/vnd.amazon.eventstream"),
            "{request}"
        );
    }

    /// A frame the connection ended in the middle of is a turn that was cut
    /// short, not a complete one that happened to stop saying anything.
    #[tokio::test]
    async fn a_frame_that_never_finished_arriving_is_truncation() {
        let mut body = bedrock::frame(
            "contentBlockDelta",
            r#"{"contentBlockIndex":0,"delta":{"text":"hi"}}"#,
        );
        body.extend_from_slice(
            &bedrock::frame(bedrock::STOP_EVENT, r#"{"stopReason":"end_turn"}"#)[..8],
        );
        let (addr, _server) = framed_server(body).await;
        let response = reqwest::Client::new()
            .post(format!("http://{addr}/model/m/converse-stream"))
            .body("{}")
            .send()
            .await
            .unwrap();

        let outcome = read_event_stream(response, |_, _| Ok(false)).await.unwrap();
        assert!(!outcome.completed);
        assert!(outcome.abrupt, "the frame left half-read is truncation");
        // Which is what the turn reports: text arrived, and no stop reason did.
        assert!(stream_incomplete(&outcome, None, true));
    }

    /// A Duo turn is authorized by a token the instance mints, not by the
    /// credential the reader stored: the stored one opens the instance, and
    /// what the gateway sees is what came back beside it.
    #[tokio::test]
    async fn a_gitlab_turn_presents_the_credential_to_the_instance_first() {
        let _env = crate::env_lock::hold();
        let (instance, instance_server) = sse_server(vec![serde_json::json!({
            "token": "gateway-jwt",
            "headers": { "X-Gitlab-Realm": "saas" }
        })
        .to_string()])
        .await;
        let (gateway, gateway_server) = sse_server(vec![completed_turn("hi")]).await;
        // The instance and the gateway are hosts the configuration names, and a
        // stub machine has neither.
        std::env::set_var("GITLAB_INSTANCE_URL", format!("http://{instance}"));
        std::env::set_var("GITLAB_AI_GATEWAY_URL", format!("http://{gateway}"));
        let config = Config {
            provider: "gitlab".into(),
            model: "gpt-4o".into(),
            base_url: String::new(),
            api_key: "glpat-secret".into(),
            ..Config::default()
        };
        let client = LlmClient::new(config);

        let turn = {
            let mut hooks = StreamHooks {
                text: &mut |_| {},
                thinking: &mut |_| {},
                retry: &mut |_| {},
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap()
        };
        std::env::remove_var("GITLAB_INSTANCE_URL");
        std::env::remove_var("GITLAB_AI_GATEWAY_URL");

        assert_eq!(turn.content, "hi");
        let exchange = instance_server.await.unwrap();
        assert_eq!(exchange.len(), 1);
        assert!(exchange[0].starts_with("POST /api/v4/ai/third_party_agents/direct_access"));
        assert!(
            exchange[0]
                .to_ascii_lowercase()
                .contains("authorization: bearer glpat-secret"),
            "{}",
            exchange[0]
        );

        let chat = gateway_server.await.unwrap();
        assert_eq!(chat.len(), 1);
        assert!(chat[0].starts_with("POST /ai/v1/proxy/openai/v1/chat/completions"));
        let sent = chat[0].to_ascii_lowercase();
        assert!(
            sent.contains("authorization: bearer gateway-jwt"),
            "{}",
            chat[0]
        );
        assert!(sent.contains("x-gitlab-realm: saas"), "{}", chat[0]);
    }

    /// Duo serves Claude through the gateway's Anthropic proxy, so the same
    /// minted token is presented on Anthropic's wire — with the version header
    /// Anthropic asks for and without the `x-api-key` a direct Anthropic turn
    /// would carry.
    #[tokio::test]
    async fn a_claude_model_is_proxied_to_anthropic_under_duo() {
        let _env = crate::env_lock::hold();
        let (instance, _instance_server) = sse_server(vec![
            serde_json::json!({"token": "gateway-jwt", "headers": {"X-Gitlab-Realm": "saas"}})
                .to_string(),
        ])
        .await;
        let (gateway, gateway_server) = sse_server(vec![anthropic_sse(&[
            serde_json::json!({"type": "message_start", "message": {"usage": {"input_tokens": 3, "output_tokens": 1}}}),
            serde_json::json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            serde_json::json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi"}}),
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 2}}),
            serde_json::json!({"type": "message_stop"}),
        ])])
        .await;
        std::env::set_var("GITLAB_INSTANCE_URL", format!("http://{instance}"));
        std::env::set_var("GITLAB_AI_GATEWAY_URL", format!("http://{gateway}"));
        let config = Config {
            provider: "gitlab".into(),
            model: "claude-sonnet-4-6".into(),
            base_url: String::new(),
            api_key: "glpat-secret".into(),
            ..Config::default()
        };
        let client = LlmClient::new(config);

        let turn = {
            let mut hooks = StreamHooks {
                text: &mut |_| {},
                thinking: &mut |_| {},
                retry: &mut |_| {},
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap()
        };
        std::env::remove_var("GITLAB_INSTANCE_URL");
        std::env::remove_var("GITLAB_AI_GATEWAY_URL");

        assert_eq!(turn.content, "hi");
        let sent = gateway_server.await.unwrap().remove(0);
        assert!(
            sent.starts_with("POST /ai/v1/proxy/anthropic/v1/messages"),
            "{sent}"
        );
        let lower = sent.to_ascii_lowercase();
        assert!(
            lower.contains("authorization: bearer gateway-jwt"),
            "{sent}"
        );
        assert!(lower.contains("x-gitlab-realm: saas"), "{sent}");
        assert!(lower.contains("anthropic-version: 2023-06-01"), "{sent}");
        assert!(!lower.contains("x-api-key"), "{sent}");
    }

    /// `/models` under Duo is the gateway's own listing, so a Claude model —
    /// which turns on Anthropic's wire — does not put the instance's personal
    /// access token on it as an `x-api-key`.
    #[tokio::test]
    async fn a_duo_model_listing_uses_the_minted_token() {
        let _env = crate::env_lock::hold();
        let (instance, _instance_server) = sse_server(vec![
            serde_json::json!({"token": "gateway-jwt", "headers": {"X-Gitlab-Realm": "saas"}})
                .to_string(),
        ])
        .await;
        let (gateway, gateway_server) = json_server(vec![
            serde_json::json!({"data": [{"id": "gpt-4o"}, {"id": "claude-sonnet-4-6"}]})
                .to_string(),
        ])
        .await;
        std::env::set_var("GITLAB_INSTANCE_URL", format!("http://{instance}"));
        std::env::set_var("GITLAB_AI_GATEWAY_URL", format!("http://{gateway}"));
        let config = Config {
            provider: "gitlab".into(),
            model: "claude-sonnet-4-6".into(),
            base_url: String::new(),
            api_key: "glpat-secret".into(),
            ..Config::default()
        };
        let client = LlmClient::new(config);
        let (models, _) = client.fetch_models().await.unwrap();
        std::env::remove_var("GITLAB_INSTANCE_URL");
        std::env::remove_var("GITLAB_AI_GATEWAY_URL");

        assert_eq!(models, ["claude-sonnet-4-6", "gpt-4o"]);
        let sent = gateway_server.await.unwrap().remove(0);
        assert!(
            sent.starts_with("GET /ai/v1/proxy/anthropic/v1/models"),
            "{sent}"
        );
        let lower = sent.to_ascii_lowercase();
        assert!(
            lower.contains("authorization: bearer gateway-jwt"),
            "{sent}"
        );
        assert!(lower.contains("x-gitlab-realm: saas"), "{sent}");
        assert!(lower.contains("anthropic-version: 2023-06-01"), "{sent}");
        assert!(!lower.contains("x-api-key"), "{sent}");
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
            models: vec!["model".into()],
            version: MODEL_CACHE_VERSION,
            reasoning: BTreeMap::new(),
        }
        .is_fresh());
        // An entry written by an older build is refetched rather than served
        // without the fields this build knows about.
        assert!(!CachedModels {
            updated_at: current,
            models: vec!["model".into()],
            version: MODEL_CACHE_VERSION - 1,
            reasoning: BTreeMap::new(),
        }
        .is_fresh());
        assert!(!CachedModels {
            updated_at: current.saturating_sub(MODEL_CACHE_TTL_SECS + 1),
            models: vec!["model".into()],
            version: MODEL_CACHE_VERSION,
            reasoning: BTreeMap::new(),
        }
        .is_fresh());
    }

    #[test]
    fn backoff_grows_with_each_attempt() {
        assert_eq!(MAX_STREAM_ATTEMPTS, 3);
    }

    #[test]
    fn openai_cache_key_is_sent_only_for_direct_openai() {
        let mut config = Config {
            api_key: "sk-test".into(),
            base_url: "https://api.openai.com/v1".into(),
            ..Config::default()
        };
        let body = serde_json::to_value(openai_request(
            &config,
            &[Message::user("hi")],
            &[],
            Some("session-123"),
        ))
        .unwrap();
        assert_eq!(body["prompt_cache_key"], "session-123");

        config.base_url = "https://api.portkey.ai/v1".into();
        let body = serde_json::to_value(openai_request(
            &config,
            &[Message::user("hi")],
            &[],
            Some("session-123"),
        ))
        .unwrap();
        assert!(body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn anthropic_cache_control_marks_a_gateway_claude_request() {
        let tool = |name: &str| ToolSpec {
            kind: "function",
            function: crate::llm::types::FunctionSpec {
                name: name.into(),
                description: "does a thing".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            },
        };
        let config = Config {
            provider: "portkey".into(),
            model: "claude-sonnet-5".into(),
            base_url: "https://api.portkey.ai/v1".into(),
            api_key: "pk-test".into(),
            ..Config::default()
        };
        let messages = vec![Message::system("be helpful"), Message::user("hi")];
        let body = serde_json::to_value(openai_request(
            &config,
            &messages,
            &[tool("bash"), tool("read")],
            Some("session-1"),
        ))
        .unwrap();

        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(
            body["messages"][1]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(body["tools"][1]["cache_control"]["type"], "ephemeral");
        assert!(body["tools"][0].get("cache_control").is_none());
        assert!(body.get("prompt_cache_key").is_none());

        // A non-Claude model on the same gateway keeps plain content.
        let config = Config {
            model: "gpt-5.6-sol".into(),
            ..config
        };
        let body = serde_json::to_value(openai_request(&config, &messages, &[tool("bash")], None))
            .unwrap();
        assert!(body["messages"][0]["content"].is_string());
        assert!(body["tools"][0].get("cache_control").is_none());
    }

    #[tokio::test]
    async fn gateway_requests_carry_the_session_cache_headers() {
        let (addr, server) = sse_server(vec![completed_turn("done")]).await;
        let client = LlmClient::new(sse_test_config(addr, 1024)).with_session_id("session-xyz");

        let mut text = String::new();
        let mut hooks = StreamHooks {
            text: &mut |delta: String| text.push_str(&delta),
            thinking: &mut |_| {},
            retry: &mut |_| {},
        };
        client
            .stream_chat(&[Message::user("hi")], &[], &mut hooks)
            .await
            .unwrap();

        let requests = server.await.unwrap();
        let request = requests[0].to_ascii_lowercase();
        assert!(
            request.contains("x-session-id: session-xyz"),
            "{}",
            requests[0]
        );
        assert!(
            request.contains("x-client-request-id: session-xyz"),
            "{}",
            requests[0]
        );
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

    /// Reads one complete HTTP request (headers and content-length body) from
    /// `socket`, returning the raw text.
    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        use tokio::io::AsyncReadExt;

        let mut raw = Vec::new();
        let mut expected: Option<usize> = None;
        loop {
            let mut chunk = [0u8; 8192];
            let read = match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            raw.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&raw);
            let Some(head_end) = text.find("\r\n\r\n") else {
                continue;
            };
            if expected.is_none() {
                expected = text[..head_end].lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
            }
            if let Some(len) = expected {
                if raw.len() - (head_end + 4) >= len {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&raw).into_owned()
    }

    /// Serves one SSE response per request, in order, and returns the raw
    /// request texts it saw so a test can assert on the request bodies.
    async fn sse_server(
        bodies: Vec<String>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for body in bodies {
                let (mut socket, _) = listener.accept().await.unwrap();
                seen.push(read_request(&mut socket).await);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
            seen
        });
        (addr, handle)
    }

    /// Serves one JSON body per request, reading only the request head: a
    /// bodyless `GET` never sends the end-of-body a body-reading stub waits for.
    async fn json_server(
        bodies: Vec<String>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for body in bodies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut chunk = [0u8; 8192];
                let read = socket.read(&mut chunk).await.unwrap_or(0);
                seen.push(String::from_utf8_lossy(&chunk[..read]).into_owned());
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
            seen
        });
        (addr, handle)
    }

    /// Serves a scripted sequence of SSE responses on one listener, in order,
    /// returning the raw request texts. A `truncated` entry declares a
    /// `content-length` larger than the body it writes and then drops the
    /// connection — how a stream ends when the peer closes without a TLS
    /// `close_notify`.
    async fn scripted_sse_server(
        responses: Vec<(String, bool)>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (body, truncated) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                seen.push(read_request(&mut socket).await);
                let declared = if truncated {
                    body.len() + 64
                } else {
                    body.len()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {declared}\r\nconnection: close\r\n\r\n{body}"
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
            seen
        });
        (addr, handle)
    }

    /// Serves one response body as raw bytes, in two writes so a frame can be
    /// met split across chunks, and returns the raw request text it saw. An
    /// event-stream body is binary, so `sse_server` cannot carry it.
    async fn framed_server(
        body: Vec<u8>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut chunk = [0u8; 8192];
            let read = socket.read(&mut chunk).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&chunk[..read]).into_owned();
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/vnd.amazon.eventstream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let split = body.len() / 2;
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(&body[..split]).await;
            let _ = socket.flush().await;
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let _ = socket.write_all(&body[split..]).await;
            let _ = socket.shutdown().await;
            request
        });
        (addr, handle)
    }

    fn sse(events: &[serde_json::Value]) -> String {
        let mut body = String::new();
        for event in events {
            body.push_str("data: ");
            body.push_str(&event.to_string());
            body.push_str("\n\n");
        }
        body.push_str("data: [DONE]\n\n");
        body
    }

    /// An empty turn that stopped on the output limit, as a reasoning model
    /// does when it spends the whole budget on hidden reasoning.
    fn length_limited_turn() -> String {
        sse(&[
            serde_json::json!({"choices": [{"index": 0, "delta": {"content": "", "reasoning_content": "thinking"}, "logprobs": null, "finish_reason": null}]}),
            serde_json::json!({"choices": [{"index": 0, "delta": {"content": "", "reasoning_content": null}, "logprobs": null, "finish_reason": "length"}], "usage": {"prompt_tokens": 10, "completion_tokens": 8192, "completion_tokens_details": {"reasoning_tokens": 8192}}}),
        ])
    }

    /// An empty turn that reasoned but stopped without reporting the output
    /// limit, as some gateways do when reasoning consumes the budget.
    fn reasoning_only_turn(finish: &str) -> String {
        sse(&[
            serde_json::json!({"choices": [{"index": 0, "delta": {"content": "", "reasoning_content": "thinking"}, "logprobs": null, "finish_reason": null}]}),
            serde_json::json!({"choices": [{"index": 0, "delta": {"content": "", "reasoning_content": null}, "logprobs": null, "finish_reason": finish}]}),
        ])
    }

    /// An empty turn whose only output was hidden reasoning reported in the
    /// usage, as OpenAI-family models do (they never stream the text).
    fn billed_reasoning_turn() -> String {
        sse(&[
            serde_json::json!({"choices": [{"index": 0, "delta": {"content": "", "reasoning_content": null}, "logprobs": null, "finish_reason": null}]}),
            serde_json::json!({"choices": [{"index": 0, "delta": {"content": "", "reasoning_content": null}, "logprobs": null, "finish_reason": "stop"}], "usage": {"prompt_tokens": 10, "completion_tokens": 8192, "completion_tokens_details": {"reasoning_tokens": 8192}}}),
        ])
    }

    fn completed_turn(content: &str) -> String {
        sse(&[
            serde_json::json!({"choices": [{"index": 0, "delta": {"content": content, "reasoning_content": null}, "logprobs": null, "finish_reason": null}]}),
            serde_json::json!({"choices": [{"index": 0, "delta": {"content": "", "reasoning_content": null}, "logprobs": null, "finish_reason": "stop"}]}),
        ])
    }

    fn completed_turn_with_usage(content: &str, input: u64, output: u64) -> String {
        sse(&[
            serde_json::json!({"choices": [{"index": 0, "delta": {"content": content, "reasoning_content": null}, "logprobs": null, "finish_reason": null}]}),
            serde_json::json!({"choices": [{"index": 0, "delta": {"content": "", "reasoning_content": null}, "logprobs": null, "finish_reason": "stop"}], "usage": {"prompt_tokens": input, "completion_tokens": output}}),
        ])
    }

    /// A tool call whose JSON arguments were cut off at the output limit.
    fn output_limited_tool_call(finish_reason: &str) -> String {
        sse(&[
            serde_json::json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_0", "function": {"name": "create_page", "arguments": "{\"title\":\"Report\""}}]}, "finish_reason": null}]}),
            serde_json::json!({"choices": [{"index": 0, "delta": {}, "finish_reason": finish_reason}], "usage": {"prompt_tokens": 10, "completion_tokens": 8192}}),
        ])
    }

    fn completed_tool_call() -> String {
        sse(&[
            serde_json::json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_0", "function": {"name": "create_page", "arguments": "{\"title\":\"Report\",\"body\":\"complete\"}"}}]}, "finish_reason": null}]}),
            serde_json::json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
        ])
    }

    fn request_budget(request: &str) -> u64 {
        let body = request.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
        serde_json::from_str::<serde_json::Value>(body).unwrap()["max_tokens"]
            .as_u64()
            .unwrap()
    }

    fn sse_test_config(addr: std::net::SocketAddr, max_tokens: u32) -> Config {
        Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            max_tokens,
            ..Config::default()
        }
    }

    fn portkey_test_config(addr: std::net::SocketAddr, max_tokens: u32) -> Config {
        Config {
            provider: "portkey".into(),
            model: "gpt-5.6-sol".into(),
            base_url: format!("http://{addr}"),
            api_key: "pk-test".into(),
            max_tokens,
            ..Config::default()
        }
    }

    fn anthropic_test_config(addr: std::net::SocketAddr, max_tokens: u32) -> Config {
        Config {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-5".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-ant-test".into(),
            max_tokens,
            ..Config::default()
        }
    }

    /// Anthropic has no `[DONE]` sentinel, so this leaves it out to exercise
    /// completion via the `message_delta` stop reason.
    fn anthropic_sse(events: &[serde_json::Value]) -> String {
        let mut body = String::new();
        for event in events {
            body.push_str("data: ");
            body.push_str(&event.to_string());
            body.push_str("\n\n");
        }
        body
    }

    #[test]
    fn abrupt_stream_closes_are_recognized_from_the_error_chain() {
        #[derive(Debug)]
        struct Chain(&'static str, Option<Box<Chain>>);
        impl std::fmt::Display for Chain {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.0)
            }
        }
        impl std::error::Error for Chain {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.1
                    .as_deref()
                    .map(|error| error as &(dyn std::error::Error + 'static))
            }
        }

        let tls = Chain(
            "error decoding response body",
            Some(Box::new(Chain(
                "request or response body error",
                Some(Box::new(Chain(
                    "error reading a body from connection: peer closed connection without \
                     sending TLS close_notify: https://docs.rs/rustls/",
                    None,
                ))),
            ))),
        );
        assert!(is_abrupt_stream_close(&tls));

        // A generic HTTP truncation is a real truncation, not a clean TLS end.
        assert!(!is_abrupt_stream_close(&Chain(
            "end of file before message length reached",
            None
        )));

        // A real provider error is not swallowed.
        assert!(!is_abrupt_stream_close(&Chain(
            "provider returned 500: server error",
            None
        )));
    }

    #[test]
    fn an_abrupt_close_with_content_is_still_incomplete() {
        let done = SseOutcome {
            completed: true,
            abrupt: false,
        };
        let clean = SseOutcome {
            completed: false,
            abrupt: false,
        };
        let abrupt = SseOutcome {
            completed: false,
            abrupt: true,
        };

        assert!(!stream_incomplete(&done, None, true));
        assert!(!stream_incomplete(&clean, Some("stop"), true));
        // A clean close with content and no stop reason is tolerated (some
        // providers omit `[DONE]`).
        assert!(!stream_incomplete(&clean, None, true));
        // An abrupt close with content but no stop reason is truncated.
        assert!(stream_incomplete(&abrupt, None, true));
        // ...unless the stop reason already proved the turn finished.
        assert!(!stream_incomplete(&abrupt, Some("stop"), true));
        assert!(stream_incomplete(&clean, None, false));
    }

    #[tokio::test]
    async fn a_stream_that_drops_after_done_keeps_the_turn() {
        // The provider sent its text and `[DONE]`, then the connection dropped
        // mid-body: the sentinel proves the turn finished, so the missing
        // shutdown must not replace it with a stream error.
        let body = sse(&[serde_json::json!(
            {"choices": [{"index": 0, "delta": {"content": "par"}}]}
        )]);
        let (addr, _server) = scripted_sse_server(vec![(body, true)]).await;
        let client = LlmClient::new(sse_test_config(addr, 8192));

        let mut retries = Vec::new();
        let turn = {
            let mut hooks = StreamHooks {
                text: &mut |_| {},
                thinking: &mut |_| {},
                retry: &mut |r: Retry| retries.push(r),
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap()
        };

        assert_eq!(turn.content, "par");
        assert!(retries.is_empty());
    }

    #[tokio::test]
    async fn a_truncated_stream_after_text_is_retried_from_scratch() {
        // The connection drops after the model streamed "par" and no stop
        // reason arrived: the attempt is incomplete, so it is retried instead
        // of being returned as a partial answer or failing the turn.
        let partial =
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"par\"}}]}\n\n".to_string();
        let (addr, server) =
            scripted_sse_server(vec![(partial, true), (completed_turn("done"), false)]).await;
        let client = LlmClient::new(sse_test_config(addr, 8192));

        let mut retries = Vec::new();
        let turn = {
            let mut hooks = StreamHooks {
                text: &mut |_| {},
                thinking: &mut |_| {},
                retry: &mut |r: Retry| retries.push(r),
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap()
        };

        assert_eq!(turn.content, "done");
        assert_eq!(retries.len(), 1);
        assert_eq!(retries[0].attempt, 1);
        assert_eq!(server.await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_truncated_anthropic_stream_after_text_is_retried() {
        // Anthropic has no `[DONE]`; the `message_delta` stop reason is what
        // proves completion, so a stream that drops after its text block is
        // retried from scratch like the OpenAI-compatible path.
        let partial = anthropic_sse(&[
            serde_json::json!({"type": "message_start", "message": {"usage": {"input_tokens": 10, "output_tokens": 1}}}),
            serde_json::json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            serde_json::json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "par"}}),
        ]);
        let answer = anthropic_sse(&[
            serde_json::json!({"type": "message_start", "message": {"usage": {"input_tokens": 10, "output_tokens": 1}}}),
            serde_json::json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            serde_json::json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "done"}}),
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 2}}),
            serde_json::json!({"type": "message_stop"}),
        ]);
        let (addr, server) = scripted_sse_server(vec![(partial, true), (answer, false)]).await;
        let client = LlmClient::new(anthropic_test_config(addr, 8192));

        let mut retries = Vec::new();
        let turn = {
            let mut hooks = StreamHooks {
                text: &mut |_| {},
                thinking: &mut |_| {},
                retry: &mut |r: Retry| retries.push(r),
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap()
        };

        assert_eq!(turn.content, "done");
        assert_eq!(retries.len(), 1);
        assert_eq!(server.await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_permanently_truncated_stream_errors_after_the_retry_budget() {
        // A stream that never completes still fails the turn once the retry
        // budget is spent, so a dead connection is not mistaken for an answer.
        let partial =
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"par\"}}]}\n\n".to_string();
        let (addr, server) = scripted_sse_server(vec![
            (partial.clone(), true),
            (partial.clone(), true),
            (partial, true),
        ])
        .await;
        let client = LlmClient::new(sse_test_config(addr, 8192));

        let err = {
            let mut hooks = StreamHooks {
                text: &mut |_| {},
                thinking: &mut |_| {},
                retry: &mut |_| {},
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap_err()
        };

        assert!(err.to_string().contains("reading response stream"), "{err}");
        assert_eq!(server.await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn an_output_limited_empty_turn_retries_with_a_larger_budget() {
        let (addr, server) = sse_server(vec![
            length_limited_turn(),
            completed_turn_with_usage("done", 7, 3),
        ])
        .await;
        let client = LlmClient::new(sse_test_config(addr, 8192));

        let mut text = String::new();
        let mut retries = Vec::new();
        let turn = {
            let mut on_text = |delta: String| text.push_str(&delta);
            let mut on_retry = |retry: Retry| retries.push(retry);
            let mut hooks = StreamHooks {
                text: &mut on_text,
                thinking: &mut |_| {},
                retry: &mut on_retry,
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap()
        };

        assert_eq!(turn.content, "done");
        assert_eq!(text, "done");
        assert_eq!(retries.len(), 1);
        assert_eq!(retries[0].attempt, 1);
        assert_eq!(turn.usage.input, 17);
        assert_eq!(turn.usage.output, 8195);
        assert_eq!(turn.usage.reasoning, 8192);
        assert_eq!(turn.usage.cost, client.config.usage_cost(&turn.usage));
        let requests = server.await.unwrap();
        let budgets: Vec<u64> = requests.iter().map(|r| request_budget(r)).collect();
        assert_eq!(budgets, vec![8192, 16384]);
    }

    #[tokio::test]
    async fn an_output_limited_tool_call_is_retried_before_it_can_run() {
        // Some gateways report a normal stop even when the billed output count
        // proves the response reached the configured ceiling.
        let (addr, server) = sse_server(vec![
            output_limited_tool_call("stop"),
            completed_tool_call(),
        ])
        .await;
        let client = LlmClient::new(sse_test_config(addr, 8192));

        let mut retries = Vec::new();
        let turn = {
            let mut hooks = StreamHooks {
                text: &mut |_| {},
                thinking: &mut |_| {},
                retry: &mut |retry: Retry| retries.push(retry),
            };
            client
                .stream_chat(&[Message::user("create it")], &[], &mut hooks)
                .await
                .unwrap()
        };

        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(
            turn.tool_calls[0].function.arguments,
            r#"{"title":"Report","body":"complete"}"#
        );
        assert_eq!(retries.len(), 1);
        let requests = server.await.unwrap();
        let budgets: Vec<u64> = requests.iter().map(|r| request_budget(r)).collect();
        assert_eq!(budgets, vec![8192, 16384]);
    }

    #[tokio::test]
    async fn an_output_limited_tool_call_never_escapes_at_the_budget_ceiling() {
        let partial = output_limited_tool_call("length");
        let (addr, server) = sse_server(vec![partial.clone(), partial.clone(), partial]).await;
        let client = LlmClient::new(sse_test_config(addr, 8192));

        let err = {
            let mut hooks = StreamHooks {
                text: &mut |_| {},
                thinking: &mut |_| {},
                retry: &mut |_| {},
            };
            client
                .stream_chat(&[Message::user("create it")], &[], &mut hooks)
                .await
                .unwrap_err()
        };

        let message = err.to_string();
        assert!(
            message.contains("before completing its response"),
            "{message}"
        );
        assert!(message.contains("max_tokens = 32768"), "{message}");
        let requests = server.await.unwrap();
        let budgets: Vec<u64> = requests.iter().map(|r| request_budget(r)).collect();
        assert_eq!(budgets, vec![8192, 16384, 32768]);
    }

    #[tokio::test]
    async fn a_portkey_gpt5_turn_escalates_like_any_openai_compatible_model() {
        let (addr, server) = sse_server(vec![length_limited_turn(), completed_turn("done")]).await;
        let client = LlmClient::new(portkey_test_config(addr, 8192));

        let mut text = String::new();
        let turn = {
            let mut hooks = StreamHooks {
                text: &mut |delta: String| text.push_str(&delta),
                thinking: &mut |_| {},
                retry: &mut |_| {},
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap()
        };

        assert_eq!(turn.content, "done");
        let requests = server.await.unwrap();
        let budgets: Vec<u64> = requests.iter().map(|r| request_budget(r)).collect();
        assert_eq!(budgets, vec![8192, 16384]);
    }

    #[tokio::test]
    async fn a_reasoning_only_empty_turn_escalates_without_a_reported_limit() {
        let (addr, server) =
            sse_server(vec![reasoning_only_turn("stop"), completed_turn("done")]).await;
        let client = LlmClient::new(portkey_test_config(addr, 8192));

        let mut text = String::new();
        let turn = {
            let mut hooks = StreamHooks {
                text: &mut |delta: String| text.push_str(&delta),
                thinking: &mut |_| {},
                retry: &mut |_| {},
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap()
        };

        assert_eq!(turn.content, "done");
        let requests = server.await.unwrap();
        let budgets: Vec<u64> = requests.iter().map(|r| request_budget(r)).collect();
        assert_eq!(budgets, vec![8192, 16384]);
    }

    #[tokio::test]
    async fn a_billed_reasoning_turn_escalates_even_without_streamed_text() {
        let (addr, server) =
            sse_server(vec![billed_reasoning_turn(), completed_turn("done")]).await;
        let client = LlmClient::new(portkey_test_config(addr, 8192));

        let mut text = String::new();
        let turn = {
            let mut hooks = StreamHooks {
                text: &mut |delta: String| text.push_str(&delta),
                thinking: &mut |_| {},
                retry: &mut |_| {},
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap()
        };

        assert_eq!(turn.content, "done");
        let requests = server.await.unwrap();
        let budgets: Vec<u64> = requests.iter().map(|r| request_budget(r)).collect();
        assert_eq!(budgets, vec![8192, 16384]);
    }

    #[tokio::test]
    async fn an_empty_turn_without_an_output_limit_retries_the_same_budget() {
        let empty =
            sse(&[serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]})]);
        let (addr, server) = sse_server(vec![empty, completed_turn("recovered")]).await;
        let client = LlmClient::new(sse_test_config(addr, 8192));

        let mut text = String::new();
        let turn = {
            let mut hooks = StreamHooks {
                text: &mut |delta: String| text.push_str(&delta),
                thinking: &mut |_| {},
                retry: &mut |_| {},
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap()
        };

        assert_eq!(turn.content, "recovered");
        let requests = server.await.unwrap();
        let budgets: Vec<u64> = requests.iter().map(|r| request_budget(r)).collect();
        assert_eq!(budgets, vec![8192, 8192]);
    }

    #[tokio::test]
    async fn budget_exhaustion_reports_the_limit_instead_of_an_empty_response() {
        let truncated = length_limited_turn();
        let (addr, server) =
            sse_server(vec![truncated.clone(), truncated.clone(), truncated]).await;
        let client = LlmClient::new(sse_test_config(addr, 8192));

        let err = {
            let mut hooks = StreamHooks {
                text: &mut |_| {},
                thinking: &mut |_| {},
                retry: &mut |_| {},
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap_err()
        };

        let message = err.to_string();
        assert!(message.contains("output budget"), "{message}");
        assert!(message.contains("max_tokens = 32768"), "{message}");
        assert!(!message.contains("empty response"), "{message}");
        // Both ways of returning no answer are recognizable, so a caller that
        // sent a best-effort request can finish with what it already has.
        assert!(err.downcast_ref::<NoAnswer>().is_some());
        let requests = server.await.unwrap();
        let budgets: Vec<u64> = requests.iter().map(|r| request_budget(r)).collect();
        assert_eq!(budgets, vec![8192, 16384, 32768]);
    }

    #[tokio::test]
    async fn an_exhausted_empty_turn_is_recognizable_as_a_no_answer() {
        let empty =
            sse(&[serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]})]);
        let (addr, server) = sse_server(vec![empty.clone(), empty.clone(), empty]).await;
        let client = LlmClient::new(sse_test_config(addr, 8192));

        let err = {
            let mut hooks = StreamHooks {
                text: &mut |_| {},
                thinking: &mut |_| {},
                retry: &mut |_| {},
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap_err()
        };

        assert_eq!(err.to_string(), "the model returned an empty response");
        assert!(err.downcast_ref::<NoAnswer>().is_some());
        // The retry budget is spent before giving up.
        let seen = server.await.unwrap();
        let budgets: Vec<u64> = seen.iter().map(|r| request_budget(r)).collect();
        assert_eq!(budgets, vec![8192, 8192, 8192]);
    }

    #[tokio::test]
    async fn an_anthropic_max_token_turn_escalates_like_openai() {
        let truncated = anthropic_sse(&[
            serde_json::json!({"type": "message_start", "message": {"usage": {"input_tokens": 10, "output_tokens": 1}}}),
            serde_json::json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
            serde_json::json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "thinking"}}),
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "max_tokens"}, "usage": {"output_tokens": 8192}}),
            serde_json::json!({"type": "message_stop"}),
        ]);
        let answer = anthropic_sse(&[
            serde_json::json!({"type": "message_start", "message": {"usage": {"input_tokens": 10, "output_tokens": 1}}}),
            serde_json::json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            serde_json::json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "done"}}),
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 2}}),
            serde_json::json!({"type": "message_stop"}),
        ]);
        let (addr, server) = sse_server(vec![truncated, answer]).await;
        let client = LlmClient::new(anthropic_test_config(addr, 8192));

        let mut text = String::new();
        let turn = {
            let mut hooks = StreamHooks {
                text: &mut |delta: String| text.push_str(&delta),
                thinking: &mut |_| {},
                retry: &mut |_| {},
            };
            client
                .stream_chat(&[Message::user("hi")], &[], &mut hooks)
                .await
                .unwrap()
        };

        assert_eq!(turn.content, "done");
        assert_eq!(text, "done");
        let requests = server.await.unwrap();
        let budgets: Vec<u64> = requests.iter().map(|r| request_budget(r)).collect();
        assert_eq!(budgets, vec![8192, 16384]);
    }
}
