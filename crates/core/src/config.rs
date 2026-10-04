use crate::auth::AuthStore;
use crate::ecosystem::{self, AgentDef, Ecosystem};
use crate::memory::{MemoryStore, Scope};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are Oxide, an AI coding agent running in the user's terminal. \
You help with software engineering tasks — writing, editing, debugging and explaining code — \
and with the work around them: research, automation, and answering questions. \
Use the provided tools to inspect and modify the user's project. \
You can see images, PDFs and text files attached to user messages; `read` returns images and \
PDFs as viewable attachments, and any other file as its own text. \
Prefer small, focused changes and verify your work. \
Be concise while you work, but when you are done give the user a detailed final summary: what \
changed and why, the files or areas you touched, and how you verified it. Make it detailed enough \
to review without re-reading the conversation, but do not pad it or restate unchanged code.";

const PORTKEY_FALLBACK_MODELS: &[&str] = &[
    "claude-haiku-4-5",
    "claude-opus-4-8",
    "claude-opus-5",
    "claude-sonnet-4-6",
    "claude-sonnet-5",
    "glm-5.2",
    "gpt-5.4",
    "gpt-5.6-luna",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
];

/// The GLM models offered in the picker for Z.AI, whose API has no documented
/// model listing endpoint.
const GLM_FALLBACK_MODELS: &[&str] = &[
    "glm-4.5-air",
    "glm-4.6",
    "glm-4.7",
    "glm-4.7-flash",
    "glm-5.1",
    "glm-5.2",
    "glm-5.3",
    "glm-5.3-flash",
];

pub fn model_label(model: &str) -> &str {
    match model {
        "claude-haiku-4-5" => "Claude Haiku 4.5",
        "claude-opus-4-8" => "Claude Opus 4.8",
        "claude-opus-5" => "Claude Opus 5",
        "claude-sonnet-4-6" => "Claude Sonnet 4.6",
        "claude-sonnet-5" => "Claude Sonnet 5",
        "deepseek-flash" => "DeepSeek V4.1 Flash",
        "deepseek-v4-pro" => "DeepSeek V4 Pro",
        "glm-4.5-air" => "GLM-4.5 Air",
        "glm-4.6" => "GLM-4.6",
        "glm-4.7" => "GLM-4.7",
        "glm-4.7-flash" => "GLM-4.7 Flash",
        "glm-5.1" => "GLM-5.1",
        "glm-5.2" => "GLM-5.2",
        "glm-5.3" => "GLM-5.3",
        "glm-5.3-flash" => "GLM-5.3 Flash",
        "glm-5.3-flashx" => "GLM-5.3 FlashX",
        "gpt-5.4" => "GPT-5.4",
        "gpt-5.6-luna" => "GPT-5.6 Luna",
        "gpt-5.6-sol" => "GPT-5.6 Sol",
        "gpt-5.6-terra" => "GPT-5.6 Terra",
        _ => model,
    }
}

/// Every logged-in provider as its own `Config`, starting with the active one.
/// Used by `/models` (CLI) and the desktop model picker to query each
/// provider's catalog.
pub fn provider_configs(config: &Config) -> Vec<(String, Config)> {
    let active = canonical_provider(&config.provider);
    let mut providers: Vec<(String, Config)> = Vec::new();
    if !config.api_key.trim().is_empty() {
        providers.push((active.clone(), config.clone()));
    }
    let store = crate::auth::AuthStore::load().unwrap_or_default();
    for name in store.providers() {
        if providers.iter().any(|(known, _)| known == &name) {
            continue;
        }
        let is_preset = ProviderPreset::for_name(&name).is_some();
        if !is_preset && !config.provider_base_urls.contains_key(&name) {
            continue;
        }
        let key = store.key(&name).unwrap_or_default().to_string();
        providers.push((name.clone(), config.for_provider(&name, &key)));
    }
    providers
}

/// The default `api-version` Azure OpenAI deployments are called with, used
/// when none is configured.
const AZURE_API_VERSION: &str = "2024-10-21";

/// The API host GitHub Copilot serves a subscription from, used when the
/// session exchange does not name an account's own endpoint.
pub const COPILOT_API: &str = "https://api.githubcopilot.com";

/// Which request URL is being built for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Chat,
    Models,
}

/// The API dialect a provider speaks. Providers that share a wire format share
/// one client implementation, so where a provider sits in [`PROVIDERS`] decides
/// which client serves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// OpenAI's chat-completions wire. Azure OpenAI and GitHub Copilot speak it
    /// too, with their own host and authentication.
    OpenAi,
    /// Anthropic's Messages API.
    Anthropic,
    /// Google's `generateContent` API, as served by the Gemini API and by
    /// Vertex AI.
    Gemini,
    /// AWS Bedrock's Converse API.
    Bedrock,
}

/// How a provider authenticates, and which host serves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStyle {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// `x-api-key: <key>`, for Anthropic's Messages API.
    XApiKey,
    /// Portkey's `x-portkey-api-key` header plus an optional Config ID.
    Portkey,
    /// `x-goog-api-key: <key>`, for the Gemini API.
    Google,
    /// Azure OpenAI's `api-key` header and `?api-version=` query parameter.
    Azure,
    /// An OAuth access token for Vertex AI.
    Vertex,
    /// AWS SigV4, or a Bedrock bearer token when one is configured.
    Aws,
    /// The short-lived Copilot token minted from a stored GitHub token.
    Copilot,
    /// A GitLab Duo gateway token minted from a stored instance token, which is
    /// what the AI gateway's proxy accepts.
    Gitlab,
}

/// One provider: its identity, its aliases, its wire dialect, and the defaults
/// a login fills in. This table is the single place a provider is declared —
/// the login picker, the aliases, the environment variables and the client
/// dispatch all read from it.
#[derive(Debug, Clone, Copy)]
pub struct ProviderPreset {
    /// The canonical name, and the key its credential is stored under.
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Where the reader gets a key.
    pub key_url: &'static str,
    /// Other spellings that resolve to this provider.
    pub aliases: &'static [&'static str],
    pub kind: ProviderKind,
    pub auth: AuthStyle,
    pub base_url: &'static str,
    pub base_url_env: &'static str,
    pub model: &'static str,
    pub key_env: &'static str,
    /// Further variables consulted for the credential, in order, after
    /// `key_env`.
    pub extra_env: &'static [&'static str],
    /// The models offered when the provider exposes no usable listing.
    pub models: &'static [&'static str],
    /// Whether the provider may run without a key (a server on this machine).
    pub local: bool,
}

impl ProviderPreset {
    pub fn for_name(name: &str) -> Option<Self> {
        let name = canonical_provider(name);
        PROVIDERS.iter().copied().find(|preset| preset.name == name)
    }

    /// The preset whose host serves `base_url`, so a provider configured under
    /// a name of its own (or a gateway pointed at a first-party API) is still
    /// read for what it speaks. Compared by host, so a path or a version
    /// segment does not decide it.
    pub fn for_base_url(base_url: &str) -> Option<Self> {
        let host = url_host(base_url)?;
        PROVIDERS
            .iter()
            .copied()
            .find(|preset| url_host(preset.base_url).as_deref() == Some(host.as_str()))
    }

    /// Every environment variable that may carry this provider's credential.
    pub fn key_envs(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.key_env).chain(self.extra_env.iter().copied())
    }
}

/// The host of a URL, lowered, without the scheme, a port or any path.
fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = rest.split(['/', '?']).next()?.trim();
    let host = host.rsplit('@').next()?.split(':').next()?.trim();
    if host.is_empty() || !host.contains('.') {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Every provider Oxide knows by name, in the order the login picker shows
/// them: the APIs most readers reach for first, then the other hosted
/// services, then the local servers and the ones that need a cloud account of
/// their own. A provider that is not listed still works: naming it with
/// `provider`, `base_url`, `model` and a key speaks the OpenAI-compatible wire
/// by default.
pub const PROVIDERS: &[ProviderPreset] = &[
    ProviderPreset {
        name: "openai",
        label: "OpenAI",
        description: "GPT models",
        key_url: "https://platform.openai.com/api-keys",
        aliases: &["gpt", "gpt-4", "gpt-4o"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.openai.com/v1",
        base_url_env: "OPENAI_BASE_URL",
        model: "gpt-4o-mini",
        key_env: "OPENAI_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "anthropic",
        label: "Anthropic",
        description: "Claude models",
        key_url: "https://console.anthropic.com/settings/keys",
        aliases: &["claude"],
        kind: ProviderKind::Anthropic,
        auth: AuthStyle::XApiKey,
        base_url: "https://api.anthropic.com/v1",
        base_url_env: "ANTHROPIC_BASE_URL",
        model: "claude-3-5-sonnet-latest",
        key_env: "ANTHROPIC_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "deepseek",
        label: "DeepSeek",
        description: "DeepSeek chat and reasoning models",
        key_url: "https://platform.deepseek.com/api_keys",
        aliases: &[],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.deepseek.com/v1",
        base_url_env: "DEEPSEEK_BASE_URL",
        model: "deepseek-chat",
        key_env: "DEEPSEEK_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "google",
        label: "Google",
        description: "Gemini models",
        key_url: "https://aistudio.google.com/apikey",
        aliases: &["gemini", "google-ai", "googleai"],
        kind: ProviderKind::Gemini,
        auth: AuthStyle::Google,
        base_url: "https://generativelanguage.googleapis.com/v1beta",
        base_url_env: "GEMINI_BASE_URL",
        model: "gemini-2.0-flash",
        key_env: "GEMINI_API_KEY",
        extra_env: &["GOOGLE_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY"],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "portkey",
        label: "Portkey",
        description: "AI gateway and model routing",
        key_url: "https://app.portkey.ai/api-keys",
        aliases: &["port-key"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Portkey,
        base_url: "https://api.portkey.ai/v1",
        base_url_env: "PORTKEY_BASE_URL",
        model: "claude-sonnet-5",
        key_env: "PORTKEY_API_KEY",
        extra_env: &[],
        models: PORTKEY_FALLBACK_MODELS,
        local: false,
    },
    ProviderPreset {
        name: "zai",
        label: "Z.AI",
        description: "GLM models",
        key_url: "https://z.ai/manage-apikey/apikey-list",
        aliases: &["glm", "z.ai", "z-ai", "zhipu", "bigmodel"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.z.ai/api/paas/v4",
        base_url_env: "ZAI_BASE_URL",
        model: "glm-5.3",
        key_env: "ZAI_API_KEY",
        extra_env: &["ZHIPU_API_KEY", "GLM_API_KEY"],
        models: GLM_FALLBACK_MODELS,
        local: false,
    },
    ProviderPreset {
        name: "xai",
        label: "xAI",
        description: "Grok models",
        key_url: "https://console.x.ai",
        aliases: &["grok", "x-ai"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.x.ai/v1",
        base_url_env: "XAI_BASE_URL",
        model: "grok-3",
        key_env: "XAI_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "mistral",
        label: "Mistral",
        description: "Mistral and Codestral models",
        key_url: "https://console.mistral.ai/api-keys",
        aliases: &[],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.mistral.ai/v1",
        base_url_env: "MISTRAL_BASE_URL",
        model: "mistral-large-latest",
        key_env: "MISTRAL_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "openrouter",
        label: "OpenRouter",
        description: "One API for many models",
        key_url: "https://openrouter.ai/settings/keys",
        aliases: &[],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://openrouter.ai/api/v1",
        base_url_env: "OPENROUTER_BASE_URL",
        model: "anthropic/claude-3.5-sonnet",
        key_env: "OPENROUTER_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "groq",
        label: "Groq",
        description: "Fast open-model inference",
        key_url: "https://console.groq.com/keys",
        aliases: &[],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.groq.com/openai/v1",
        base_url_env: "GROQ_BASE_URL",
        model: "llama-3.3-70b-versatile",
        key_env: "GROQ_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "cerebras",
        label: "Cerebras",
        description: "Wafer-scale inference",
        key_url: "https://cloud.cerebras.ai",
        aliases: &[],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.cerebras.ai/v1",
        base_url_env: "CEREBRAS_BASE_URL",
        model: "llama-3.3-70b",
        key_env: "CEREBRAS_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "together",
        label: "Together AI",
        description: "Open models",
        key_url: "https://api.together.ai/settings/api-keys",
        aliases: &["togetherai"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.together.xyz/v1",
        base_url_env: "TOGETHER_BASE_URL",
        model: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        key_env: "TOGETHER_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "fireworks",
        label: "Fireworks AI",
        description: "Open models",
        key_url: "https://fireworks.ai/account/api-keys",
        aliases: &["fireworks-ai"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.fireworks.ai/inference/v1",
        base_url_env: "FIREWORKS_BASE_URL",
        model: "accounts/fireworks/models/llama-v3p3-70b-instruct",
        key_env: "FIREWORKS_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "deepinfra",
        label: "DeepInfra",
        description: "Open models",
        key_url: "https://deepinfra.com/dash/api_keys",
        aliases: &[],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.deepinfra.com/v1/openai",
        base_url_env: "DEEPINFRA_BASE_URL",
        model: "meta-llama/Llama-3.3-70B-Instruct",
        key_env: "DEEPINFRA_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "nebius",
        label: "Nebius",
        description: "Token Factory inference",
        key_url: "https://studio.nebius.com",
        aliases: &[],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.studio.nebius.com/v1",
        base_url_env: "NEBIUS_BASE_URL",
        model: "meta-llama/Llama-3.3-70B-Instruct",
        key_env: "NEBIUS_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "baseten",
        label: "Baseten",
        description: "Model serving",
        key_url: "https://app.baseten.co/settings/api_keys",
        aliases: &[],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://inference.baseten.co/v1",
        base_url_env: "BASETEN_BASE_URL",
        model: "meta-llama/Llama-3.3-70B-Instruct",
        key_env: "BASETEN_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "siliconflow",
        label: "SiliconFlow",
        description: "Open models",
        key_url: "https://cloud.siliconflow.com/account/ak",
        aliases: &[],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.siliconflow.com/v1",
        base_url_env: "SILICONFLOW_BASE_URL",
        model: "Qwen/Qwen2.5-72B-Instruct",
        key_env: "SILICONFLOW_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "novita",
        label: "NovitaAI",
        description: "Open models",
        key_url: "https://novita.ai/settings/key-management",
        aliases: &["novita-ai"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.novita.ai/openai",
        base_url_env: "NOVITA_BASE_URL",
        model: "meta-llama/llama-3.3-70b-instruct",
        key_env: "NOVITA_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "nvidia",
        label: "NVIDIA",
        description: "NIM inference",
        key_url: "https://build.nvidia.com",
        aliases: &["nim"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://integrate.api.nvidia.com/v1",
        base_url_env: "NVIDIA_BASE_URL",
        model: "meta/llama-3.3-70b-instruct",
        key_env: "NVIDIA_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "upstage",
        label: "Upstage",
        description: "Solar models",
        key_url: "https://console.upstage.ai",
        aliases: &[],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.upstage.ai/v1/solar",
        base_url_env: "UPSTAGE_BASE_URL",
        model: "solar-pro",
        key_env: "UPSTAGE_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "moonshot",
        label: "Moonshot",
        description: "Kimi models",
        key_url: "https://platform.moonshot.ai/console/api-keys",
        aliases: &["kimi", "moonshot-ai"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.moonshot.ai/v1",
        base_url_env: "MOONSHOT_BASE_URL",
        model: "kimi-k2-0711-preview",
        key_env: "MOONSHOT_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "alibaba",
        label: "Alibaba",
        description: "Qwen models on DashScope",
        key_url: "https://bailian.console.alibabacloud.com",
        aliases: &["qwen", "dashscope"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        base_url_env: "DASHSCOPE_BASE_URL",
        model: "qwen-max",
        key_env: "DASHSCOPE_API_KEY",
        extra_env: &["QWEN_API_KEY"],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "minimax",
        label: "MiniMax",
        description: "MiniMax models",
        key_url: "https://platform.minimax.io/user-center/basic-information/interface-key",
        aliases: &[],
        kind: ProviderKind::Anthropic,
        auth: AuthStyle::XApiKey,
        base_url: "https://api.minimax.io/anthropic/v1",
        base_url_env: "MINIMAX_BASE_URL",
        model: "MiniMax-M2",
        key_env: "MINIMAX_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "perplexity",
        label: "Perplexity",
        description: "Sonar models with search",
        key_url: "https://www.perplexity.ai/settings/api",
        aliases: &["pplx"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.perplexity.ai",
        base_url_env: "PERPLEXITY_BASE_URL",
        model: "sonar",
        key_env: "PERPLEXITY_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "cohere",
        label: "Cohere",
        description: "Command models",
        key_url: "https://dashboard.cohere.com/api-keys",
        aliases: &[],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://api.cohere.ai/compatibility/v1",
        base_url_env: "COHERE_BASE_URL",
        model: "command-r-plus",
        key_env: "COHERE_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "vercel",
        label: "Vercel AI Gateway",
        description: "Routing across providers",
        key_url: "https://vercel.com/dashboard/ai-gateway",
        aliases: &["ai-gateway", "gateway"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://ai-gateway.vercel.sh/v1",
        base_url_env: "AI_GATEWAY_BASE_URL",
        model: "anthropic/claude-3.5-sonnet",
        key_env: "AI_GATEWAY_API_KEY",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "huggingface",
        label: "Hugging Face",
        description: "Inference router",
        key_url: "https://huggingface.co/settings/tokens",
        aliases: &["hf"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "https://router.huggingface.co/v1",
        base_url_env: "HF_BASE_URL",
        model: "meta-llama/Llama-3.3-70B-Instruct",
        key_env: "HF_TOKEN",
        extra_env: &["HUGGINGFACE_API_KEY"],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "ollama",
        label: "Ollama",
        description: "Local models via Ollama",
        key_url: "https://ollama.com/download",
        aliases: &[],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "http://localhost:11434/v1",
        base_url_env: "OLLAMA_BASE_URL",
        model: "llama3.3",
        key_env: "OLLAMA_API_KEY",
        extra_env: &[],
        models: &[],
        local: true,
    },
    ProviderPreset {
        name: "lmstudio",
        label: "LM Studio",
        description: "Local models via LM Studio",
        key_url: "https://lmstudio.ai",
        aliases: &["lm-studio"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "http://localhost:1234/v1",
        base_url_env: "LMSTUDIO_BASE_URL",
        model: "local-model",
        key_env: "LMSTUDIO_API_KEY",
        extra_env: &[],
        models: &[],
        local: true,
    },
    ProviderPreset {
        name: "llamacpp",
        label: "llama.cpp",
        description: "Local models via llama.cpp",
        key_url: "https://github.com/ggml-org/llama.cpp",
        aliases: &["llama-cpp", "llama.cpp"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Bearer,
        base_url: "http://localhost:8080/v1",
        base_url_env: "LLAMACPP_BASE_URL",
        model: "local-model",
        key_env: "LLAMACPP_API_KEY",
        extra_env: &[],
        models: &[],
        local: true,
    },
    ProviderPreset {
        name: "vertex",
        label: "Vertex AI",
        description: "Gemini on Google Cloud",
        key_url: "https://cloud.google.com/vertex-ai/docs/authentication",
        aliases: &["google-vertex", "vertex-ai"],
        kind: ProviderKind::Gemini,
        auth: AuthStyle::Vertex,
        base_url: "",
        base_url_env: "VERTEX_BASE_URL",
        model: "gemini-2.0-flash",
        key_env: "GOOGLE_VERTEX_CREDENTIALS",
        extra_env: &["GOOGLE_APPLICATION_CREDENTIALS"],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "bedrock",
        label: "Amazon Bedrock",
        description: "Claude and open models on AWS",
        key_url: "https://docs.aws.amazon.com/bedrock/latest/userguide/security-iam.html",
        aliases: &["amazon-bedrock", "aws-bedrock", "aws"],
        kind: ProviderKind::Bedrock,
        auth: AuthStyle::Aws,
        base_url: "",
        base_url_env: "BEDROCK_BASE_URL",
        model: "anthropic.claude-3-5-sonnet-20241022-v2:0",
        key_env: "AWS_BEARER_TOKEN_BEDROCK",
        extra_env: &[],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "azure",
        label: "Azure OpenAI",
        description: "OpenAI models on Azure",
        key_url: "https://portal.azure.com",
        aliases: &["azure-openai", "azure-ai"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Azure,
        base_url: "",
        base_url_env: "AZURE_OPENAI_BASE_URL",
        model: "",
        key_env: "AZURE_API_KEY",
        extra_env: &["AZURE_OPENAI_API_KEY"],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "github-copilot",
        label: "GitHub Copilot",
        description: "Copilot models on a GitHub subscription (paste a GitHub token)",
        key_url: "https://github.com/settings/tokens",
        aliases: &["copilot", "github"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Copilot,
        base_url: COPILOT_API,
        base_url_env: "COPILOT_BASE_URL",
        model: "gpt-4o",
        key_env: "GITHUB_TOKEN",
        extra_env: &["GH_TOKEN"],
        models: &[],
        local: false,
    },
    ProviderPreset {
        name: "gitlab",
        label: "GitLab Duo",
        description: "Duo models through the AI gateway (paste a personal access token)",
        key_url: "https://gitlab.com/-/user_settings/personal_access_tokens",
        aliases: &["gitlab-duo", "duo"],
        kind: ProviderKind::OpenAi,
        auth: AuthStyle::Gitlab,
        base_url: "",
        base_url_env: "GITLAB_AI_GATEWAY_URL",
        model: "gpt-4o",
        key_env: "GITLAB_TOKEN",
        extra_env: &["GL_TOKEN"],
        models: &[],
        local: false,
    },
];

/// Resolves a provider name or alias to the canonical name its credential is
/// stored under. An unknown name is returned lowercased and trimmed, so a
/// custom provider keeps the spelling it was configured with.
pub fn canonical_provider(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    for preset in PROVIDERS {
        if preset.name == name || preset.aliases.contains(&name.as_str()) {
            return preset.name.to_string();
        }
    }
    name
}

/// How much reasoning effort to ask the model for. `Auto` (the default) leaves
/// the effort to the provider or uses its native adaptive mode; explicit levels
/// are translated by the active provider client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Reasoning {
    #[default]
    Auto,
    Off,
    Low,
    Medium,
    High,
}

impl Reasoning {
    /// Parses a user-supplied level, accepting common aliases.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "" | "auto" | "default" => Some(Reasoning::Auto),
            "off" | "none" | "disabled" => Some(Reasoning::Off),
            "low" | "minimal" | "small" => Some(Reasoning::Low),
            "medium" | "med" => Some(Reasoning::Medium),
            "high" | "xhigh" | "x-high" => Some(Reasoning::High),
            _ => None,
        }
    }

    /// A short lowercase label used in the UI and CLI.
    pub fn label(self) -> &'static str {
        match self {
            Reasoning::Auto => "auto",
            Reasoning::Off => "off",
            Reasoning::Low => "low",
            Reasoning::Medium => "medium",
            Reasoning::High => "high",
        }
    }

    /// The cycle used by the TUI (auto → off → low → medium → high).
    pub fn next(self) -> Self {
        match self {
            Reasoning::Auto => Reasoning::Off,
            Reasoning::Off => Reasoning::Low,
            Reasoning::Low => Reasoning::Medium,
            Reasoning::Medium => Reasoning::High,
            Reasoning::High => Reasoning::Auto,
        }
    }

    /// The OpenAI-compatible `reasoning_effort` value, if any.
    pub fn effort(self) -> Option<&'static str> {
        match self {
            Reasoning::Low => Some("low"),
            Reasoning::Medium => Some("medium"),
            Reasoning::High => Some("high"),
            Reasoning::Auto | Reasoning::Off => None,
        }
    }

    /// The Anthropic extended-thinking budget in tokens, if enabled.
    pub fn budget_tokens(self, max_tokens: u32) -> Option<u32> {
        let desired = match self {
            Reasoning::Low => 2048,
            Reasoning::Medium => 6144,
            Reasoning::High => 12_288,
            Reasoning::Auto | Reasoning::Off => return None,
        };
        let budget = desired.min(max_tokens.saturating_sub(1024));
        (budget >= 1024).then_some(budget)
    }
}

/// Newer Claude generations use adaptive thinking instead of fixed token
/// budgets. Provider-prefixed and Bedrock model ids are accepted.
pub fn supports_adaptive_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    let Some((_, suffix)) = model.split_once("claude-") else {
        return false;
    };
    let parts: Vec<&str> = suffix.split(['-', '.', '_']).collect();
    let Some(index) = parts
        .iter()
        .position(|part| part.len() <= 2 && part.chars().all(|ch| ch.is_ascii_digit()))
    else {
        return false;
    };
    let Ok(major) = parts[index].parse::<u32>() else {
        return false;
    };
    let minor = parts
        .get(index + 1)
        .filter(|part| part.len() <= 2 && part.chars().all(|ch| ch.is_ascii_digit()))
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    major > 4 || (major == 4 && minor >= 6)
}

/// Whether a GLM model always thinks. The GLM-5.3 series rejects
/// `thinking.type = "disabled"`, so the lowest effort is the closest oxide can
/// get to turning reasoning off there.
pub fn glm_forces_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    let Some(version) = model.strip_prefix("glm-") else {
        return false;
    };
    let version = version.split(['-', '/']).next().unwrap_or_default();
    let mut parts = version.split('.');
    let major = parts
        .next()
        .filter(|part| part.len() <= 2 && part.chars().all(|ch| ch.is_ascii_digit()))
        .and_then(|part| part.parse::<u32>().ok());
    let minor = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    matches!(major, Some(major) if major > 5 || (major == 5 && minor >= 3))
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn explicit(raw: &Option<serde_json::Value>, key: &str) -> bool {
    raw.as_ref().is_some_and(|value| value.get(key).is_some())
}

/// The provider and entry from `auth.json` when exactly one credential is
/// stored, so an unconfigured oxide can pick it without guessing.
fn single_stored_provider(store: &AuthStore) -> Option<(&String, &crate::auth::AuthEntry)> {
    if store.entries.len() == 1 {
        store.entries.iter().next()
    } else {
        None
    }
}

/// Adopts the sole stored credential when the config has no key yet, including
/// the provider's preset model and base URL unless the config set them.
fn apply_stored_provider_fallback(
    config: &mut Config,
    store: &AuthStore,
    raw: &Option<serde_json::Value>,
) {
    if !config.api_key.trim().is_empty() {
        return;
    }
    let Some((name, entry)) = single_stored_provider(store) else {
        return;
    };
    config.provider = name.clone();
    config.api_key = entry.key.clone();
    if ProviderPreset::for_name(name).is_some() && !explicit(raw, "model") {
        // A model remembered for this provider wins over its preset.
        config.model = config.model_for_provider(name);
    }
    if !explicit(raw, "base_url") {
        if let Some(url) = config.remembered_base_url(name) {
            config.base_url = url;
        } else if let Some(preset) = ProviderPreset::for_name(name) {
            config.base_url = preset.base_url.to_string();
        }
    }
}

const CONFIG_DIR_NAME: &str = "Oxide";

/// The Oxide configuration directory under the platform config dir
/// (`<platform config>/Oxide`). The pre-branding `<platform config>/oxide`
/// directory is migrated once on first access when the new one is absent.
pub fn config_dir() -> Option<PathBuf> {
    let base = dirs::config_dir()?;
    let dir = base.join(CONFIG_DIR_NAME);
    migrate_legacy_config_dir(&base, &dir);
    Some(dir)
}

/// Like [`config_dir`], falling back to a relative `Oxide` path when the
/// platform config directory cannot be resolved.
pub fn config_dir_or_default() -> PathBuf {
    config_dir().unwrap_or_else(|| PathBuf::from(CONFIG_DIR_NAME))
}

fn migrate_legacy_config_dir(base: &Path, dir: &Path) {
    if has_dir_entry(base, CONFIG_DIR_NAME) {
        return;
    }
    if !has_dir_entry(base, "oxide") {
        return;
    }
    // Rename through a temporary name: on case-insensitive filesystems
    // (macOS, Windows) `oxide` and `Oxide` are the same directory, so a
    // direct rename would not change the on-disk case.
    let staging = base.join(".Oxide-migrate");
    let _ = std::fs::remove_dir_all(&staging);
    if std::fs::rename(base.join("oxide"), &staging).is_ok() {
        let _ = std::fs::rename(&staging, dir);
    }
}

/// Whether `base` contains an entry whose name matches `name` exactly
/// (case-sensitively), even on a case-insensitive filesystem.
fn has_dir_entry(base: &Path, name: &str) -> bool {
    std::fs::read_dir(base)
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry.file_name() == std::ffi::OsStr::new(name)
                    && entry.file_type().map(|kind| kind.is_dir()).unwrap_or(true)
            })
        })
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub portkey_config: String,
    /// The model last used with each provider, so switching between logged-in
    /// providers restores that provider's model instead of leaving the other
    /// provider's model id behind.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_models: BTreeMap<String, String>,
    /// A custom endpoint last used with each provider, so switching between
    /// providers keeps each one's gateway instead of leaking the active
    /// `base_url` into the others.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_base_urls: BTreeMap<String, String>,
    #[serde(default)]
    pub model_catalog: Vec<String>,
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// An explicit override for the model's full input context window, separate
    /// from `max_tokens`, which caps only one response. `0` (the default)
    /// derives the window from the model and falls back to
    /// [`default_context_window`].
    #[serde(default)]
    pub context_window: u64,
    #[serde(default = "default_true")]
    pub auto_approve: bool,
    #[serde(default)]
    pub reasoning: Reasoning,
    #[serde(skip)]
    pub ecosystem: Ecosystem,
    #[serde(skip)]
    pub active_agent: Option<AgentDef>,
    #[serde(skip)]
    pub memory: MemoryStore,
    #[serde(skip)]
    pub compaction: crate::compact::CompactionConfig,
    #[serde(skip)]
    pub prices: BTreeMap<String, crate::pricing::ModelPrice>,
    /// Whether a finished agent turn raises a desktop toast, and whether it
    /// plays the system alert sound.
    #[serde(skip)]
    pub notify: crate::notify::NotifyConfig,
    #[serde(skip)]
    pub tool_filter: crate::cli::ToolFilter,
    #[serde(skip)]
    pub ephemeral: bool,
    #[serde(skip)]
    pub load_context_files: bool,
    #[serde(skip)]
    pub default_project_trust: crate::trust::DefaultTrust,
    #[serde(skip)]
    pub trusted: bool,
    /// The local projects this run can reach, named in the system prompt so a
    /// repository elsewhere on the machine is found from the list instead of a
    /// scan of the home directory.
    #[serde(skip)]
    pub workspaces: crate::workspaces::Workspaces,
}

fn default_provider() -> String {
    "openai".to_string()
}

fn default_system_prompt() -> String {
    DEFAULT_SYSTEM_PROMPT.to_string()
}

fn default_max_tokens() -> u32 {
    8192
}

/// The window used when the model is not in the built-in table. Kept large
/// because a large window is the common case for the providers Oxide targets;
/// a model with a smaller documented window is capped by
/// [`builtin_context_window`], so a fresh configuration does not defer
/// compaction past the provider's real limit.
fn default_context_window() -> u64 {
    1_000_000
}

/// The input context window of a known model, matched by its longest prefix so
/// `gpt-4o` and `gpt-4.1` do not fall under `gpt-4`. `None` for a model not in
/// the table, which keeps [`default_context_window`]. The values are the
/// providers' documented windows; a gateway that exposes a different one is
/// answered by setting `context_window` explicitly.
fn builtin_context_window(model: &str) -> Option<u64> {
    const WINDOWS: &[(&str, u64)] = &[
        ("gpt-3.5", 16_385),
        ("gpt-4.1", 1_000_000),
        ("gpt-4o", 128_000),
        ("gpt-4-turbo", 128_000),
        ("gpt-4", 8_192),
        ("o1", 200_000),
        ("o3", 200_000),
        ("o4-mini", 200_000),
        ("claude-3-5", 200_000),
        ("claude-3-7", 200_000),
        ("claude-3-opus", 200_000),
        ("claude-3-sonnet", 200_000),
        ("claude-3-haiku", 200_000),
        ("deepseek", 128_000),
        ("glm-4", 128_000),
        ("glm-5", 128_000),
        ("gemini", 1_048_576),
    ];
    let model = model.trim().to_ascii_lowercase();
    WINDOWS
        .iter()
        .filter(|(prefix, _)| model.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, window)| *window)
}

fn default_true() -> bool {
    true
}

/// Reads `hideThinkingBlock` from the global `settings.json`, Pi's key for
/// whether reasoning blocks start collapsed.
pub fn load_hide_thinking_block() -> bool {
    let path = settings_path();
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("hideThinkingBlock")?.as_bool())
        .unwrap_or(false)
}

/// The global `settings.json`, beside `config.json`. `OXIDE_SETTINGS_FILE`
/// overrides it, which is how tests (and a project-aware caller) redirect it.
pub fn settings_path() -> PathBuf {
    if let Some(path) = std::env::var_os("OXIDE_SETTINGS_FILE") {
        return PathBuf::from(path);
    }
    config_dir_or_default().join("settings.json")
}

/// Persists one key into a `settings.json`, preserving every other key, so a
/// caller can write the global file or a redirected one. A front-end that
/// needs the path it wrote reads [`settings_path`] itself.
pub fn save_setting_to(path: &Path, key: &str, value: serde_json::Value) -> Result<()> {
    let mut current = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    current
        .as_object_mut()
        .expect("an object")
        .insert(key.to_string(), value);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&current).context("serializing settings")?;
    std::fs::write(path, format!("{text}\n")).with_context(|| format!("writing {}", path.display()))
}

/// Reads `defaultProjectTrust` from the global `settings.json` in the oxide
/// config directory (Pi keeps the same key in `~/.pi/agent/settings.json`).
pub(crate) fn load_default_project_trust() -> crate::trust::DefaultTrust {
    let path = settings_path();
    let Ok(text) = std::fs::read_to_string(path) else {
        return crate::trust::DefaultTrust::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return crate::trust::DefaultTrust::default();
    };
    value
        .get("defaultProjectTrust")
        .and_then(|value| value.as_str())
        .and_then(crate::trust::DefaultTrust::parse)
        .unwrap_or_default()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "gpt-4o-mini".to_string(),
            default_model: None,
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            portkey_config: String::new(),
            provider_models: BTreeMap::new(),
            provider_base_urls: BTreeMap::new(),
            model_catalog: Vec::new(),
            system_prompt: default_system_prompt(),
            max_tokens: default_max_tokens(),
            context_window: 0,
            auto_approve: true,
            reasoning: Reasoning::default(),
            ecosystem: Ecosystem::default(),
            active_agent: None,
            memory: MemoryStore::default(),
            compaction: crate::compact::CompactionConfig::default(),
            prices: crate::pricing::defaults(),
            notify: crate::notify::NotifyConfig::default(),
            tool_filter: crate::cli::ToolFilter::default(),
            ephemeral: false,
            load_context_files: true,
            default_project_trust: crate::trust::DefaultTrust::default(),
            trusted: true,
            workspaces: crate::workspaces::Workspaces::default(),
        }
    }
}

impl Config {
    /// Cost of one usage in USD, or 0 when the model has no known price.
    pub fn usage_cost(&self, usage: &crate::llm::Usage) -> f64 {
        crate::pricing::lookup(&self.prices, &self.model)
            .map(|price| price.cost(usage))
            .unwrap_or(0.0)
    }

    /// Whether the active model is known to support a thinking/reasoning level.
    /// The footer appends ` • <level>` only for these, like Pi's `model.reasoning`.
    pub fn supports_reasoning(&self) -> bool {
        let model = self.model.to_ascii_lowercase();
        const REASONING: [&str; 12] = [
            "o1",
            "o3",
            "o4",
            "gpt-5",
            "claude",
            "deepseek-reasoner",
            "deepseek-v4",
            "deepseek-flash",
            "glm",
            "gemini-2.5",
            "qwen3",
            "sonnet",
        ];
        REASONING.iter().any(|prefix| model.starts_with(prefix))
    }

    /// The model's context window, used for the Pi-style context percentage
    /// and compaction threshold. `OXIDE_CONTEXT_LIMIT` overrides it, then an
    /// explicit `context_window`, then the model's documented window, then the
    /// [`default_context_window`] fallback. Keep the window at least as large
    /// as the response cap for compatibility with older configurations that
    /// used `max_tokens` to raise the window.
    pub fn context_window(&self) -> u64 {
        std::env::var("OXIDE_CONTEXT_LIMIT")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or_else(|| {
                let configured = if self.context_window > 0 {
                    self.context_window
                } else {
                    builtin_context_window(&self.model).unwrap_or_else(default_context_window)
                };
                configured.max(self.max_tokens as u64)
            })
    }

    pub fn config_path() -> PathBuf {
        config_dir_or_default().join("config.json")
    }

    /// Load config from disk, then apply CLI overrides and environment, and
    /// finally discover the project + global ecosystem from `cwd`.
    pub fn load(
        cwd: &Path,
        model: Option<String>,
        provider: Option<String>,
        agent: Option<String>,
        reasoning: Option<String>,
    ) -> Result<Self> {
        let path = Self::config_path();
        let raw: Option<serde_json::Value> = if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading config at {}", path.display()))?;
            Some(
                serde_json::from_str(&text)
                    .with_context(|| format!("parsing config at {}", path.display()))?,
            )
        } else {
            None
        };

        let mut config: Config = match &raw {
            Some(value) => serde_json::from_value(value.clone())
                .with_context(|| format!("parsing config at {}", path.display()))?,
            None => Self::default(),
        };

        let provider_from_env = env_nonempty("OXIDE_PROVIDER");
        let provider_overridden = provider.is_some() || provider_from_env.is_some();
        config.provider = provider
            .or(provider_from_env)
            .unwrap_or_else(|| config.provider.clone());
        let preset = ProviderPreset::for_name(&config.provider);

        if let Some(value) = model {
            config.model = value;
        } else if let Some(value) = env_nonempty("OXIDE_MODEL") {
            config.model = value;
        } else if let Some(preset) = preset {
            if provider_overridden || !explicit(&raw, "model") {
                config.model = preset.model.to_string();
            }
        }

        if let Some(url) = env_nonempty("OXIDE_BASE_URL") {
            config.base_url = url;
        } else if let Some(preset) = preset {
            if let Some(url) = env_nonempty(preset.base_url_env) {
                config.base_url = url;
            } else if provider_overridden || !explicit(&raw, "base_url") {
                config.base_url = config
                    .remembered_base_url(&config.provider)
                    .unwrap_or_else(|| preset.base_url.to_string());
            }
        } else if let Some(url) = env_nonempty("OPENAI_BASE_URL") {
            config.base_url = url;
        } else if provider_overridden || !explicit(&raw, "base_url") {
            // A custom provider's remembered endpoint follows it on startup.
            if let Some(url) = config.remembered_base_url(&config.provider) {
                config.base_url = url;
            }
        }

        let store = AuthStore::load().ok();
        if let Some(store) = &store {
            if let Some(key) = store.key(&config.provider) {
                config.api_key = key.to_string();
            }
        }

        if let Some(preset) = preset {
            if let Some(key) = preset.key_envs().find_map(env_nonempty) {
                config.api_key = key;
            }
        }
        if let Some(key) = env_nonempty("OXIDE_API_KEY") {
            config.api_key = key;
        }
        if config.is_portkey() {
            if let Some(value) = env_nonempty("PORTKEY_CONFIG") {
                config.portkey_config = value;
            }
            if let Some(value) = env_nonempty("PORTKEY_MODELS") {
                config.model_catalog = value
                    .split(',')
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
                    .map(str::to_string)
                    .collect();
            }
        }
        let openai_key_fallback = match preset {
            Some(preset) => preset.kind == ProviderKind::OpenAi && !preset.local,
            None => true,
        };
        if config.api_key.is_empty() && openai_key_fallback {
            if let Some(key) = env_nonempty("OPENAI_API_KEY") {
                config.api_key = key;
            }
        }

        // When no provider was chosen anywhere, fall back to the sole stored
        // credential so one saved TUI login is enough to get started.
        let provider_explicit = provider_overridden || explicit(&raw, "provider");
        if !provider_explicit {
            if let Some(store) = &store {
                apply_stored_provider_fallback(&mut config, store, &raw);
            }
        }

        config.base_url = config.base_url.trim_end_matches('/').to_string();

        if let Some(value) = reasoning.or_else(|| env_nonempty("OXIDE_REASONING")) {
            config.reasoning = Reasoning::parse(&value).with_context(|| {
                format!(
                    "unknown reasoning level `{value}` (expected auto, off, low, medium, or high)"
                )
            })?;
        }
        config.default_project_trust = load_default_project_trust();
        config.workspaces = crate::workspaces::Workspaces::load(cwd);
        config.ecosystem = ecosystem::load(cwd);
        config.memory = MemoryStore::load(cwd);
        config.compaction = crate::compact::load_config(cwd);
        config.prices = crate::pricing::load(cwd);
        config.notify = crate::notify::load_config(cwd);
        if let Some(name) = agent {
            config.activate_agent(&name)?;
        }

        Ok(config)
    }

    /// Reloads the ecosystem honoring a trust decision. Untrusted projects keep
    /// context files but drop project-local resources that can execute or
    /// reshape the agent.
    pub fn reload_ecosystem(&mut self, cwd: &Path) {
        self.ecosystem = if self.trusted {
            ecosystem::load_with(cwd, self.load_context_files)
        } else {
            ecosystem::load_opts(
                cwd,
                ecosystem::LoadOptions {
                    context_files: self.load_context_files,
                    project_resources: false,
                },
            )
        };
    }

    /// Selects a discovered agent as the active one. Fails when the name is
    /// unknown, listing the available agents.
    pub fn activate_agent(&mut self, name: &str) -> Result<()> {
        let Some(active) = self.ecosystem.agent(name).cloned() else {
            let available: Vec<&str> = self
                .ecosystem
                .agents
                .iter()
                .map(|agent| agent.name.as_str())
                .collect();
            anyhow::bail!(
                "unknown agent `{name}` (available: {})",
                available.join(", ")
            );
        };
        self.active_agent = Some(active);
        Ok(())
    }

    pub fn require_api_key(&self) -> Result<&str> {
        self.require_api_key_with(&|name| std::env::var(name).ok())
    }

    /// The same check with the environment injected, so the two providers whose
    /// credential this config does not hold are decided the same way in a test
    /// as at a run.
    pub fn require_api_key_with(&self, env: &dyn Fn(&str) -> Option<String>) -> Result<&str> {
        if self.api_key.trim().is_empty() && self.is_local_provider() {
            // A model server on this machine is reached without a credential,
            // so an empty key is not an error there.
            return Ok(&self.api_key);
        }
        if self.api_key.trim().is_empty() {
            // Two providers keep their credential where this config does not
            // read it: Bedrock signs with AWS credentials from the environment
            // or `~/.aws/credentials`, and Vertex mints its token from a
            // service-account file. An empty `api_key` is therefore not by
            // itself a refusal — but a provider with nothing at all to sign
            // with is refused here, before a turn starts, rather than after the
            // reader has waited on it.
            match self.auth_style() {
                AuthStyle::Aws => {
                    crate::llm::aws::credentials_from(env)?;
                    return Ok(&self.api_key);
                }
                AuthStyle::Vertex if crate::llm::vertex::has_credential_with(env) => {
                    return Ok(&self.api_key);
                }
                _ => {}
            }
            let mut message = format!(
                "no API key found for `{}`. Start the TUI and run `/login {}`, set {}, or add \"api_key\" to {}",
                self.provider,
                self.provider,
                self.preset()
                    .map(|preset| preset.key_envs().collect::<Vec<_>>().join(" or "))
                    .unwrap_or_else(|| self.key_env_name().to_string()),
                Self::config_path().display()
            );
            if let Ok(store) = AuthStore::load() {
                let others: Vec<&str> = store
                    .entries
                    .keys()
                    .filter(|name| name.as_str() != self.provider)
                    .map(String::as_str)
                    .collect();
                if !others.is_empty() {
                    message.push_str(&format!(
                        "\nstored credentials exist for: {} (run with `--provider <name>`)",
                        others.join(", ")
                    ));
                }
            }
            anyhow::bail!(message);
        }
        Ok(&self.api_key)
    }

    /// Applies a provider credential to the running config. Switching providers
    /// remembers the outgoing provider's model and restores the incoming one's,
    /// so several stored logins keep separate model choices.
    pub fn apply_provider(&mut self, provider: &str, key: &str) {
        let current = canonical_provider(&self.provider);
        let target = canonical_provider(provider);
        let provider_changed = current != target;
        if provider_changed && !current.is_empty() && !self.model.trim().is_empty() {
            self.provider_models
                .insert(current.clone(), self.model.clone());
        }
        if provider_changed && !current.is_empty() {
            self.remember_base_url(&current);
        }
        self.provider = provider.to_string();
        self.api_key = key.to_string();
        if provider_changed {
            self.model = self.model_for_provider(provider);
            match ProviderPreset::for_name(provider) {
                Some(preset) => {
                    self.base_url = self
                        .provider_base_urls
                        .get(&target)
                        .cloned()
                        .unwrap_or_else(|| preset.base_url.to_string());
                }
                // A custom provider keeps whatever endpoint it was last given;
                // the previous provider's URL is used when none is remembered.
                None => {
                    if let Some(url) = self.provider_base_urls.get(&target).cloned() {
                        self.base_url = url;
                    }
                }
            }
        }
    }

    /// Records the outgoing provider's endpoint when it was customized, or
    /// clears it when it matches the preset default, so each provider only
    /// carries a `base_url` it actually needs.
    fn remember_base_url(&mut self, provider: &str) {
        let name = canonical_provider(provider);
        let default = ProviderPreset::for_name(&name).map(|preset| preset.base_url.to_string());
        if self.base_url.trim().is_empty() || default.as_deref() == Some(self.base_url.as_str()) {
            self.provider_base_urls.remove(&name);
        } else {
            self.provider_base_urls.insert(name, self.base_url.clone());
        }
    }

    /// The custom endpoint remembered for a provider, if one was saved.
    fn remembered_base_url(&self, provider: &str) -> Option<String> {
        self.provider_base_urls
            .get(&canonical_provider(provider))
            .cloned()
    }

    /// The endpoint to use with a provider: the custom one remembered for it,
    /// the active provider's current URL, otherwise the provider preset's.
    pub fn base_url_for_provider(&self, provider: &str) -> String {
        let name = canonical_provider(provider);
        if let Some(url) = self.remembered_base_url(&name) {
            return url;
        }
        if name == canonical_provider(&self.provider) && !self.base_url.trim().is_empty() {
            return self.base_url.clone();
        }
        ProviderPreset::for_name(&name)
            .map(|preset| preset.base_url.to_string())
            .unwrap_or_else(|| self.base_url.clone())
    }

    /// The absolute URL a chat request is posted to.
    pub fn chat_url(&self) -> Result<String> {
        self.endpoint_url(Endpoint::Chat)
    }

    /// The absolute URL the model listing is read from.
    pub fn models_url(&self) -> Result<String> {
        self.endpoint_url(Endpoint::Models)
    }

    /// Most providers append one path to `base_url`, while the ones that put the
    /// model, the deployment or the account in the path — and the version in a
    /// query — build the whole URL from what the configuration and the
    /// environment hold. Naming a base URL by hand still overrides the host for
    /// the ones with a derived one, so a private gateway or an emulator is
    /// reached the same way.
    fn endpoint_url(&self, endpoint: Endpoint) -> Result<String> {
        let base = self.base_url.trim().trim_end_matches('/');
        // GitLab Duo is reached through its own gateway's proxy rather than a
        // host of its own, and that proxy is where both dialects live.
        if self.auth_style() == AuthStyle::Gitlab {
            let gateway = self.gitlab_gateway();
            // Both proxies carry the provider's own list, so the one the model
            // speaks is the one the listing is read from — the same base, and so
            // the same headers, a turn with that model already uses.
            let proxy = match self.provider_kind() {
                ProviderKind::Anthropic => crate::llm::gitlab::anthropic_base(&gateway),
                _ => crate::llm::gitlab::openai_base(&gateway),
            };
            return Ok(match endpoint {
                Endpoint::Chat if self.provider_kind() == ProviderKind::Anthropic => {
                    format!("{proxy}/messages")
                }
                Endpoint::Chat => format!("{proxy}/chat/completions"),
                Endpoint::Models => format!("{proxy}/models"),
            });
        }
        match self.provider_kind() {
            ProviderKind::Anthropic => {
                if base.is_empty() {
                    anyhow::bail!(
                        "the `{}` provider needs a base URL — set `base_url` in config.json",
                        self.provider
                    );
                }
                Ok(format!("{base}/messages"))
            }
            ProviderKind::Gemini => self.gemini_url(endpoint, base),
            ProviderKind::Bedrock => Ok(String::new()),
            ProviderKind::OpenAi if self.auth_style() == AuthStyle::Azure => {
                self.azure_url(endpoint, base)
            }
            ProviderKind::OpenAi => {
                if base.is_empty() {
                    anyhow::bail!(
                        "the `{}` provider needs a base URL — set `base_url` in config.json",
                        self.provider
                    );
                }
                Ok(match endpoint {
                    Endpoint::Chat => format!("{base}/chat/completions"),
                    Endpoint::Models => format!("{base}/models"),
                })
            }
        }
    }

    /// Azure OpenAI reaches a deployment rather than a model, and pins the
    /// request to an API version, so both the path and the query carry choices
    /// the base URL does not.
    fn azure_url(&self, endpoint: Endpoint, base: &str) -> Result<String> {
        let host = if !base.is_empty() {
            base.to_string()
        } else if let Some(endpoint) = env_nonempty("AZURE_OPENAI_ENDPOINT") {
            endpoint.trim_end_matches('/').to_string()
        } else if let Some(resource) =
            env_nonempty("AZURE_OPENAI_RESOURCE").or_else(|| env_nonempty("AZURE_RESOURCE_NAME"))
        {
            format!("https://{}.openai.azure.com", resource.trim_matches('.'))
        } else {
            anyhow::bail!(
                "the Azure provider needs an endpoint — set AZURE_OPENAI_ENDPOINT, or a \
                 `base_url` in config.json"
            );
        };
        let version = env_nonempty("AZURE_OPENAI_API_VERSION")
            .or_else(|| env_nonempty("AZURE_API_VERSION"))
            .unwrap_or_else(|| AZURE_API_VERSION.to_string());
        Ok(match endpoint {
            Endpoint::Chat => {
                let deployment = env_nonempty("AZURE_OPENAI_DEPLOYMENT")
                    .unwrap_or_else(|| self.model.trim().to_string());
                if deployment.is_empty() {
                    anyhow::bail!(
                        "the Azure provider needs a deployment — set a model (the deployment's \
                         name) or AZURE_OPENAI_DEPLOYMENT"
                    );
                }
                format!(
                    "{host}/openai/deployments/{deployment}/chat/completions?api-version={version}"
                )
            }
            Endpoint::Models => format!("{host}/openai/models?api-version={version}"),
        })
    }

    /// Gemini on the Gemini API names the model in the path and streams from
    /// `streamGenerateContent`; on Vertex the same API sits under a project and
    /// a location. The key is a header on the first and an OAuth token on the
    /// second, which the client supplies.
    fn gemini_url(&self, endpoint: Endpoint, base: &str) -> Result<String> {
        if self.auth_style() == AuthStyle::Vertex {
            let (host, prefix) = self.vertex_target()?;
            return Ok(match endpoint {
                Endpoint::Chat => format!(
                    "{host}{prefix}/publishers/google/models/{}:streamGenerateContent?alt=sse",
                    self.model
                ),
                Endpoint::Models => {
                    format!("{host}{prefix}/publishers/google/models")
                }
            });
        }
        if base.is_empty() {
            anyhow::bail!(
                "the `{}` provider needs a base URL — set `base_url` in config.json",
                self.provider
            );
        }
        Ok(match endpoint {
            Endpoint::Chat => format!("{base}/models/{}:streamGenerateContent?alt=sse", self.model),
            Endpoint::Models => format!("{base}/models"),
        })
    }

    /// The Vertex host and the project path prefix a request goes under. The
    /// project and location come from the environment the Google Cloud SDKs
    /// read, unless a base URL was set by hand.
    pub fn vertex_target(&self) -> Result<(String, String)> {
        let base = self.base_url.trim().trim_end_matches('/');
        let location = env_nonempty("GOOGLE_VERTEX_LOCATION")
            .or_else(|| env_nonempty("GOOGLE_CLOUD_LOCATION"))
            .or_else(|| env_nonempty("CLOUD_ML_REGION"))
            .unwrap_or_else(|| "us-central1".to_string());
        let host = if !base.is_empty() {
            base.to_string()
        } else if location == "global" {
            "https://aiplatform.googleapis.com/v1".to_string()
        } else {
            format!("https://{location}-aiplatform.googleapis.com/v1")
        };
        if let Some(project) = env_nonempty("GOOGLE_VERTEX_PROJECT")
            .or_else(|| env_nonempty("GOOGLE_CLOUD_PROJECT"))
            .or_else(|| env_nonempty("CLOUD_ML_PROJECT_ID"))
            .or_else(|| crate::llm::vertex::project_of(&self.api_key))
        {
            return Ok((
                host,
                format!("/projects/{}/locations/{location}", project.trim()),
            ));
        }
        anyhow::bail!(
            "the Vertex provider needs a project — set GOOGLE_VERTEX_PROJECT (and \
             GOOGLE_VERTEX_LOCATION), or a `base_url` that already names one"
        )
    }

    /// Applies the optional settings chosen in the login dialog. Blank values
    /// keep the provider's current/default model, endpoint, or Config ID, and
    /// a Portkey Config ID is ignored for other providers.
    pub fn apply_login_options(&mut self, model: &str, base_url: &str, portkey_config: &str) {
        let name = canonical_provider(&self.provider);
        if !model.trim().is_empty() {
            self.model = model.trim().to_string();
            self.provider_models
                .insert(name.clone(), self.model.clone());
        }
        if !base_url.trim().is_empty() {
            self.base_url = base_url.trim().trim_end_matches('/').to_string();
            self.remember_base_url(&name);
        }
        if name == "portkey" && !portkey_config.trim().is_empty() {
            self.portkey_config = portkey_config.trim().to_string();
        }
    }

    /// The model to use with a provider: the one remembered for it, otherwise
    /// the provider preset's, otherwise the model in use.
    pub fn model_for_provider(&self, provider: &str) -> String {
        let name = canonical_provider(provider);
        self.provider_models
            .get(&name)
            .cloned()
            .or_else(|| ProviderPreset::for_name(&name).map(|preset| preset.model.to_string()))
            .unwrap_or_else(|| self.model.clone())
    }

    /// A copy of this config pointed at another provider, for querying that
    /// provider's catalog without touching the running session. The Portkey
    /// model catalog only carries over to Portkey itself.
    pub fn for_provider(&self, provider: &str, key: &str) -> Self {
        let mut config = self.clone();
        config.apply_provider(provider, key);
        if !config.is_portkey() {
            config.model_catalog.clear();
        }
        config
    }

    /// Persists the active provider, its model, and the per-provider model
    /// memory so the next launch resumes this selection.
    pub fn persist_selection_at(&self, path: &Path) -> Result<()> {
        let provider = canonical_provider(&self.provider);
        let model = self.model.clone();
        let memory = self.provider_models.clone();
        let endpoints = self.provider_base_urls.clone();
        let base_url = self.base_url.clone();
        let portkey_config = self.portkey_config.trim().to_string();
        let default_url =
            ProviderPreset::for_name(&provider).map(|preset| preset.base_url.to_string());
        Self::update_at(path, move |object| {
            object.insert(
                "provider".to_string(),
                serde_json::Value::String(provider.clone()),
            );
            object.insert("model".to_string(), serde_json::Value::String(model));
            // `base_url` describes the active provider only; clear it when it
            // is the preset default so a switch cannot leave a custom gateway
            // behind for the next provider.
            if base_url.trim().is_empty() || default_url.as_deref() == Some(base_url.as_str()) {
                object.remove("base_url");
            } else {
                object.insert("base_url".to_string(), serde_json::Value::String(base_url));
            }
            if !memory.is_empty() {
                let memory_entry = object
                    .entry("provider_models")
                    .or_insert_with(|| serde_json::json!({}));
                if !memory_entry.is_object() {
                    *memory_entry = serde_json::json!({});
                }
                if let Some(object) = memory_entry.as_object_mut() {
                    for (name, model) in memory {
                        object.insert(name, serde_json::Value::String(model));
                    }
                }
            }
            if endpoints.is_empty() {
                object.remove("provider_base_urls");
            } else {
                object.insert(
                    "provider_base_urls".to_string(),
                    serde_json::to_value(&endpoints).unwrap_or_else(|_| serde_json::json!({})),
                );
            }
            // Portkey's Config ID is Portkey's own setting, so it travels with
            // the selection whatever provider is active — a login that chose
            // one would otherwise be routed only until the next launch.
            if !portkey_config.is_empty() {
                object.insert(
                    "portkey_config".to_string(),
                    serde_json::Value::String(portkey_config),
                );
            }
        })
    }

    /// Persists the active provider in `config.json` so the next launch uses it,
    /// preserving any other settings already in the file.
    pub fn set_active_provider_at(path: &Path, provider: &str) -> Result<()> {
        Self::set_active_field_at(path, "provider", provider)
    }

    /// Persists the selected model in `config.json`, preserving other settings.
    pub fn set_active_model_at(path: &Path, model: &str) -> Result<()> {
        Self::set_active_field_at(path, "model", model)
    }

    /// Persists the model used by default for new sessions (`/models` Ctrl+S).
    pub fn set_default_model_at(path: &Path, model: &str) -> Result<()> {
        Self::set_active_field_at(path, "default_model", model)
    }

    /// Persists the selected theme name in `config.json`, so the CLI and the
    /// desktop both start with it (the CLI reads the same `theme` key).
    pub fn set_theme_at(path: &Path, name: &str) -> Result<()> {
        Self::set_active_field_at(path, "theme", name)
    }

    /// Persists whether a permission-gated tool runs without asking, so the
    /// next launch keeps the answer `/approvals on|off` gave.
    pub fn set_auto_approve_at(path: &Path, auto_approve: bool) -> Result<()> {
        Self::update_at(path, move |object| {
            object.insert(
                "auto_approve".to_string(),
                serde_json::Value::Bool(auto_approve),
            );
        })
    }

    fn set_active_field_at(path: &Path, key: &str, value: &str) -> Result<()> {
        let key = key.to_string();
        let value = value.to_string();
        Self::update_at(path, move |object| {
            object.insert(key, serde_json::Value::String(value));
        })
    }

    /// Reads, edits, and writes `config.json`, preserving every other setting
    /// and recovering from a missing or malformed file.
    fn update_at(
        path: &Path,
        update: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
    ) -> Result<()> {
        let mut root: serde_json::Value = if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading config at {}", path.display()))?;
            serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };
        if !root.is_object() {
            root = serde_json::json!({});
        }
        if let Some(object) = root.as_object_mut() {
            update(object);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&root)?;
        std::fs::write(path, text)
            .with_context(|| format!("writing config to {}", path.display()))?;
        Ok(())
    }

    /// The API dialect this configuration targets.
    pub fn provider_kind(&self) -> ProviderKind {
        let kind = self
            .preset()
            .map(|preset| preset.kind)
            .unwrap_or(ProviderKind::OpenAi);
        // Duo reaches the gateway's two proxies, and the model says which one
        // serves it: a Claude model is proxied to Anthropic, everything else to
        // OpenAI.
        if kind == ProviderKind::OpenAi
            && self.auth_style() == AuthStyle::Gitlab
            && self.model.trim().to_ascii_lowercase().starts_with("claude")
        {
            return ProviderKind::Anthropic;
        }
        kind
    }

    /// The preset behind the active provider name, when it is a known one.
    pub fn preset(&self) -> Option<ProviderPreset> {
        ProviderPreset::for_name(&self.provider)
            .or_else(|| ProviderPreset::for_base_url(&self.base_url))
    }

    /// How this configuration authenticates, and which host serves it.
    /// The GitLab instance a Duo credential is presented to — the service that
    /// mints the gateway token, which is not the host a turn streams from.
    pub fn gitlab_instance(&self) -> String {
        env_nonempty("GITLAB_INSTANCE_URL")
            .or_else(|| env_nonempty("GITLAB_URL"))
            .map(|instance| instance.trim().trim_end_matches('/').to_string())
            .unwrap_or_else(|| crate::llm::gitlab::INSTANCE.to_string())
    }

    /// The AI gateway a GitLab Duo request goes to, which a reader may point at
    /// a self-managed instance's own gateway instead.
    fn gitlab_gateway(&self) -> String {
        let base = self.base_url.trim();
        if !base.is_empty() {
            return base.trim_end_matches('/').to_string();
        }
        env_nonempty("GITLAB_AI_GATEWAY_URL")
            .map(|gateway| gateway.trim().trim_end_matches('/').to_string())
            .unwrap_or_else(|| crate::llm::gitlab::GATEWAY.to_string())
    }

    pub fn auth_style(&self) -> AuthStyle {
        self.preset()
            .map(|preset| preset.auth)
            .unwrap_or(AuthStyle::Bearer)
    }

    /// Whether the provider is a server on this machine, which may run without
    /// a credential.
    pub fn is_local_provider(&self) -> bool {
        self.preset().map(|preset| preset.local).unwrap_or(false)
    }

    /// Whether the provider has no usable model-listing endpoint, so the
    /// bundled catalog is what the picker shows instead of a failed request.
    pub fn lists_no_models(&self) -> bool {
        self.preset()
            .map(|preset| !preset.models.is_empty())
            .unwrap_or(false)
    }

    /// The environment variable that supplies the API key for this provider.
    pub fn key_env_name(&self) -> &'static str {
        self.preset()
            .map(|preset| preset.key_env)
            .unwrap_or("OPENAI_API_KEY")
    }

    pub fn is_portkey(&self) -> bool {
        canonical_provider(&self.provider) == "portkey"
    }

    /// Whether the active provider is Z.AI's GLM API, which selects thinking
    /// with `thinking.type` instead of `reasoning_effort`. A custom provider
    /// pointed at either Z.AI host is detected the same way Pi does.
    pub fn is_zai(&self) -> bool {
        if canonical_provider(&self.provider) == "zai" {
            return true;
        }
        let base_url = self.base_url.to_ascii_lowercase();
        base_url.contains("api.z.ai") || base_url.contains("open.bigmodel.cn")
    }

    /// Whether the active model is DeepSeek's thinking mode, wherever it is
    /// served from. Its `reasoning_content` has to be replayed on every later
    /// assistant message, so the provider and the first-party endpoint are
    /// checked along with the model name, which covers a DeepSeek model behind
    /// a gateway (OpenRouter, Portkey).
    pub fn is_deepseek(&self) -> bool {
        canonical_provider(&self.provider) == "deepseek"
            || self
                .base_url
                .to_ascii_lowercase()
                .contains("api.deepseek.com")
            || self.model.to_ascii_lowercase().contains("deepseek")
    }

    /// The models bundled with a provider whose catalog cannot be listed.
    fn bundled_models(&self) -> Vec<String> {
        self.preset()
            .map(|preset| preset.models)
            .unwrap_or(&[])
            .iter()
            .map(|model| (*model).to_string())
            .collect()
    }

    pub fn model_catalog(&self) -> Vec<String> {
        let bundled = self.bundled_models();
        let mut models = if self.model_catalog.is_empty() {
            bundled.clone()
        } else {
            self.model_catalog.clone()
        };
        if !bundled.is_empty() && !self.model.trim().is_empty() {
            models.push(self.model.clone());
        }
        models.sort();
        models.dedup();
        models
    }

    /// Normalizes the list of models reported by the provider. A known built-in
    /// provider is authoritative, so the active model is only injected when the
    /// provider reports nothing (or the endpoint is not recognized), keeping
    /// stale or hand-typed ids out of the picker.
    pub fn merge_model_catalog(&self, mut models: Vec<String>) -> Vec<String> {
        let known = ProviderPreset::for_name(&self.provider).is_some();
        if (!known || models.is_empty()) && !self.model.trim().is_empty() {
            models.push(self.model.clone());
        }
        models.sort();
        models.dedup();
        models
    }

    /// Builds the effective system prompt from the base prompt plus the active
    /// agent, loaded memory, instructions, and an index of available
    /// skills/commands/subagents.
    pub fn compose_system_prompt(&self) -> String {
        // `.oxide/SYSTEM.md` replaces the default prompt; `APPEND_SYSTEM.md`
        // (and CLI `--append-system-prompt`) layer on top.
        let mut sections = vec![self
            .ecosystem
            .system_prompt
            .clone()
            .unwrap_or_else(|| self.system_prompt.clone())];
        sections.extend(
            self.ecosystem
                .append_system_prompt
                .iter()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty()),
        );

        if let Some(agent) = &self.active_agent {
            sections.push(format!(
                "You are operating as the `{}` agent.\n{}",
                agent.name, agent.prompt
            ));
        }

        for memory in &self.ecosystem.memory {
            sections.push(format!(
                "# Context: {}\n{}",
                memory.name,
                memory.content.trim()
            ));
        }
        for rule in &self.ecosystem.rules {
            sections.push(format!(
                "# Instructions: {}\n{}",
                rule.name,
                rule.content.trim()
            ));
        }

        if let Some(section) = self.workspaces.section() {
            sections.push(section);
        }

        let remembered = self.memory.recent(8);
        if !remembered.is_empty() {
            let mut list = String::from(
                "# Persistent memory\nNotes retained across sessions. Use the `memory` tool to add, search, or forget entries.",
            );
            for entry in &remembered {
                let scope = match entry.scope {
                    Scope::Project => "project",
                    Scope::User => "user",
                };
                list.push_str(&format!("\n- [{scope}] {}", entry.content));
            }
            sections.push(list);
        }

        if !self.ecosystem.skills.is_empty() {
            let mut list = String::from(
                "# Available skills\nLoad a skill with the `skill` tool when its description \
                 matches the task; users can also force one by naming it after a slash \
                 (`/<name>`, which every front-end lists).",
            );
            for skill in &self.ecosystem.skills {
                let description = skill.description.clone().unwrap_or_default();
                list.push_str(&format!("\n- {}: {}", skill.name, description));
            }
            sections.push(list);
        }

        if !self.ecosystem.commands.is_empty() || !self.ecosystem.prompt_templates.is_empty() {
            let mut list = String::from(
                "# Available commands\nInvoke a command with the `command` tool when the user's \
                 request matches its description; pass any focus or scope as `arguments`.",
            );
            for command in &self.ecosystem.commands {
                let description = command.description.clone().unwrap_or_default();
                list.push_str(&format!("\n- /{}: {}", command.name, description));
            }
            for template in &self.ecosystem.prompt_templates {
                if self.ecosystem.command(&template.name).is_some() {
                    continue;
                }
                let description = template.description.clone().unwrap_or_default();
                list.push_str(&format!("\n- /{}: {}", template.name, description));
            }
            sections.push(list);
        }

        let subagents: Vec<&AgentDef> = self
            .ecosystem
            .agents
            .iter()
            .filter(|agent| agent.mode != ecosystem::AgentMode::Primary)
            .collect();
        if !subagents.is_empty() {
            let mut list = String::from("# Available subagents");
            for agent in subagents {
                let description = agent.description.clone().unwrap_or_default();
                list.push_str(&format!("\n- {}: {}", agent.name, description));
            }
            sections.push(list);
        }

        if !self.ecosystem.plugins.is_empty() || !self.ecosystem.hooks.is_empty() {
            let mut list = String::from(
                "# Plugins\nInstalled plugins already extend this run: use what one provides instead \
                 of building the same thing by hand, and never tell the user to install a plugin \
                 that is listed here. One marked `(disabled)` is installed but switched off — say \
                 so and let the user switch it on (`/plugins` in the terminal, or \
                 `oxide plugin enable <name>`) rather than suggesting an install.",
            );
            for plugin in &self.ecosystem.plugins {
                let version = plugin
                    .version
                    .as_deref()
                    .map(|version| format!(" v{version}"))
                    .unwrap_or_default();
                let mut row = format!("- {}{version}", plugin.name);
                if let Some(description) = plugin
                    .description
                    .as_deref()
                    .filter(|text| !text.is_empty())
                {
                    row.push_str(&format!(" — {description}"));
                }
                let capabilities = plugin.capabilities();
                if !capabilities.is_empty() {
                    row.push_str(&format!(" ({capabilities})"));
                }
                if !plugin.enabled {
                    row.push_str(" (disabled)");
                }
                list.push_str(&format!("\n{row}"));
            }
            if !self.ecosystem.hooks.is_empty() {
                list.push_str(&format!(
                    "\nHook plugins are active ({}): a hook can rewrite a tool call's arguments \
                     before it runs and a tool's output before you see it, so output that looks \
                     changed by something else may be theirs.",
                    self.ecosystem.hooks.len()
                ));
            }
            sections.push(list);
        }

        if !self.ecosystem.mcp.is_empty() {
            let mut list = String::from(
                "# MCP servers\nRoute requests to the matching MCP server: a server whose URL or \
                 service name the message mentions is loaded before your first step, so for any \
                 other request that belongs to a configured service, call mcp_load for that server \
                 first and use its tools instead of webfetch, so authenticated documents, tickets, \
                 and other resources stay accessible. Never ask the user for an identifier a \
                 configured server can discover itself — a cloud, space, project, board or channel \
                 id, or a URL — because its tools can resolve it.",
            );
            for server in &self.ecosystem.mcp {
                if !server.enabled {
                    continue;
                }
                let domains = server.domains();
                let domains = if domains.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", domains.join(", "))
                };
                list.push_str(&format!("\n- {}{}", server.name, domains));
            }
            sections.push(list);
        }

        sections.push(
            "# Tool use\nWork in larger batches to avoid extra round trips. Read a whole file (or a \
             wide `offset`/`limit`) once instead of re-reading the same path in small slices, and \
             issue several independent `read`, `grep`, `find`, or `ls` calls in the same step. Use \
             `grep` to locate a symbol or string, then read the surrounding lines. Only re-read a \
             file after you edit it. Before you edit a region, read it and copy `oldText` from \
             that result — the `N|` line numbers and trailing whitespace are tolerated, but text \
             that has moved on is not, and a mismatch replies with the closest region to copy. \
             Prefer the dedicated tools over shell equivalents: `read` to \
             inspect a file, `grep` to find text, `find` to locate files, and `ls` to list a \
             directory. Search the codebase with `grep`/`find`, never with a shell \
             `grep -r`/`rg`/`find` from the repo root: the tools skip `.git/`, `target/`, \
             `node_modules/` and `.venv/` and honor `.gitignore`; `grep` spreads its scan across \
             threads, while a shell search reads every build artifact — and a `| grep -v` filter \
             after it cannot give back the time already spent. Scope a shell search to one \
             directory and reserve it for a command's own output (`git log | grep`); never sweep \
             the whole filesystem with `find /`, or the home directory with `find ~`, which reads \
             every unrelated project on the machine before the command times out. Keep each \
             `bash` command focused on one task \
             instead of chaining unrelated commands with `;` or `&&`. `read`, `ls`, `find`, and \
             `grep` accept absolute paths, so you do not need a shell to inspect files outside \
             the project, and `bash` already starts in the project root, so run a command \
             directly instead of prefixing `cd <root> &&`. Create a file — including a scratch or \
             verification script — with the `write` tool rather than a shell heredoc, and keep it \
             inside the project: a script run from the temp directory resolves its relative imports \
             against its own directory, not the working directory, so `require('./...')` fails the \
             moment it runs. When a command can print a large \
             payload, select just the fields you need (for example a `--jq`/`jq` filter) rather \
             than piping it through `head`, which still fetches and renders everything."
                .to_string(),
        );

        sections.push(
            "# Definition of Done\nAn explicit instruction from the user outranks every check below: when \
             they say a check is not needed (\"no need to check the PR's status\", \"skip CI\", \"don't \
             wait for the build\"), do what they asked and say plainly what you did not verify. Never \
             treat that instruction as something to override, and do not run the check anyway to be \
             safe. \
             A task is not done until you have verified the outcome otherwise, and \
             tool output is the only evidence — a command that failed, printed nothing, or was never \
             run is not proof. After each state-changing action, confirm the result before \
             reporting success, and re-read what you changed. This applies to every kind of work, \
             not just code: an edit is on disk and the build/tests/lint pass; a pull request's CI \
             and mergeability are checked (`gh pr checks <url>`, `gh pr view <url> --json \
             state,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup`) and pending checks \
             are waited for; a comment, review, or ticket reply is read back to confirm it landed \
             in the right place; a release, deployment, migration, or published artifact is \
             queried for its status; and any task with a known verifier is run through it. Run a \
             verifier once and read its whole result — re-running the same build or test on \
             unchanged files is slow and adds nothing. \
             Separate what you verified (\"ran ./gradlew test — passed\", \"`grep` found the \
             annotation\") from what you assume, and say plainly when you could not confirm \
             something. Do not claim a change is applied, a fix works, a PR is ready, a reply is \
             posted, or a release is out without the output that shows it. A code change made in \
             response to a pull request or review is delivered only once it is committed and \
             pushed to the branch under review — fix and push the code before you reply in the \
             thread, and confirm the new commit before you summarize."
                .to_string(),
        );

        sections.push(
            "# Scope\nKeep every change scoped to what was asked. Before you commit, run \
             `git status` and `git diff --staged` and stage only the paths your work touched with \
             explicit `git add <path>` — never blanket-stage with `git add -A`, `git add --all`, \
             `git add .`, or `git commit -a`/`-am` in a tree that may hold unrelated edits. Leave \
             local-only and generated files out of commits and pull requests: \
             `.claude/settings.local.json`, `.idea/`, `.vscode/`, `.DS_Store`, editor state, build \
             output, and unrelated lockfile churn. Do not revert someone else's unrelated changes \
             without asking, but exclude them. Before you open or update a pull request, review its \
             file list (`git diff --name-only <base>...HEAD`, `gh pr diff --name-only`, or \
             `gh pr view <url> --json files`) and drop anything unrelated it picked up."
                .to_string(),
        );

        sections.push(
            "# GitHub and GitLab\nWork with GitHub and GitLab pull/merge requests and issues through \
             their CLIs (`gh` and `glab`) rather than `webfetch`: they authenticate, so private \
             repositories and review threads are reachable, and return structured data. Use \
             `gh pr view <url-or-number> --comments` / `glab mr view` to read an item, `gh pr diff` / \
             `glab mr diff` for its changes, `gh pr checks` for CI, and `gh pr comment` / \
             `glab mr note` for a new top-level comment. Reserve `webfetch` for public pages \
             that have no CLI equivalent.\nReply to code review comments inside their existing \
             threads instead of \
             posting one general comment: list them with `gh api repos/{owner}/{repo}/pulls/<n>/comments` \
             and answer one with \
             `gh api -X POST repos/{owner}/{repo}/pulls/<n>/comments/<comment_id>/replies -f body=<text>` \
             (`gh pr comment` is only for a new top-level comment). Select just the fields you need \
             and format API output for a person to read — one line per item, not minified JSON. Fix \
             and push the code before you reply: commit the change and `git push` it to the branch \
             under review first, so a reply never claims a comment is addressed while the branch \
             still has the old code.\nNever \
             commit to or push the repository's default branch. Determine it first with \
             `git symbolic-ref --short refs/remotes/origin/HEAD` or \
             `gh repo view --json defaultBranchRef --jq .defaultBranchRef.name` (fall back to \
             `main`/`master` only when neither is set): branch protection rejects a direct push \
             and you would have to undo the commit. When you are asked to raise a pull request \
             while on the default branch, create a feature branch first, commit there, push that \
             branch, and open the PR from it.\nAny summary about a pull or merge request — one \
             you opened, updated, checked, or replied to — names it with a link the user can \
             click (`[#123 title](https://github.com/owner/repo/pull/123)`, the URL `gh pr create` \
             printed or `gh pr view <number> --json url` reports; `glab` reports the same for a \
             merge request), never as a bare `#123`: the desktop app and the VS Code panel open a \
             Markdown link in the browser and a terminal links the printed URL, while a number \
             on its own leaves the user nothing to open.\nName a commit the same way — \
             `[<short sha>](<url>)` — in a report, a reply to a review comment or any other \
             summary: the URL is the one the forge reports for that commit \
             (`gh api repos/{owner}/{repo}/commits/<sha> --jq .html_url` on GitHub), never a \
             template filled in by hand, since a GitLab project answers its own \
             `https://gitlab.com/owner/repo/-/commit/<sha>` on whatever domain it lives on and \
             not a `github.com/commit/` path. A hash written on its own is not turned into a \
             link by the forge, so a reply that says which commit addressed a comment carries \
             that link and never the hash alone."
                .to_string(),
        );

        if self.compaction.enabled {
            sections.push(
                "# Context management\nOlder turns are summarized automatically as the context \
                 fills up; the most recent work is kept verbatim. Keep summaries of your own work \
                 close to the end of a task so nothing important is lost."
                    .to_string(),
            );
        }

        sections.join("\n\n")
    }

    /// Resolves a leading `/command` to its expanded prompt and any agent
    /// routing (`agent`, `subtask`) declared in the command's frontmatter.
    pub fn resolve_command(&self, input: &str) -> Option<ecosystem::ResolvedCommand> {
        self.ecosystem.resolve_command(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_enforces_scope() {
        let config = Config::default();
        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("# Scope"), "{prompt}");
        assert!(prompt.contains("never blanket-stage"), "{prompt}");
        assert!(prompt.contains(".claude/settings.local.json"), "{prompt}");
    }

    #[test]
    fn system_prompt_defines_done() {
        let config = Config::default();
        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("# Definition of Done"), "{prompt}");
        assert!(prompt.contains("not just code"), "{prompt}");
        assert!(prompt.contains("gh pr checks"), "{prompt}");
        assert!(prompt.contains("deployment"), "{prompt}");
    }

    #[test]
    fn system_prompt_lets_an_explicit_user_instruction_outrank_the_checks() {
        let config = Config::default();
        let prompt = config.compose_system_prompt();
        let done = prompt
            .split("# Definition of Done")
            .nth(1)
            .and_then(|rest| rest.split("# ").next())
            .expect("the section is composed");
        assert!(
            done.contains("explicit instruction from the user outranks"),
            "{done}"
        );
        // The example the section has to survive is a user asking for a pull
        // request without a status check.
        assert!(done.contains("no need to check the PR's status"), "{done}");
        assert!(done.contains("do not run the check anyway"), "{done}");
    }

    #[test]
    fn system_prompt_links_the_pull_request_a_summary_is_about() {
        let config = Config::default();
        let prompt = config.compose_system_prompt();
        let forge = prompt
            .split("# GitHub and GitLab")
            .nth(1)
            .and_then(|rest| rest.split("# ").next())
            .expect("the section is composed");
        assert!(
            forge.contains("names it with a link the user can click"),
            "{forge}"
        );
        // The link has to be the URL, not the number the model already has.
        assert!(forge.contains("gh pr view <number> --json url"), "{forge}");
        assert!(forge.contains("never as a bare `#123`"), "{forge}");
        // A commit is opened the same way: a hash on its own is not a link, and
        // the URL is the forge's own rather than a GitHub-shaped template.
        assert!(
            forge.contains("in a report, a reply to a review comment"),
            "{forge}"
        );
        assert!(
            forge.contains("gh api repos/{owner}/{repo}/commits/<sha> --jq .html_url"),
            "{forge}"
        );
        assert!(
            forge.contains("https://gitlab.com/owner/repo/-/commit/<sha>"),
            "{forge}"
        );
        assert!(forge.contains("not a `github.com/commit/` path"), "{forge}");
        assert!(forge.contains("never the hash alone"), "{forge}");
    }

    #[test]
    fn system_prompt_encourages_batched_tool_use() {
        let config = Config::default();
        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("# Tool use"));
        assert!(prompt.contains("batches"));
        assert!(
            prompt.contains("already starts in the project root"),
            "{prompt}"
        );
        // A scratch script outside the project breaks a relative import, so the
        // prompt steers it to `write` inside the project instead of a heredoc.
        assert!(prompt.contains("resolves its relative imports"), "{prompt}");
    }

    #[test]
    fn system_prompt_keeps_searches_out_of_the_shell() {
        let config = Config::default();
        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("never with a shell"), "{prompt}");
        assert!(
            prompt.contains("never sweep the whole filesystem"),
            "{prompt}"
        );
        assert!(prompt.contains("honor `.gitignore`"), "{prompt}");
        // Only `grep` fans its scan across threads; `find` dispatches to the
        // sequential `walk` loop, so the prompt must not credit both.
        assert!(
            prompt.contains("`grep` spreads its scan across"),
            "{prompt}"
        );
        assert!(!prompt.contains("walk in parallel"), "{prompt}");
    }

    #[test]
    fn system_prompt_prefers_forge_clis() {
        let config = Config::default();
        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("# GitHub and GitLab"), "{prompt}");
        assert!(prompt.contains("`gh` and `glab`"), "{prompt}");
        assert!(prompt.contains("inside their existing threads"), "{prompt}");
        assert!(prompt.contains("/replies"), "{prompt}");
        // The forge guidance must not also call a top-level comment a reply:
        // the first sentence that names `gh pr comment` once said "to reply",
        // and the model followed it instead of the inline instruction below.
        assert!(
            prompt.contains("`glab mr note` for a new top-level comment"),
            "{prompt}"
        );
        assert!(!prompt.contains("`glab mr note` to reply"), "{prompt}");
    }

    #[test]
    fn context_window_derives_from_the_model() {
        // The default model's own documented window, not the large fallback,
        // so a fresh configuration compacts before the provider rejects the
        // request.
        let config = Config::default();
        assert_eq!(config.context_window, 0);
        assert_eq!(config.context_window(), 128_000);

        // Longest-prefix matching: `gpt-4.1` and `gpt-4-turbo` do not fall
        // under `gpt-4`.
        for (model, window) in [
            ("gpt-4.1", 1_000_000),
            ("gpt-4-turbo", 128_000),
            ("gpt-4o-mini", 128_000),
            ("deepseek-chat", 128_000),
            ("glm-5.3", 128_000),
        ] {
            let config = Config {
                model: model.to_string(),
                ..Config::default()
            };
            assert_eq!(config.context_window(), window, "{model}");
        }

        // A model the table does not know keeps the 1M fallback, and an
        // explicit value always wins over both.
        let unknown = Config {
            model: "claude-opus-5".to_string(),
            ..Config::default()
        };
        assert_eq!(unknown.context_window(), 1_000_000);
        let explicit = Config {
            context_window: 300_000,
            ..Config::default()
        };
        assert_eq!(explicit.context_window(), 300_000);
    }

    #[test]
    fn context_window_keeps_the_response_cap_as_a_lower_bound() {
        let config = Config {
            context_window: 1_050_000,
            max_tokens: 2_000_000,
            ..Config::default()
        };
        assert_eq!(config.context_window(), 2_000_000);
    }

    #[test]
    fn zero_context_window_uses_the_model_window() {
        let config: Config = serde_json::from_value(serde_json::json!({
            "context_window": 0,
            "model": "gpt-4o-mini",
        }))
        .unwrap();
        assert_eq!(config.context_window(), 128_000);
    }

    #[test]
    fn system_prompt_names_the_local_projects() {
        let root = std::env::temp_dir().join(format!("oxide_ws_prompt_{}", std::process::id()));
        let here = root.join("Projects/site");
        let other = root.join("Projects/api-service");
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let store = root.join("projects.json");
        // Written through serde, as the desktop does: a Windows path's
        // backslashes would not survive a hand-built JSON string.
        let text = serde_json::json!({
            "projects": [{ "path": other.to_string_lossy(), "name": "api-service" }],
        })
        .to_string();
        std::fs::write(&store, text).unwrap();

        let config = Config {
            workspaces: crate::workspaces::Workspaces::load_from(&here, &store),
            ..Config::default()
        };
        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("# Workspaces"), "{prompt}");
        assert!(prompt.contains("This run is in"), "{prompt}");
        assert!(
            prompt.contains(&other.to_string_lossy().to_string()),
            "{prompt}"
        );
        // A machine with nothing else added is still told where it is and to
        // look beside itself rather than scan the home directory.
        let alone = Config {
            workspaces: crate::workspaces::Workspaces::load_from(&here, &root.join("missing.json")),
            ..Config::default()
        };
        let prompt = alone.compose_system_prompt();
        assert!(prompt.contains("# Workspaces"), "{prompt}");
        assert!(prompt.contains("only folder Oxide has"), "{prompt}");
        // A hand-built config has no directory to name.
        assert!(!Config::default()
            .compose_system_prompt()
            .contains("# Workspaces"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn presets_cover_built_in_providers() {
        assert_eq!(
            ProviderPreset::for_name("openai").unwrap().kind,
            ProviderKind::OpenAi
        );
        assert_eq!(
            ProviderPreset::for_name("gpt").unwrap().kind,
            ProviderKind::OpenAi
        );
        assert_eq!(
            ProviderPreset::for_name("deepseek").unwrap().kind,
            ProviderKind::OpenAi
        );
        assert_eq!(
            ProviderPreset::for_name("deepseek").unwrap().base_url,
            "https://api.deepseek.com/v1"
        );
        assert_eq!(
            ProviderPreset::for_name("anthropic").unwrap().kind,
            ProviderKind::Anthropic
        );
        assert_eq!(
            ProviderPreset::for_name("anthropic").unwrap().key_env,
            "ANTHROPIC_API_KEY"
        );
        let portkey = ProviderPreset::for_name("portkey").unwrap();
        assert_eq!(portkey.kind, ProviderKind::OpenAi);
        assert_eq!(portkey.base_url, "https://api.portkey.ai/v1");
        assert_eq!(portkey.model, "claude-sonnet-5");
        assert_eq!(portkey.key_env, "PORTKEY_API_KEY");
        let zai = ProviderPreset::for_name("zai").unwrap();
        assert_eq!(zai.kind, ProviderKind::OpenAi);
        assert_eq!(zai.base_url, "https://api.z.ai/api/paas/v4");
        assert_eq!(zai.model, "glm-5.3");
        assert_eq!(zai.key_env, "ZAI_API_KEY");
        assert_eq!(ProviderPreset::for_name("glm").unwrap().model, "glm-5.3");
        assert!(ProviderPreset::for_name("custom-endpoint").is_none());
    }

    #[test]
    fn zai_runtime_fields_and_fallback_catalog() {
        let mut config = Config {
            provider: "openai".into(),
            model: "gpt-4o-mini".into(),
            base_url: "https://api.openai.com/v1".into(),
            ..Config::default()
        };
        config.apply_provider("glm", "zai-key");
        assert_eq!(config.model, "glm-5.3");
        assert_eq!(config.base_url, "https://api.z.ai/api/paas/v4");
        assert!(config.is_zai());
        assert!(config.supports_reasoning());

        let catalog = config.model_catalog();
        assert!(catalog.contains(&"glm-5.3-flash".to_string()));
        assert!(catalog.contains(&"glm-5.3".to_string()));
        assert!(catalog.windows(2).all(|pair| pair[0] <= pair[1]));

        let explicit = Config {
            model: "glm-custom".into(),
            model_catalog: vec!["glm-custom".into()],
            ..config.clone()
        };
        assert_eq!(explicit.model_catalog(), vec!["glm-custom"]);
    }

    #[test]
    fn glm_forced_thinking_versions() {
        assert!(glm_forces_thinking("glm-5.3"));
        assert!(glm_forces_thinking("glm-5.3-flash"));
        assert!(glm_forces_thinking("GLM-5.4"));
        assert!(!glm_forces_thinking("glm-5.2"));
        assert!(!glm_forces_thinking("glm-4.6"));
        assert!(!glm_forces_thinking("glm-5v-turbo"));
        assert!(!glm_forces_thinking("gpt-5.4"));
    }

    #[test]
    fn custom_providers_pointed_at_zai_behave_like_zai() {
        let by_url = Config {
            provider: "my-endpoint".into(),
            base_url: "https://api.z.ai/api/paas/v4".into(),
            ..Config::default()
        };
        assert!(by_url.is_zai());
        assert!(!by_url.model_catalog().is_empty());

        let by_cn_url = Config {
            base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
            ..by_url
        };
        assert!(by_cn_url.is_zai());

        let other = Config {
            base_url: "https://api.deepseek.com/v1".into(),
            ..Config::default()
        };
        assert!(!other.is_zai());
        assert!(other.model_catalog().is_empty());
    }

    #[test]
    fn reasoning_parses_aliases_and_cycles() {
        assert_eq!(Reasoning::parse("auto"), Some(Reasoning::Auto));
        assert_eq!(Reasoning::parse("OFF"), Some(Reasoning::Off));
        assert_eq!(Reasoning::parse("small"), Some(Reasoning::Low));
        assert_eq!(Reasoning::parse("medium"), Some(Reasoning::Medium));
        assert_eq!(Reasoning::parse("xhigh"), Some(Reasoning::High));
        assert_eq!(Reasoning::parse("nonsense"), None);

        assert_eq!(Reasoning::default(), Reasoning::Auto);
        assert_eq!(Reasoning::Auto.next(), Reasoning::Off);
        assert_eq!(Reasoning::Off.next(), Reasoning::Low);
        assert_eq!(Reasoning::Low.next(), Reasoning::Medium);
        assert_eq!(Reasoning::Medium.next(), Reasoning::High);
        assert_eq!(Reasoning::High.next(), Reasoning::Auto);
    }

    #[test]
    fn detects_adaptive_claude_models() {
        assert!(supports_adaptive_thinking("claude-sonnet-5"));
        assert!(supports_adaptive_thinking("claude-opus-4-8"));
        assert!(supports_adaptive_thinking(
            "us.anthropic.claude-sonnet-4-6-20250929-v1:0"
        ));
        assert!(!supports_adaptive_thinking("claude-haiku-4-5"));
        assert!(!supports_adaptive_thinking("claude-sonnet-4"));
        assert!(!supports_adaptive_thinking("gpt-5.4"));
    }

    #[test]
    fn reasoning_maps_to_provider_controls() {
        assert_eq!(Reasoning::Off.effort(), None);
        assert_eq!(Reasoning::Medium.effort(), Some("medium"));
        assert_eq!(Reasoning::High.budget_tokens(8192), Some(7168));
        assert_eq!(Reasoning::Low.budget_tokens(4096), Some(2048));
        assert_eq!(Reasoning::Off.budget_tokens(8192), None);
        assert_eq!(Reasoning::High.budget_tokens(1024), None);
    }

    #[test]
    fn mcp_prompt_routes_matching_service_content() {
        let mut config = Config::default();
        config.ecosystem.mcp.push(ecosystem::McpServer {
            name: "documents".to_string(),
            enabled: true,
            kind: ecosystem::McpKind::Remote {
                url: "https://docs.example.com/mcp".to_string(),
                headers: Default::default(),
                oauth: None,
            },
            domains: vec!["docs.example.com".to_string()],
        });

        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("Route requests to the matching MCP server"));
        assert!(prompt.contains("call mcp_load for that"));
        assert!(prompt.contains("Never ask the user for an identifier"));
        assert!(prompt.contains("instead of webfetch"));
        assert!(prompt.contains("documents — docs.example.com"));
    }

    #[test]
    fn plugin_prompt_names_what_is_installed_and_what_it_brings() {
        let mut config = Config::default();
        config.ecosystem.plugins.push(ecosystem::PluginSummary {
            name: "docs-toolbox".to_string(),
            description: Some("Writes project docs from source".to_string()),
            version: Some("1.2".to_string()),
            enabled: true,
            skills: 2,
            commands: 1,
            ..Default::default()
        });
        config.ecosystem.plugins.push(ecosystem::PluginSummary {
            name: "notion".to_string(),
            description: Some("Notion pages".to_string()),
            enabled: false,
            ..Default::default()
        });
        config
            .ecosystem
            .hooks
            .push(std::path::PathBuf::from("format.ts"));

        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("# Plugins"));
        assert!(prompt.contains("never tell the user to install a plugin"));
        assert!(prompt.contains(
            "- docs-toolbox v1.2 — Writes project docs from source (1 command, 2 skills)"
        ));
        assert!(prompt.contains("- notion — Notion pages (disabled)"));
        assert!(prompt.contains("`oxide plugin enable <name>`"));
        assert!(prompt.contains("Hook plugins are active (1)"));
    }

    #[test]
    fn plugin_prompt_stays_out_without_plugins_or_hooks() {
        let config = Config::default();
        let prompt = config.compose_system_prompt();
        assert!(!prompt.contains("# Plugins"));
        assert!(!prompt.contains("Hook plugins are active"));
    }

    #[test]
    fn project_hook_plugins_reach_the_prompt() {
        let dir = std::env::temp_dir().join(format!("oxide_plugin_prompt_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join(".oxide/plugins")).unwrap();
        std::fs::write(
            dir.join(".oxide/plugins/format.ts"),
            "export default () => ({})",
        )
        .unwrap();

        let config = Config {
            ecosystem: ecosystem::load(&dir),
            ..Config::default()
        };
        let hooks = config.ecosystem.hooks.len();
        assert!(config
            .ecosystem
            .hooks
            .iter()
            .any(|path| path.ends_with("format.ts")));

        let prompt = config.compose_system_prompt();
        assert!(prompt.contains("# Plugins"), "{prompt}");
        assert!(prompt.contains(&format!("Hook plugins are active ({hooks})")));
        assert!(config.ecosystem.plugins.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn single_stored_provider_only_when_unambiguous() {
        let mut store = AuthStore::default();
        assert!(single_stored_provider(&store).is_none());

        store.set("deepseek", "sk-deepseek");
        let (name, entry) = single_stored_provider(&store).unwrap();
        assert_eq!(name, "deepseek");
        assert_eq!(entry.key, "sk-deepseek");

        store.set("openai", "sk-openai");
        assert!(single_stored_provider(&store).is_none());
    }

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = format!(
            "oxide-config-test-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn apply_provider_remembers_a_model_per_provider() {
        let mut config = Config::default();
        config.apply_provider("deepseek", "sk-deepseek");
        assert_eq!(config.model, "deepseek-chat");

        config.model = "deepseek-v4-pro".to_string();
        config.apply_provider("anthropic", "sk-ant");
        assert_eq!(config.model, "claude-3-5-sonnet-latest");
        assert_eq!(config.provider, "anthropic");

        // Switching back restores the model chosen for DeepSeek instead of
        // leaving the Anthropic model id behind.
        config.apply_provider("deepseek", "sk-deepseek");
        assert_eq!(config.model, "deepseek-v4-pro");
        assert_eq!(config.base_url, "https://api.deepseek.com/v1");
        assert_eq!(config.model_for_provider("openai"), "gpt-4o-mini");
    }

    #[test]
    fn for_provider_points_a_copy_at_another_provider() {
        let config = Config {
            provider: "portkey".into(),
            model: "claude-sonnet-5".into(),
            model_catalog: vec!["catalog-model".into()],
            base_url: "https://api.portkey.ai/v1".into(),
            ..Config::default()
        };

        let deepseek = config.for_provider("deepseek", "sk-deepseek");
        assert_eq!(deepseek.provider, "deepseek");
        assert_eq!(deepseek.api_key, "sk-deepseek");
        assert_eq!(deepseek.model, "deepseek-chat");
        assert_eq!(deepseek.base_url, "https://api.deepseek.com/v1");
        assert!(deepseek.model_catalog.is_empty());

        // The active config is untouched.
        assert_eq!(config.provider, "portkey");
        assert_eq!(config.model, "claude-sonnet-5");
        assert_eq!(config.model_catalog, vec!["catalog-model"]);

        let portkey = config.for_provider("portkey", "pk-test");
        assert_eq!(portkey.model_catalog, vec!["catalog-model"]);
    }

    #[test]
    fn persist_selection_records_the_provider_and_its_models() {
        let dir = temp_dir("persist-selection");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"auto_approve":false}"#).unwrap();

        let mut config = Config::default();
        config.apply_provider("anthropic", "sk-ant");
        config.model = "claude-opus-5".to_string();
        config.persist_selection_at(&path).unwrap();

        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["provider"], "anthropic");
        assert_eq!(stored["model"], "claude-opus-5");
        assert_eq!(stored["provider_models"]["openai"], "gpt-4o-mini");
        assert_eq!(stored["auto_approve"], false);

        let reloaded: Config = serde_json::from_value(stored).unwrap();
        assert_eq!(reloaded.provider_models["openai"], "gpt-4o-mini");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A Portkey Config ID is chosen in the login dialog and lives in
    /// `config.json` beside the provider, so a selection that is persisted
    /// without it would send the next launch to the gateway unrouted.
    #[test]
    fn persist_selection_keeps_the_portkey_config_id() {
        let dir = temp_dir("persist-portkey-config");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"auto_approve":false}"#).unwrap();

        let mut config = Config::default();
        config.apply_provider("portkey", "pk-test");
        config.apply_login_options("gpt-5.4", "", "pc-example");
        config.persist_selection_at(&path).unwrap();

        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["portkey_config"], "pc-example");
        let reloaded: Config = serde_json::from_value(stored).unwrap();
        assert_eq!(reloaded.portkey_config, "pc-example");

        // A provider that is not Portkey leaves the remembered Config ID
        // where it is, since it is Portkey's own setting.
        config.apply_provider("openai", "sk-test");
        config.persist_selection_at(&path).unwrap();
        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["portkey_config"], "pc-example");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_auto_approve_rewrites_only_that_key() {
        let dir = temp_dir("set-auto-approve");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"provider":"zai","model":"glm-5"}"#).unwrap();

        Config::set_auto_approve_at(&path, false).unwrap();

        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["auto_approve"], false);
        assert_eq!(stored["provider"], "zai");
        assert_eq!(stored["model"], "glm-5");

        // A missing or malformed file is recovered from rather than fatal, and
        // the wider config round-trips the written key.
        std::fs::remove_file(&path).unwrap();
        Config::set_auto_approve_at(&path, true).unwrap();
        let reloaded = Config::load(&dir, None, None, None, None).unwrap();
        assert!(reloaded.auto_approve);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn apply_provider_remembers_a_custom_endpoint_per_provider() {
        let mut config = Config {
            provider: "portkey".into(),
            model: "gpt-5.6-sol".into(),
            base_url: "https://gateway.example.com/v1".into(),
            portkey_config: "pc-example".into(),
            ..Config::default()
        };

        config.apply_provider("deepseek", "sk-deepseek");
        assert_eq!(config.base_url, "https://api.deepseek.com/v1");
        assert_eq!(
            config.provider_base_urls["portkey"],
            "https://gateway.example.com/v1"
        );

        config.apply_provider("portkey", "pk-test");
        assert_eq!(config.base_url, "https://gateway.example.com/v1");
        assert_eq!(config.portkey_config, "pc-example");
        assert!(!config.provider_base_urls.contains_key("deepseek"));
    }

    #[test]
    fn apply_provider_restores_a_custom_provider_endpoint() {
        let mut config = Config {
            provider: "my-endpoint".into(),
            model: "custom-model".into(),
            base_url: "https://custom.example/v1".into(),
            ..Config::default()
        };

        config.apply_provider("deepseek", "sk-deepseek");
        assert_eq!(config.base_url, "https://api.deepseek.com/v1");
        assert_eq!(
            config.provider_base_urls["my-endpoint"],
            "https://custom.example/v1"
        );

        config.apply_provider("my-endpoint", "sk-custom");
        assert_eq!(config.base_url, "https://custom.example/v1");
    }

    #[test]
    fn login_options_apply_model_endpoint_and_portkey_config() {
        let mut config = Config::default();
        config.apply_provider("portkey", "pk-test");
        config.apply_login_options(
            "gpt-5.6-sol",
            "https://gateway.example.com/v1/",
            "pc-example",
        );

        assert_eq!(config.model, "gpt-5.6-sol");
        assert_eq!(config.model_for_provider("portkey"), "gpt-5.6-sol");
        assert_eq!(config.base_url, "https://gateway.example.com/v1");
        assert_eq!(
            config.base_url_for_provider("portkey"),
            "https://gateway.example.com/v1"
        );
        assert_eq!(config.portkey_config, "pc-example");

        // Blank values keep the current settings.
        config.apply_login_options("", "", "");
        assert_eq!(config.model, "gpt-5.6-sol");
        assert_eq!(config.base_url, "https://gateway.example.com/v1");
        assert_eq!(config.portkey_config, "pc-example");

        // The Config ID is ignored for other providers, and a preset endpoint
        // is not stored as a custom one.
        config.apply_provider("openai", "sk-openai");
        config.apply_login_options("gpt-4o", "https://api.openai.com/v1", "pc-ignored");
        assert_eq!(config.model, "gpt-4o");
        assert_eq!(config.portkey_config, "pc-example");
        assert!(!config.provider_base_urls.contains_key("openai"));
        assert_eq!(
            config.provider_base_urls["portkey"],
            "https://gateway.example.com/v1"
        );
    }

    #[test]
    fn persist_selection_rewrites_the_active_endpoint() {
        let dir = temp_dir("persist-endpoint");
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{"provider":"portkey","base_url":"https://old.example/v1"}"#,
        )
        .unwrap();

        let mut config = Config {
            provider: "deepseek".into(),
            model: "deepseek-chat".into(),
            base_url: "https://api.deepseek.com/v1".into(),
            provider_base_urls: [(
                "portkey".to_string(),
                "https://gateway.example.com/v1".to_string(),
            )]
            .into_iter()
            .collect(),
            ..Config::default()
        };
        config.persist_selection_at(&path).unwrap();

        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["provider"], "deepseek");
        assert!(stored.get("base_url").is_none());
        assert_eq!(
            stored["provider_base_urls"]["portkey"],
            "https://gateway.example.com/v1"
        );

        config.apply_provider("portkey", "pk-test");
        config.persist_selection_at(&path).unwrap();
        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["base_url"], "https://gateway.example.com/v1");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn apply_provider_updates_runtime_fields() {
        let mut config = Config::default();
        config.apply_provider("deepseek", "sk-test");
        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.model, "deepseek-chat");
        assert_eq!(config.base_url, "https://api.deepseek.com/v1");
    }

    #[test]
    fn apply_portkey_provider_updates_runtime_fields() {
        let mut config = Config::default();
        config.apply_provider("portkey", "pk-test");
        assert_eq!(config.provider, "portkey");
        assert_eq!(config.api_key, "pk-test");
        assert_eq!(config.model, "claude-sonnet-5");
        assert_eq!(config.base_url, "https://api.portkey.ai/v1");
        assert_eq!(config.key_env_name(), "PORTKEY_API_KEY");
        assert!(config.is_portkey());
    }

    #[test]
    fn refreshing_portkey_credentials_preserves_custom_gateway() {
        let mut config = Config {
            provider: "portkey".into(),
            model: "account-model".into(),
            base_url: "https://gateway.example.com/v1".into(),
            portkey_config: "pc-example".into(),
            ..Config::default()
        };
        config.apply_provider("port-key", "pk-new");
        assert_eq!(config.provider, "port-key");
        assert_eq!(config.api_key, "pk-new");
        assert_eq!(config.model, "account-model");
        assert_eq!(config.base_url, "https://gateway.example.com/v1");
        assert_eq!(config.portkey_config, "pc-example");
    }

    #[test]
    fn portkey_model_labels_are_friendly() {
        assert_eq!(model_label("claude-sonnet-5"), "Claude Sonnet 5");
        assert_eq!(model_label("gpt-5.6-terra"), "GPT-5.6 Terra");
        assert_eq!(model_label("deepseek-v4-pro"), "DeepSeek V4 Pro");
        assert_eq!(model_label("glm-5.3-flash"), "GLM-5.3 Flash");
        assert_eq!(model_label("custom-model"), "custom-model");
    }

    #[test]
    fn deepseek_uses_only_models_reported_by_the_provider() {
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-v4-flash".into(),
            ..Config::default()
        };
        let merged =
            config.merge_model_catalog(vec!["deepseek-v4-pro".into(), "deepseek-flash".into()]);
        assert_eq!(merged, vec!["deepseek-flash", "deepseek-v4-pro"]);
    }

    #[test]
    fn merge_model_catalog_adds_active_model_for_unknown_providers() {
        let config = Config {
            provider: "my-endpoint".into(),
            model: "custom-model".into(),
            ..Config::default()
        };
        assert_eq!(
            config.merge_model_catalog(vec!["custom-model".into()]),
            vec!["custom-model"]
        );
    }

    #[test]
    fn apply_provider_leaves_unknown_endpoint_untouched() {
        let mut config = Config::default();
        let model = config.model.clone();
        let base_url = config.base_url.clone();
        config.apply_provider("my-endpoint", "sk-test");
        assert_eq!(config.provider, "my-endpoint");
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.model, model);
        assert_eq!(config.base_url, base_url);
    }

    #[test]
    fn fallback_adopts_sole_stored_credential() {
        let mut config = Config::default();
        let mut store = AuthStore::default();
        store.set("deepseek", "sk-deepseek");
        apply_stored_provider_fallback(&mut config, &store, &None);
        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.api_key, "sk-deepseek");
        assert_eq!(config.model, "deepseek-chat");
        assert_eq!(config.base_url, "https://api.deepseek.com/v1");
    }

    #[test]
    fn fallback_uses_the_model_remembered_for_the_stored_provider() {
        let mut config = Config {
            provider_models: [("deepseek".to_string(), "deepseek-v4-pro".to_string())]
                .into_iter()
                .collect(),
            ..Config::default()
        };
        let mut store = AuthStore::default();
        store.set("deepseek", "sk-deepseek");

        apply_stored_provider_fallback(&mut config, &store, &None);

        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.model, "deepseek-v4-pro");
    }

    #[test]
    fn fallback_adopts_a_remembered_endpoint() {
        let mut config = Config {
            provider_base_urls: [(
                "deepseek".to_string(),
                "https://gateway.example/v1".to_string(),
            )]
            .into_iter()
            .collect(),
            ..Config::default()
        };
        let mut store = AuthStore::default();
        store.set("deepseek", "sk-deepseek");

        apply_stored_provider_fallback(&mut config, &store, &None);

        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.base_url, "https://gateway.example/v1");
    }

    #[test]
    fn fallback_applies_a_custom_providers_remembered_endpoint() {
        let mut config = Config {
            provider_base_urls: [(
                "my-endpoint".to_string(),
                "https://custom.example/v1".to_string(),
            )]
            .into_iter()
            .collect(),
            ..Config::default()
        };
        let mut store = AuthStore::default();
        store.set("my-endpoint", "sk-custom");

        apply_stored_provider_fallback(&mut config, &store, &None);

        assert_eq!(config.provider, "my-endpoint");
        assert_eq!(config.base_url, "https://custom.example/v1");
    }

    #[test]
    fn base_url_for_provider_keeps_the_active_url() {
        let config = Config {
            provider: "portkey".into(),
            base_url: "https://gateway.example.com/v1".into(),
            ..Config::default()
        };

        assert_eq!(
            config.base_url_for_provider("portkey"),
            "https://gateway.example.com/v1"
        );
        assert_eq!(
            config.base_url_for_provider("deepseek"),
            "https://api.deepseek.com/v1"
        );
        assert_eq!(
            config.base_url_for_provider("my-endpoint"),
            "https://gateway.example.com/v1",
            "an unknown provider falls back to the active URL"
        );
    }

    #[test]
    fn fallback_keeps_existing_key() {
        let mut config = Config {
            api_key: "sk-openai".into(),
            ..Config::default()
        };
        let mut store = AuthStore::default();
        store.set("deepseek", "sk-deepseek");
        apply_stored_provider_fallback(&mut config, &store, &None);
        assert_eq!(config.provider, "openai");
        assert_eq!(config.api_key, "sk-openai");
    }

    #[test]
    fn fallback_ignores_multiple_credentials() {
        let mut config = Config::default();
        let mut store = AuthStore::default();
        store.set("deepseek", "sk-deepseek");
        store.set("anthropic", "sk-anthropic");
        apply_stored_provider_fallback(&mut config, &store, &None);
        assert_eq!(config.provider, "openai");
        assert!(config.api_key.is_empty());
    }

    #[test]
    fn fallback_respects_explicit_model_and_base_url() {
        let raw = Some(serde_json::json!({
            "model": "custom-model",
            "base_url": "https://custom.example/v1"
        }));
        let mut config = Config {
            model: "custom-model".into(),
            base_url: "https://custom.example/v1".into(),
            ..Config::default()
        };
        let mut store = AuthStore::default();
        store.set("deepseek", "sk-deepseek");
        apply_stored_provider_fallback(&mut config, &store, &raw);
        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.api_key, "sk-deepseek");
        assert_eq!(config.model, "custom-model");
        assert_eq!(config.base_url, "https://custom.example/v1");
    }

    #[test]
    fn config_parses_provider_only_file() {
        let config: Config = serde_json::from_str(r#"{"provider":"deepseek"}"#).unwrap();
        assert_eq!(config.provider, "deepseek");
        assert!(config.model.is_empty());
        assert!(config.base_url.is_empty());
        assert!(config.api_key.is_empty());
    }

    #[test]
    fn set_active_provider_writes_and_preserves_settings() {
        let dir = temp_dir("active-provider");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"mode":"plan"}"#).unwrap();

        Config::set_active_provider_at(&path, "anthropic").unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["provider"], "anthropic");
        assert_eq!(value["mode"], "plan");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_active_model_writes_and_preserves_settings() {
        let dir = temp_dir("active-model");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"provider":"deepseek","mode":"plan"}"#).unwrap();

        Config::set_active_model_at(&path, "deepseek-reasoner").unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["model"], "deepseek-reasoner");
        assert_eq!(value["provider"], "deepseek");
        assert_eq!(value["mode"], "plan");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_default_model_writes_and_preserves_settings() {
        let dir = temp_dir("default-model");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"provider":"deepseek","mode":"plan"}"#).unwrap();

        Config::set_default_model_at(&path, "deepseek-reasoner").unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["default_model"], "deepseek-reasoner");
        assert_eq!(value["provider"], "deepseek");
        assert_eq!(value["mode"], "plan");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn legacy_lowercase_config_dir_is_migrated() {
        fn temp(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir()
                .join(format!("oxide_config_migrate_{tag}_{}", std::process::id()));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        let base = temp("move");
        std::fs::create_dir_all(base.join("oxide")).unwrap();
        std::fs::write(base.join("oxide/config.json"), "{}").unwrap();
        migrate_legacy_config_dir(&base, &base.join(CONFIG_DIR_NAME));
        assert!(has_dir_entry(&base, CONFIG_DIR_NAME));
        assert!(!has_dir_entry(&base, "oxide"));
        assert!(base.join(CONFIG_DIR_NAME).join("config.json").is_file());
        std::fs::remove_dir_all(&base).ok();

        // An already-branded directory wins and the legacy entry is untouched.
        let base = temp("keep");
        std::fs::create_dir_all(base.join(CONFIG_DIR_NAME)).unwrap();
        migrate_legacy_config_dir(&base, &base.join(CONFIG_DIR_NAME));
        assert!(has_dir_entry(&base, CONFIG_DIR_NAME));
        std::fs::remove_dir_all(&base).ok();
    }

    /// Bedrock signs with AWS credentials and Vertex mints its token from a
    /// service-account file, so neither keeps its credential in this config. A
    /// blank `api_key` is accepted where those exist and refused — naming what
    /// to set — where they do not, before a turn starts rather than after it.
    #[test]
    fn a_credential_outside_the_config_is_read_before_a_run() {
        let bedrock = Config {
            provider: "bedrock".into(),
            ..Config::default()
        };
        let signed = |name: &str| match name {
            "AWS_ACCESS_KEY_ID" => Some("AKIAEXAMPLE".to_string()),
            "AWS_SECRET_ACCESS_KEY" => Some("secret".to_string()),
            _ => None,
        };
        assert_eq!(bedrock.require_api_key_with(&signed).unwrap(), "");
        // Nothing in the environment, and a shared credentials file that is not
        // there — the machine running the test must not decide the answer.
        let absent = |name: &str| {
            (name == "AWS_SHARED_CREDENTIALS_FILE")
                .then(|| format!("/nonexistent-oxide-{}/credentials", std::process::id()))
        };
        let error = bedrock
            .require_api_key_with(&absent)
            .expect_err("nothing to sign with is an error");
        assert!(error.to_string().contains("AWS_ACCESS_KEY_ID"), "{error}");

        // A bearer token is the other way a Bedrock request is authorized, and
        // a stored one rides on the key itself.
        let stored = Config {
            api_key: "bedrock-token".into(),
            ..bedrock
        };
        assert_eq!(
            stored.require_api_key_with(&|_| None).unwrap(),
            "bedrock-token"
        );

        let vertex = Config {
            provider: "vertex".into(),
            ..Config::default()
        };
        let error = vertex
            .require_api_key_with(&|_| None)
            .expect_err("no credential is an error");
        assert!(
            error.to_string().contains("GOOGLE_VERTEX_CREDENTIALS"),
            "{error}"
        );
    }

    /// GitLab Duo is not a host of its own: the stored token is presented to an
    /// instance, which mints the gateway token a turn is authorized with, and
    /// the turn itself goes to the gateway's proxy — OpenAI's wire for an
    /// OpenAI model, Anthropic's for a Claude one.
    #[test]
    fn a_gitlab_turn_is_sent_to_the_gateways_proxy() {
        let _env = crate::env_lock::hold();
        assert_eq!(canonical_provider("duo"), "gitlab");
        assert_eq!(canonical_provider("gitlab-duo"), "gitlab");
        let mut config = Config::default();
        config.apply_provider("gitlab", "glpat-secret");
        // A login leaves no endpoint of its own: the gateway names it.
        assert_eq!(config.base_url, "");
        let gitlab = &config;
        assert_eq!(gitlab.provider_kind(), ProviderKind::OpenAi);
        assert_eq!(gitlab.auth_style(), AuthStyle::Gitlab);
        assert_eq!(
            gitlab.chat_url().unwrap(),
            "https://cloud.gitlab.com/ai/v1/proxy/openai/v1/chat/completions"
        );
        assert_eq!(
            gitlab.models_url().unwrap(),
            "https://cloud.gitlab.com/ai/v1/proxy/openai/v1/models"
        );
        assert_eq!(gitlab.gitlab_instance(), "https://gitlab.com");

        let claude = Config {
            model: "claude-sonnet-4-6".into(),
            ..gitlab.clone()
        };
        assert_eq!(claude.provider_kind(), ProviderKind::Anthropic);
        assert_eq!(
            claude.chat_url().unwrap(),
            "https://cloud.gitlab.com/ai/v1/proxy/anthropic/v1/messages"
        );
        // A Duo subscription lists the provider's own models, so a Claude model
        // is listed by the proxy its turns go through.
        assert_eq!(
            claude.models_url().unwrap(),
            "https://cloud.gitlab.com/ai/v1/proxy/anthropic/v1/models"
        );

        // A self-managed instance points the configuration at its own gateway.
        let self_managed = Config {
            base_url: "https://gitlab.example.com/".into(),
            ..gitlab.clone()
        };
        assert_eq!(
            self_managed.chat_url().unwrap(),
            "https://gitlab.example.com/ai/v1/proxy/openai/v1/chat/completions"
        );
        // A model that is not Claude anywhere else is left alone.
        let openai = Config {
            model: "claude-sonnet-5".into(),
            ..Config::default()
        };
        assert_eq!(openai.provider_kind(), ProviderKind::OpenAi);
    }

    #[test]
    fn kind_and_key_env_resolve_from_config() {
        let anthropic = Config {
            provider: "anthropic".into(),
            ..Config::default()
        };
        assert_eq!(anthropic.provider_kind(), ProviderKind::Anthropic);
        assert_eq!(anthropic.key_env_name(), "ANTHROPIC_API_KEY");

        let custom = Config {
            provider: "my-endpoint".into(),
            ..Config::default()
        };
        assert_eq!(custom.provider_kind(), ProviderKind::OpenAi);
        assert_eq!(custom.key_env_name(), "OPENAI_API_KEY");
    }

    /// The table is the only place a provider is declared, so a name or an
    /// alias that two entries claim would silently shadow one of them in the
    /// picker, in `--provider`, and in the credential store.
    #[test]
    fn every_provider_is_named_and_spelled_once() {
        let mut names = std::collections::HashSet::new();
        for preset in PROVIDERS {
            assert!(
                names.insert(preset.name),
                "{} is declared twice",
                preset.name
            );
            assert_eq!(
                canonical_provider(preset.name),
                preset.name,
                "{} does not resolve to itself",
                preset.name
            );
            assert!(
                !preset.label.trim().is_empty(),
                "{} has no label",
                preset.name
            );
            assert!(
                !preset.description.trim().is_empty(),
                "{} has no description",
                preset.name
            );
            assert!(
                preset.key_url.starts_with("https://"),
                "{} has no key URL",
                preset.name
            );
            for alias in preset.aliases {
                assert_ne!(
                    *alias, preset.name,
                    "{} lists itself as an alias",
                    preset.name
                );
                assert!(names.insert(alias), "{alias} is claimed by two providers");
                let resolved = ProviderPreset::for_name(alias)
                    .unwrap_or_else(|| panic!("{alias} resolves to nothing"));
                assert_eq!(resolved.name, preset.name, "{alias} resolves elsewhere");
            }
        }
        // A local provider is reached without a key, so it is the only kind
        // whose preset may offer a download page instead of a key page.
        for preset in PROVIDERS.iter().filter(|preset| !preset.local) {
            assert!(!preset.key_env.is_empty(), "{} has no key env", preset.name);
        }
    }

    /// A provider's host is how a custom provider (or a gateway) is read for
    /// the dialect it speaks, so a host that does not resolve is a provider
    /// whose models are answered in the wrong format.
    #[test]
    fn every_hosted_provider_is_found_by_its_host() {
        for preset in PROVIDERS
            .iter()
            .filter(|preset| !preset.base_url.is_empty() && !preset.local)
        {
            let found = ProviderPreset::for_base_url(preset.base_url)
                .unwrap_or_else(|| panic!("{} is not found by its host", preset.name));
            assert_eq!(
                found.name, preset.name,
                "{}'s host picks another provider",
                preset.name
            );
        }
        assert!(ProviderPreset::for_base_url("https://example.com/v1").is_none());
        // A local server's host is `localhost` for all of them — they differ by
        // port alone, which is not what a host names — so none is picked by it,
        // and a custom provider pointed there is read as OpenAI-compatible,
        // which is what the local presets speak anyway.
        assert!(ProviderPreset::for_base_url("http://localhost:11434/v1").is_none());
    }
}
