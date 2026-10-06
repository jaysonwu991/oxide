//! The models pi.dev publishes for a provider, which is where the context
//! window of a model the built-in table does not know comes from.
//!
//! Pi resolves a window from a catalog it fetches from `pi.dev`, so a model
//! released since this binary was built gets its own window there rather than
//! the conservative fallback. Oxide keeps the same answer: the catalog is
//! fetched per provider, revalidated with an ETag and remembered in
//! `model-catalog.json` beside the rest of Oxide's state, so a launch reads the
//! windows from disk and never waits on the network for them. The lookup itself
//! is redone only once the remembered answer is old ([`REFRESH_AFTER_SECS`]),
//! from a background task a front-end starts at launch, so a launch costs at
//! most one request every four hours.
//!
//! `modelCatalog` in a `settings.json` — with `OXIDE_MODEL_CATALOG` overriding
//! it — turns the lookups off, which is what an offline or air-gapped machine
//! wants; the remembered windows are still read, since reading them touches no
//! network.
//!
//! Two oxide processes share the file — a terminal and the app — so a write
//! holds a lock of the file's own, beside it, across the read that merges one
//! provider's answer in and the rename that replaces it.

use crate::config::lookup_context_window;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The base URL the catalogs are published under, which
/// `OXIDE_MODEL_CATALOG_URL` overrides with a mirror of its own.
const BASE_URL: &str = "https://pi.dev";

/// How long a remembered catalog is trusted before a launch looks again. Pi's
/// own interval, so the two agree on how stale a catalog may be.
pub const REFRESH_AFTER_SECS: u64 = 4 * 60 * 60;

/// The file the remembered catalogs live in, beside `config.json`.
pub const FILE_NAME: &str = "model-catalog.json";

/// The `settings.json` key that turns the lookups off.
const SETTING_NAME: &str = "modelCatalog";
const ENV_NAME: &str = "OXIDE_MODEL_CATALOG";
/// The path override, which is how a test (or a caller with its own config
/// directory) redirects the file.
const FILE_ENV: &str = "OXIDE_MODEL_CATALOG_FILE";
const URL_ENV: &str = "OXIDE_MODEL_CATALOG_URL";

/// Short enough that a launch's background task gives up rather than holding a
/// connection for minutes, long enough for a slow network.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Two oxide processes — a terminal and the desktop app — can write the file at
/// the same time, so the read that merges one process's answer into it and the
/// rename that puts it in place are held under a lock of the file's own, beside
/// it, which the other process observes; the mutex is what serializes the
/// threads within one of them.
static STORE_LOCK: Mutex<()> = Mutex::new(());

/// One model as the catalog publishes it. Only the window is kept: it is what
/// the resolution needs, and the rest of the entry changes shape with Pi's
/// catalog rather than with anything Oxide reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    #[serde(default)]
    pub context_window: u64,
}

/// One provider's catalog: the models, the validator the next lookup
/// revalidates with, and when it was last checked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    /// When the catalog was last looked up, in Unix seconds.
    #[serde(default)]
    pub checked_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default)]
    pub models: BTreeMap<String, CatalogModel>,
}

impl CatalogEntry {
    /// Whether it is time to look the catalog up again.
    pub fn is_stale(&self) -> bool {
        now_secs().saturating_sub(self.checked_at) >= REFRESH_AFTER_SECS
    }

    /// The windows alone, which is what a lookup reads.
    pub fn windows(&self) -> Models {
        self.models
            .iter()
            .filter(|(_, model)| model.context_window > 0)
            .map(|(id, model)| (id.clone(), model.context_window))
            .collect()
    }
}

/// The window of each model of one provider, by model id.
pub type Models = BTreeMap<String, u64>;

/// Every remembered catalog, keyed by the pi.dev provider id, which is what a
/// running configuration carries.
pub type ModelsByProvider = BTreeMap<String, BTreeMap<String, u64>>;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    providers: BTreeMap<String, CatalogEntry>,
}

/// The pi.dev provider id whose catalog answers for an Oxide provider, or
/// `None` for a provider pi.dev publishes no catalog for — a gateway that
/// fronts other providers (Portkey), a local runtime (Ollama), and the few
/// hosts whose models are not served there. Every id here is one the service
/// actually answers for.
pub fn provider_id(provider: &str) -> Option<&'static str> {
    let preset = crate::config::ProviderPreset::for_name(provider)?;
    Some(match preset.name {
        "openai" => "openai",
        "anthropic" => "anthropic",
        "deepseek" => "deepseek",
        "google" => "google",
        "zai" => "zai",
        "xai" => "xai",
        "mistral" => "mistral",
        "openrouter" => "openrouter",
        "groq" => "groq",
        "cerebras" => "cerebras",
        "together" => "together",
        "fireworks" => "fireworks",
        "baseten" => "baseten",
        "nvidia" => "nvidia",
        "moonshot" => "moonshotai",
        "minimax" => "minimax",
        "vercel" => "vercel-ai-gateway",
        "huggingface" => "huggingface",
        "vertex" => "google-vertex",
        "bedrock" => "amazon-bedrock",
        "azure" => "azure",
        "github-copilot" => "github-copilot",
        _ => return None,
    })
}

/// The file the remembered catalogs are kept in.
pub fn path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(FILE_ENV) {
        return Some(PathBuf::from(path));
    }
    Some(crate::config::config_dir()?.join(FILE_NAME))
}

/// The windows every remembered catalog holds, keyed by the pi.dev provider id
/// and then by model id. Read at [`crate::config::Config::load`], so a run's
/// window resolution is a lookup rather than a request.
pub fn windows() -> ModelsByProvider {
    let Some(path) = path() else {
        return BTreeMap::new();
    };
    load(&path)
        .providers
        .into_iter()
        .map(|(provider, entry)| (provider, entry.windows()))
        .filter(|(_, models)| !models.is_empty())
        .collect()
}

/// One provider's remembered catalog, whatever its age.
pub fn cached(provider: &str) -> Option<CatalogEntry> {
    let id = provider_id(provider)?;
    read(&path()?, id)
}

/// The window `models` gives `model`, or `None` when the catalog does not hold
/// it.
///
/// The catalog is keyed by the provider's own model ids, so the id itself is
/// tried first. A gateway or a Bedrock region spells the same model another
/// way — `us.anthropic.claude-sonnet-4-5-20250929-v1:0` is
/// `anthropic.claude-sonnet-4-5-20250929-v1:0` in the catalog — so the vendor
/// and region prefixes come off both sides and the table's own longest-prefix
/// rule is laid over the result. That rules a run's shorter spelling of a model
/// the catalog spells out in full onto it as well — the id a run holds may be
/// the name of the model while the catalog holds the name and the release it
/// was dated — since a key that has nothing but the model's own name before a
/// release stamp is that model rather than a longer name that begins with it.
pub fn window_for(models: &Models, model: &str) -> Option<u64> {
    let model = model.trim();
    let exact = models
        .iter()
        .find(|(id, window)| **window > 0 && id.eq_ignore_ascii_case(model));
    if let Some((_, window)) = exact {
        return Some(*window);
    }
    let lowered = model.to_ascii_lowercase();
    let stripped = crate::config::vendorless(&lowered);
    // The same model under another spelling of the prefixes. The longer
    // prefixes come off both sides, so the comparison below is the same one
    // the table makes.
    let spelled = models
        .iter()
        .find(|(id, window)| **window > 0 && crate::config::vendorless(id) == stripped);
    if let Some((_, window)) = spelled {
        return Some(*window);
    }
    // The table's rule, over the keys with their prefixes off: the longest key
    // the requested model still begins with.
    let dated = lookup_context_window(
        models
            .iter()
            .map(|(id, window)| (crate::config::vendorless(id), *window)),
        model,
    );
    if dated.is_some() {
        return dated;
    }
    // And the other way round: a key that carries on from the model's own name
    // with a release, which is what the run's spelling leaves off. The shortest
    // such key is the one that added the least, and it has to be a release
    // rather than another name — `gpt-4o-mini` is not `gpt-4o`.
    models
        .iter()
        .filter(|(id, window)| **window > 0 && dated_from(stripped, crate::config::vendorless(id)))
        .min_by_key(|(id, _)| id.len())
        .map(|(_, window)| *window)
}

/// Whether `key` is `model` with the release it was dated with after it —
/// `-20250929`, `@20250929`, `-0613`. A release starts with a digit, so a name
/// that merely begins with the model (`gpt-4o-mini`, `glm-5.2-highspeed`) is a
/// model of its own and does not answer for it.
fn dated_from(model: &str, key: &str) -> bool {
    let Some((head, rest)) = key.split_at_checked(model.len()) else {
        return false;
    };
    if !head.eq_ignore_ascii_case(model) {
        return false;
    }
    let mut rest = rest.chars();
    matches!(rest.next(), Some('-' | '@' | ':')) && rest.next().is_some_and(|c| c.is_ascii_digit())
}

/// Looks a provider's catalog up now and remembers it. A `304` and a failure
/// both keep the remembered models and the validator, so a lookup that could
/// not do better than last time does not throw the answer away — and every one
/// of them is remembered as checked now, so a machine that cannot reach the
/// service waits out the cooldown instead of asking again at every launch.
pub async fn refresh(provider: &str) -> Result<CatalogEntry> {
    let id = provider_id(provider)
        .with_context(|| format!("pi.dev publishes no model catalog for {provider}"))?;
    let path = path().context("resolving the Oxide config directory")?;
    let stored = read(&path, id);
    // Only revalidate when a remembered body backs the validator, so a `304`
    // can never leave the catalog empty.
    let validator = stored
        .as_ref()
        .filter(|entry| !entry.models.is_empty())
        .and_then(|entry| entry.etag.clone());
    let mut request = client()?
        .get(format!("{}/api/models/providers/{id}", base_url()))
        .header("accept", "application/json");
    if let Some(tag) = validator {
        request = request.header("if-none-match", tag);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            keep_remembered(&path, id, stored.as_ref())?;
            return Err(error).with_context(|| format!("requesting the {id} model catalog"));
        }
    };
    let status = response.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return keep_remembered(&path, id, stored.as_ref());
    }
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::NOT_IMPLEMENTED {
        // The provider is answered for no longer; remembering that keeps a
        // launch from asking again until the answer could have changed.
        return keep_remembered(&path, id, None);
    }
    if !status.is_success() {
        keep_remembered(&path, id, stored.as_ref())?;
        bail!("the {id} model catalog request failed: {status}");
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let value: serde_json::Value = match response.json().await {
        Ok(value) => value,
        Err(error) => {
            keep_remembered(&path, id, stored.as_ref())?;
            return Err(error).with_context(|| format!("reading the {id} model catalog"));
        }
    };
    let entry = CatalogEntry {
        checked_at: now_secs(),
        etag,
        models: parse_catalog(&value),
    };
    write(&path, id, &entry)?;
    Ok(entry)
}

/// Records that the catalog was looked up now, holding on to what `stored`
/// holds — its models and its validator — unless a caller replaces it. A
/// lookup that reached the service and was answered nothing new keeps the
/// models it had; one the provider no longer answers for keeps nothing.
fn keep_remembered(path: &Path, id: &str, stored: Option<&CatalogEntry>) -> Result<CatalogEntry> {
    let entry = CatalogEntry {
        checked_at: now_secs(),
        ..stored.cloned().unwrap_or_default()
    };
    write(path, id, &entry)?;
    Ok(entry)
}

/// Looks a provider's catalog up when the remembered one is old enough to be
/// looked up again, which is what a launch runs in the background. `None` says
/// there was nothing to do: the lookups are off, pi.dev has no catalog for this
/// provider, or the remembered one is still fresh.
pub async fn refresh_if_stale(provider: &str, cwd: Option<&Path>) -> Result<Option<CatalogEntry>> {
    if !enabled_in(cwd) || provider_id(provider).is_none() {
        return Ok(None);
    }
    if cached(provider).is_some_and(|entry| !entry.is_stale()) {
        return Ok(None);
    }
    refresh(provider).await.map(Some)
}

/// The providers a launch's catalog look covers: every provider this machine
/// holds a credential for, plus the one `config.json` selects. The two that
/// sign with a credential the machine already holds — Bedrock and Vertex —
/// store no key here and are the ones a catalog answers for most usefully.
pub fn launch_providers(active: &str) -> Vec<String> {
    let mut providers = crate::auth::stored_providers();
    providers.push(crate::config::canonical_provider(active));
    providers.sort();
    providers.dedup();
    providers
}

/// The look a launch makes in the background: every catalog old enough to be
/// looked up again is refreshed, and the windows are answered only when one of
/// those lookups changed them. `None` covers both halves of the usual case —
/// the lookups are off, or nothing was stale — and a front-end has nothing to
/// repaint for a catalog it already read through
/// [`crate::config::Config::load`].
pub async fn refresh_launch(providers: &[String], cwd: Option<&Path>) -> Option<ModelsByProvider> {
    if !enabled_in(cwd) {
        return None;
    }
    let remembered = windows();
    let mut looked_up = false;
    for provider in providers {
        if matches!(refresh_if_stale(provider, cwd).await, Ok(Some(_))) {
            looked_up = true;
        }
    }
    if !looked_up {
        return None;
    }
    let refreshed = windows();
    (refreshed != remembered).then_some(refreshed)
}

/// Whether a launch should look the catalogs up at all. `modelCatalog` in a
/// `settings.json` decides — the project's own file winning over the global one
/// — and `OXIDE_MODEL_CATALOG` overrides both. On by default.
pub fn enabled(cwd: &Path) -> bool {
    enabled_in(Some(cwd))
}

/// The same flag for a front-end with no run of its own — the desktop app,
/// whose launch is about the app rather than the folder it happens to be in —
/// where the global `settings.json` is the file that decides.
pub fn enabled_in(cwd: Option<&Path>) -> bool {
    if let Some(value) = env_bool(ENV_NAME) {
        return value;
    }
    let mut paths = vec![crate::config::settings_path()];
    if let Some(root) = cwd.and_then(crate::ecosystem::project_root) {
        paths.push(root.join(".oxide").join("settings.json"));
    }
    for path in paths.iter().rev() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(enabled) = value.get(SETTING_NAME).and_then(serde_json::Value::as_bool) {
            return enabled;
        }
    }
    true
}

/// The models of a catalog answer. Pi's catalog serves the models under their
/// ids, but an array and a `{"models": [...]}` envelope are accepted too, since
/// that is what a catalog has served in the past.
fn parse_catalog(value: &serde_json::Value) -> BTreeMap<String, CatalogModel> {
    let entries: Vec<&serde_json::Value> = match value {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(map) => match map.get("models").filter(|_| map.len() == 1) {
            Some(serde_json::Value::Array(items)) => items.iter().collect(),
            _ => map.values().collect(),
        },
        _ => Vec::new(),
    };
    let mut models = BTreeMap::new();
    for entry in entries {
        let Some(object) = entry.as_object() else {
            continue;
        };
        let Some(id) = object
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let window = object
            .get("contextWindow")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        models.insert(
            id.to_string(),
            CatalogModel {
                context_window: window,
            },
        );
    }
    models
}

fn base_url() -> String {
    let base = std::env::var(URL_ENV).unwrap_or_else(|_| BASE_URL.to_string());
    base.trim_end_matches('/').to_string()
}

/// The client the catalogs are fetched with: the same user agent as the release
/// check, with a timeout short enough that a launch's background task gives up.
fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("oxide/{}", env!("CARGO_PKG_VERSION")))
        .timeout(REQUEST_TIMEOUT)
        .build()
        .context("building the http client")
}

fn env_bool(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<bool>().ok())
}

fn read(path: &Path, provider: &str) -> Option<CatalogEntry> {
    let _guard = STORE_LOCK.lock().ok()?;
    load(path).providers.get(provider).cloned()
}

fn write(path: &Path, provider: &str, entry: &CatalogEntry) -> Result<()> {
    if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let _guard = STORE_LOCK
        .lock()
        .ok()
        .context("locking the remembered model catalogs")?;
    // Held across the read below and the rename at the end, so a provider
    // merged in by another oxide process is not written back out without it.
    // A lock that cannot be taken is no reason to fail a lookup: the write is
    // then only what it was before this existed.
    let _file = lock_store(path);
    let mut store = load(path);
    store.providers.insert(provider.to_string(), entry.clone());
    let text = serde_json::to_string_pretty(&store).context("serializing the model catalogs")?;
    // Written through a temporary name, so a reader never sees half a file.
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, text).with_context(|| format!("writing {}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("writing {}", path.display()));
    }
    Ok(())
}

/// Holds the lock of the file beside the store, which is what a second oxide
/// process opening the same store observes. Dropping the handle releases it.
fn lock_store(path: &Path) -> Option<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path(path))
        .ok()?;
    file.lock().ok()?;
    Some(file)
}

/// Where the lock beside a store lives: `model-catalog.json` is locked through
/// `model-catalog.lock`.
fn lock_path(path: &Path) -> PathBuf {
    path.with_extension("lock")
}

fn load(path: &Path) -> Store {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("oxide-model-catalog-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn models(entries: &[(&str, u64)]) -> Models {
        entries
            .iter()
            .map(|(id, window)| (id.to_string(), *window))
            .collect()
    }

    fn entry_models(entries: &[(&str, u64)]) -> BTreeMap<String, CatalogModel> {
        entries
            .iter()
            .map(|(id, window)| {
                (
                    id.to_string(),
                    CatalogModel {
                        context_window: *window,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn a_provider_is_asked_under_the_id_pi_dev_publishes() {
        assert_eq!(provider_id("deepseek"), Some("deepseek"));
        assert_eq!(provider_id("bedrock"), Some("amazon-bedrock"));
        assert_eq!(provider_id("vertex"), Some("google-vertex"));
        assert_eq!(provider_id("moonshot"), Some("moonshotai"));
        assert_eq!(provider_id("vercel"), Some("vercel-ai-gateway"));
        // An alias resolves to the provider it names.
        assert_eq!(provider_id("gpt"), Some("openai"));
        // Nothing is published for a gateway or a local runtime, and an unknown
        // name is not a provider at all.
        assert_eq!(provider_id("portkey"), None);
        assert_eq!(provider_id("ollama"), None);
        assert_eq!(provider_id("lmstudio"), None);
        assert_eq!(provider_id("alibaba"), None);
        assert_eq!(provider_id("not-a-provider"), None);
    }

    #[test]
    fn a_catalog_answer_is_read_whatever_shape_it_arrives_in() {
        let under_ids: serde_json::Value = serde_json::from_str(
            r#"{
                "deepseek-flash": {"id": "deepseek-flash", "contextWindow": 1000000, "maxTokens": 384000},
                "deepseek-v4-pro": {"id": "deepseek-v4-pro", "contextWindow": 1000000}
            }"#,
        )
        .unwrap();
        assert_eq!(
            parse_catalog(&under_ids),
            entry_models(&[
                ("deepseek-flash", 1_000_000),
                ("deepseek-v4-pro", 1_000_000)
            ])
        );

        let as_array: serde_json::Value = serde_json::from_str(
            r#"[{"id": "gpt-5", "contextWindow": 400000}, {"id": "gpt-4", "contextWindow": 8192}]"#,
        )
        .unwrap();
        assert_eq!(
            parse_catalog(&as_array),
            entry_models(&[("gpt-5", 400_000), ("gpt-4", 8_192)])
        );

        let wrapped: serde_json::Value =
            serde_json::from_str(r#"{"models": [{"id": "glm-5.3", "contextWindow": 1000000}]}"#)
                .unwrap();
        assert_eq!(
            parse_catalog(&wrapped),
            entry_models(&[("glm-5.3", 1_000_000)])
        );

        // An entry with no window, no id, or not an object at all contributes
        // nothing rather than a window of zero; an entry with a window and no
        // id is not a model either.
        let partial: serde_json::Value = serde_json::from_str(
            r#"{"a": {"id": "a"}, "b": {"contextWindow": 5}, "c": "text", "d": {"id": "", "contextWindow": 9}}"#,
        )
        .unwrap();
        assert_eq!(parse_catalog(&partial), entry_models(&[("a", 0)]));
        assert_eq!(
            CatalogEntry {
                models: parse_catalog(&partial),
                ..CatalogEntry::default()
            }
            .windows()
            .len(),
            0
        );
    }

    #[test]
    fn a_window_is_found_under_every_spelling_of_the_id() {
        let catalog = models(&[
            ("deepseek-flash", 1_000_000),
            ("us.anthropic.claude-opus-4-6-v1", 200_000),
            ("claude-sonnet-4-5", 1_000_000),
            ("gpt-4.1", 1_047_576),
        ]);

        // The id the catalog is keyed by, whatever its case or padding.
        assert_eq!(window_for(&catalog, "deepseek-flash"), Some(1_000_000));
        assert_eq!(window_for(&catalog, "DeepSeek-Flash"), Some(1_000_000));
        assert_eq!(window_for(&catalog, " deepseek-flash "), Some(1_000_000));

        // The same model under another region: the vendor and region prefixes
        // come off both sides, so a run in another region reads that window.
        assert_eq!(
            window_for(&catalog, "eu.anthropic.claude-opus-4-6-v1"),
            Some(200_000)
        );
        assert_eq!(
            window_for(&catalog, "anthropic.claude-opus-4-6-v1"),
            Some(200_000)
        );
        // Either side in either case: a key may be spelled the way an endpoint
        // serves it, and the id a run holds is whatever the run was given.
        let shouted = models(&[("EU.Anthropic.Claude-Opus-4-6-v1", 200_000)]);
        assert_eq!(
            window_for(&shouted, "anthropic.claude-opus-4-6-v1"),
            Some(200_000)
        );
        assert_eq!(
            window_for(&catalog, "ANTHROPIC.CLAUDE-OPUS-4-6-V1"),
            Some(200_000)
        );

        // A dated id against the catalog's own shorter spelling.
        assert_eq!(
            window_for(&catalog, "claude-sonnet-4-5-20250929"),
            Some(1_000_000)
        );
        assert_eq!(window_for(&catalog, "gpt-4.1-2025-04-14"), Some(1_047_576));

        // A catalog that holds only the region-and-date spelling of a model,
        // asked for under the run's own shorter id: the vendor and region
        // prefixes come off both sides, and a key that carries on from the
        // model's name with its release answers it, so the window is not dropped
        // for want of the region and the date the run never spells.
        let region = models(&[("eu.anthropic.claude-sonnet-4-5-20250929-v1:0", 200_000)]);
        assert_eq!(
            window_for(&region, "anthropic.claude-sonnet-4-5"),
            Some(200_000)
        );
        assert_eq!(window_for(&region, "claude-sonnet-4-5"), Some(200_000));
        assert_eq!(
            window_for(&region, "us.anthropic.claude-sonnet-4-5-20250929-v1:0"),
            Some(200_000)
        );
        // A name that merely begins with the model asked for is a model of its
        // own: neither one is answered by the other's window.
        let neighbours = models(&[("gpt-4o-mini", 100_000), ("glm-5.2-highspeed", 1_000_000)]);
        assert_eq!(window_for(&neighbours, "gpt-4o"), None);
        assert_eq!(window_for(&neighbours, "glm-5.2"), None);

        // A model the catalog does not hold is left to the table behind it.
        assert_eq!(window_for(&catalog, "gpt-4o"), None);
        // A catalog that holds only a window of zero holds nothing.
        assert_eq!(window_for(&models(&[("x", 0)]), "x"), None);
    }

    #[test]
    fn a_remembered_catalog_is_read_back_with_its_validator() {
        let _env = crate::env_lock::hold();
        let dir = temp_dir("store");
        let path = dir.join(FILE_NAME);
        write(
            &path,
            "deepseek",
            &CatalogEntry {
                checked_at: 1_000,
                etag: Some("\"abc\"".into()),
                models: entry_models(&[("deepseek-flash", 1_000_000)]),
            },
        )
        .unwrap();
        // A second provider's catalog is remembered beside the first one.
        write(
            &path,
            "amazon-bedrock",
            &CatalogEntry {
                checked_at: 1_000,
                etag: None,
                models: entry_models(&[("anthropic.claude-sonnet-4-5", 200_000)]),
            },
        )
        .unwrap();

        let entry = read(&path, "deepseek").expect("the remembered catalog");
        assert_eq!(entry.etag.as_deref(), Some("\"abc\""));
        assert_eq!(entry.models["deepseek-flash"].context_window, 1_000_000);
        assert_eq!(entry.windows()["deepseek-flash"], 1_000_000);
        assert_eq!(
            read(&path, "amazon-bedrock").map(|entry| entry.models.len()),
            Some(1)
        );
        // A provider never looked up has no entry rather than an empty one.
        assert_eq!(read(&path, "openai"), None);

        // What `Config::load` reads is the windows alone, by provider id.
        std::env::set_var(FILE_ENV, &path);
        let windows = windows();
        assert_eq!(windows["deepseek"]["deepseek-flash"], 1_000_000);
        assert_eq!(
            windows["amazon-bedrock"]["anthropic.claude-sonnet-4-5"],
            200_000
        );
        assert_eq!(windows.len(), 2);
        std::env::remove_var(FILE_ENV);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unreadable_store_is_no_remembered_catalog() {
        let dir = temp_dir("broken");
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(read(&path, "deepseek"), None);
        // A malformed store is replaced rather than refusing the write.
        write(
            &path,
            "deepseek",
            &CatalogEntry {
                checked_at: 1_000,
                ..CatalogEntry::default()
            },
        )
        .unwrap();
        assert_eq!(
            read(&path, "deepseek").map(|entry| entry.checked_at),
            Some(1_000)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_launch_look_is_off_by_a_setting_or_the_environment() {
        let _env = crate::env_lock::hold();
        let dir = temp_dir("settings");
        let global = dir.join("settings.json");
        let project = dir.join(".oxide").join("settings.json");
        std::env::set_var("OXIDE_SETTINGS_FILE", &global);

        // On by default, with no file to say otherwise.
        assert!(enabled_in(None));

        std::fs::write(&global, r#"{"modelCatalog": false}"#).unwrap();
        assert!(!enabled_in(None));
        // The project's own file wins over the global one, both ways round.
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        std::fs::write(&project, r#"{"modelCatalog": true}"#).unwrap();
        assert!(enabled(&dir));
        std::fs::write(&global, r#"{"modelCatalog": true}"#).unwrap();
        std::fs::write(&project, r#"{"modelCatalog": false}"#).unwrap();
        assert!(!enabled(&dir));

        // The environment overrides both, and a malformed file says nothing.
        std::env::set_var(ENV_NAME, "true");
        assert!(enabled(&dir));
        std::env::set_var(ENV_NAME, "false");
        assert!(!enabled_in(None));
        std::env::remove_var(ENV_NAME);
        std::fs::write(&global, "{not json").unwrap();
        assert!(enabled_in(None));

        std::env::remove_var("OXIDE_SETTINGS_FILE");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_old_catalog_is_looked_up_again() {
        let entry = CatalogEntry {
            checked_at: now_secs(),
            ..CatalogEntry::default()
        };
        assert!(!entry.is_stale());
        assert!(CatalogEntry {
            checked_at: now_secs() - REFRESH_AFTER_SECS,
            ..CatalogEntry::default()
        }
        .is_stale());
        assert!(CatalogEntry::default().is_stale());
    }

    #[tokio::test]
    async fn a_catalog_is_fetched_revalidated_and_remembered() {
        let _env = crate::env_lock::hold();
        let dir = temp_dir("fetch");
        let path = dir.join(FILE_NAME);
        std::env::set_var(FILE_ENV, &path);

        let body = r#"{"deepseek-flash":{"id":"deepseek-flash","contextWindow":1000000}}"#;
        let server = TestServer::start(vec![
            // The first look has no validator to send, so it is answered in
            // full, with the validator to revalidate with next time.
            Response::new(200, body, &[("etag", "\"one\"")], &[]),
            // The second is a revalidation, which the server answers with 304.
            Response::new(304, "", &[], &["if-none-match: \"one\""]),
        ]);
        std::env::set_var(URL_ENV, server.url());

        let entry = refresh("deepseek").await.unwrap();
        assert_eq!(entry.models["deepseek-flash"].context_window, 1_000_000);
        assert_eq!(entry.etag.as_deref(), Some("\"one\""));
        assert_eq!(server.requests()[0].path, "/api/models/providers/deepseek");

        let entry = refresh("deepseek").await.unwrap();
        assert_eq!(
            server.requests()[1].header("if-none-match"),
            Some("\"one\"")
        );
        // A 304 keeps the models it revalidated, and only moves the freshness.
        assert_eq!(entry.models["deepseek-flash"].context_window, 1_000_000);
        assert_eq!(entry.etag.as_deref(), Some("\"one\""));
        assert!(!entry.is_stale());

        std::env::remove_var(URL_ENV);
        std::env::remove_var(FILE_ENV);
        server.stop();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_catalog_that_is_gone_is_remembered_as_gone() {
        let _env = crate::env_lock::hold();
        let dir = temp_dir("missing");
        let path = dir.join(FILE_NAME);
        std::env::set_var(FILE_ENV, &path);

        let server = TestServer::start(vec![Response::new(404, "", &[], &[])]);
        std::env::set_var(URL_ENV, server.url());

        let entry = refresh("openai").await.unwrap();
        assert!(entry.models.is_empty());
        // Remembering the answer is what keeps a launch from asking again.
        assert!(!entry.is_stale());
        assert!(refresh_if_stale("openai", None).await.unwrap().is_none());

        std::env::remove_var(URL_ENV);
        std::env::remove_var(FILE_ENV);
        server.stop();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_failed_look_keeps_the_catalog_it_could_not_replace() {
        let _env = crate::env_lock::hold();
        let dir = temp_dir("failure");
        let path = dir.join(FILE_NAME);
        std::env::set_var(FILE_ENV, &path);
        write(
            &path,
            "openai",
            &CatalogEntry {
                checked_at: 0,
                etag: Some("\"one\"".into()),
                models: entry_models(&[("gpt-5", 400_000)]),
            },
        )
        .unwrap();

        let server = TestServer::start(vec![Response::new(500, "", &[], &[])]);
        std::env::set_var(URL_ENV, server.url());

        assert!(refresh("openai").await.is_err());
        let kept = read(&path, "openai").unwrap();
        assert_eq!(kept.models["gpt-5"].context_window, 400_000);
        assert_eq!(kept.etag.as_deref(), Some("\"one\""));
        // The failure moved the freshness window, so a launch does not retry a
        // broken lookup over and over.
        assert!(!kept.is_stale());

        std::env::remove_var(URL_ENV);
        std::env::remove_var(FILE_ENV);
        server.stop();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_look_that_could_not_reach_the_service_waits_out_the_cooldown() {
        let _env = crate::env_lock::hold();
        let dir = temp_dir("unreachable");
        let path = dir.join(FILE_NAME);
        std::env::set_var(FILE_ENV, &path);
        write(
            &path,
            "openai",
            &CatalogEntry {
                checked_at: 0,
                etag: Some("\"one\"".into()),
                models: entry_models(&[("gpt-5", 400_000)]),
            },
        )
        .unwrap();
        // A port nothing answers on: the failure is in reaching the service
        // rather than in what it said, which is the one a launch meets offline.
        std::env::set_var(URL_ENV, "http://127.0.0.1:1");

        assert!(refresh("openai").await.is_err());
        let kept = read(&path, "openai").unwrap();
        assert_eq!(kept.models["gpt-5"].context_window, 400_000);
        assert_eq!(kept.etag.as_deref(), Some("\"one\""));
        // The look that never landed moved the freshness window, so the next
        // launch waits instead of asking again — and nothing answers there, so
        // a lookup that went out would fail rather than answer nothing.
        assert!(!kept.is_stale());
        assert!(refresh_if_stale("openai", None).await.unwrap().is_none());

        std::env::remove_var(URL_ENV);
        std::env::remove_var(FILE_ENV);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_first_look_that_failed_is_remembered_as_checked() {
        let _env = crate::env_lock::hold();
        let dir = temp_dir("unreachable-first");
        let path = dir.join(FILE_NAME);
        std::env::set_var(FILE_ENV, &path);
        std::env::set_var(URL_ENV, "http://127.0.0.1:1");

        // Nothing remembered to keep: the check is what is remembered, so a
        // machine that cannot reach the service is not asked again every launch
        // for a window it never had.
        assert!(refresh("deepseek").await.is_err());
        let checked = read(&path, "deepseek").expect("the check is remembered");
        assert!(checked.models.is_empty());
        assert!(checked.etag.is_none());
        assert!(!checked.is_stale());
        assert!(refresh_if_stale("deepseek", None).await.unwrap().is_none());

        // A service that answered nonsense is the same: what it said cannot
        // replace what was remembered, and the check still happened.
        let server = TestServer::start(vec![Response::new(200, "not json", &[], &[])]);
        std::env::set_var(URL_ENV, server.url());
        assert!(refresh("deepseek").await.is_err());
        let kept = read(&path, "deepseek").unwrap();
        assert!(kept.models.is_empty());
        assert!(!kept.is_stale());

        std::env::remove_var(URL_ENV);
        std::env::remove_var(FILE_ENV);
        server.stop();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_write_waits_for_the_lock_beside_the_store() {
        let _env = crate::env_lock::hold();
        let dir = temp_dir("shared-store");
        let path = dir.join(FILE_NAME);
        write(
            &path,
            "deepseek",
            &CatalogEntry {
                checked_at: 1_000,
                etag: None,
                models: entry_models(&[("deepseek-flash", 1_000_000)]),
            },
        )
        .unwrap();

        // Two oxide processes — a terminal and the app — write this file, each
        // answering for its own provider. What a second process holds is the
        // lock of the file's own, so a write here waits for it rather than
        // reading a store the other one is about to replace:
        let held = lock_store(&path).expect("the lock beside the store is taken");
        let writer = {
            let path = path.clone();
            std::thread::spawn(move || {
                write(
                    &path,
                    "openai",
                    &CatalogEntry {
                        checked_at: 2_000,
                        etag: None,
                        models: entry_models(&[("gpt-5", 400_000)]),
                    },
                )
                .unwrap();
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            !writer.is_finished(),
            "the write waited for the lock the other process held"
        );
        drop(held);
        writer.join().unwrap();

        // The provider the waiting write did not name is still there, and the
        // store is still one JSON object rather than what the two writers made
        // of it together.
        assert_eq!(
            read(&path, "deepseek").unwrap().models["deepseek-flash"].context_window,
            1_000_000
        );
        assert_eq!(
            read(&path, "openai").unwrap().models["gpt-5"].context_window,
            400_000
        );
        assert!(path.with_extension("lock").exists());
        assert!(serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(&path).unwrap()
        )
        .is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_fresh_catalog_is_not_looked_up_again() {
        let _env = crate::env_lock::hold();
        let dir = temp_dir("fresh");
        let path = dir.join(FILE_NAME);
        std::env::set_var(FILE_ENV, &path);
        write(
            &path,
            "deepseek",
            &CatalogEntry {
                checked_at: now_secs(),
                etag: Some("\"one\"".into()),
                models: entry_models(&[("deepseek-flash", 1_000_000)]),
            },
        )
        .unwrap();
        // A base URL nothing answers on: a lookup that went out would fail
        // rather than answer, so a `None` here is the lookup not being made.
        std::env::set_var(URL_ENV, "http://127.0.0.1:1");

        assert!(refresh_if_stale("deepseek", None).await.unwrap().is_none());
        // A provider with no catalog is not looked up either, and neither is
        // one the environment turned the lookups off for.
        assert!(refresh_if_stale("portkey", None).await.unwrap().is_none());
        std::env::set_var(ENV_NAME, "false");
        assert!(refresh_if_stale("openai", None).await.unwrap().is_none());
        std::env::remove_var(ENV_NAME);

        std::env::remove_var(URL_ENV);
        std::env::remove_var(FILE_ENV);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_launch_looks_a_stale_catalog_up_and_answers_the_windows_it_changed() {
        let _env = crate::env_lock::hold();
        let dir = temp_dir("launch");
        let path = dir.join(FILE_NAME);
        std::env::set_var(FILE_ENV, &path);
        let stale = |checked_at: u64| CatalogEntry {
            checked_at,
            models: entry_models(&[("deepseek-flash", 1_000_000)]),
            ..CatalogEntry::default()
        };
        // A base URL nothing answers on: a lookup that went out would fail
        // rather than answer, so a `None` here is the lookup not being made.
        std::env::set_var(URL_ENV, "http://127.0.0.1:1");

        // Nothing stale, and a provider pi.dev has no catalog for: neither is a
        // request, and neither is anything for a front-end to repaint.
        write(&path, "deepseek", &stale(now_secs())).unwrap();
        assert!(refresh_launch(&["deepseek".to_string()], None)
            .await
            .is_none());
        assert!(refresh_launch(&["portkey".to_string()], None)
            .await
            .is_none());

        let body = r#"{"deepseek-flash":{"id":"deepseek-flash","contextWindow":1000000},"deepseek-v4-pro":{"id":"deepseek-v4-pro","contextWindow":2000000}}"#;
        let both = entry_models(&[
            ("deepseek-flash", 1_000_000),
            ("deepseek-v4-pro", 2_000_000),
        ]);
        // One response per lookup this test expects to go out, so a request no
        // response is left for fails rather than passing as no answer at all.
        let server = TestServer::start(vec![
            Response::new(200, body, &[], &[]),
            Response::new(200, body, &[], &[]),
        ]);
        std::env::set_var(URL_ENV, server.url());

        // An old catalog is looked up, and the windows that came back are the
        // ones a front-end repaints with.
        write(&path, "deepseek", &stale(0)).unwrap();
        let refreshed = refresh_launch(&["deepseek".to_string()], None)
            .await
            .expect("the windows the lookup changed");
        assert_eq!(refreshed["deepseek"]["deepseek-flash"], 1_000_000);
        assert_eq!(refreshed["deepseek"]["deepseek-v4-pro"], 2_000_000);
        assert_eq!(server.requests()[0].path, "/api/models/providers/deepseek");

        // A look that found the windows it already held answers nothing: the
        // run resolved them from disk at load, so there is nothing to repaint.
        write(
            &path,
            "deepseek",
            &CatalogEntry {
                checked_at: 0,
                models: both,
                ..CatalogEntry::default()
            },
        )
        .unwrap();
        assert!(refresh_launch(&["deepseek".to_string()], None)
            .await
            .is_none());
        assert_eq!(server.requests().len(), 2);

        // The lookups off is no request at all, stale catalog or not.
        std::env::set_var(ENV_NAME, "false");
        write(&path, "deepseek", &stale(0)).unwrap();
        assert!(refresh_launch(&["deepseek".to_string()], None)
            .await
            .is_none());
        std::env::remove_var(ENV_NAME);

        std::env::remove_var(URL_ENV);
        std::env::remove_var(FILE_ENV);
        server.stop();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_launch_looks_up_the_provider_in_use_and_every_stored_one() {
        // The active provider is canonicalized, and the list is ordered and
        // free of repeats, since one lookup runs per provider.
        let providers = launch_providers("gpt");
        assert!(providers.contains(&"openai".to_string()));
        assert!(!providers.contains(&"gpt".to_string()));
        let mut expected = providers.clone();
        expected.sort();
        expected.dedup();
        assert_eq!(providers, expected);
        // A name that is no provider at all is looked up as it stands; pi.dev
        // answers with no catalog, which is nothing to do rather than an error.
        assert!(launch_providers("not-a-provider").contains(&"not-a-provider".to_string()));
    }

    /// A response handed to a [`TestServer`], in the order the test expects the
    /// catalog to be asked for it. `requires` names headers the request must
    /// carry — a request that does not is answered `400`, so a test asserting a
    /// revalidation cannot pass on a request that sent no validator.
    struct Response {
        status: u16,
        body: String,
        headers: Vec<(String, String)>,
        requires: Vec<String>,
    }

    impl Response {
        fn new(status: u16, body: &str, headers: &[(&str, &str)], requires: &[&str]) -> Self {
            Self {
                status,
                body: body.to_string(),
                headers: headers
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
                requires: requires.iter().map(|header| header.to_string()).collect(),
            }
        }

        fn write(&self, stream: &mut TcpStream) {
            let mut text = format!(
                "HTTP/1.1 {} {}\r\ncontent-length: {}\r\nconnection: close\r\n",
                self.status,
                reason(self.status),
                self.body.len()
            );
            for (name, value) in &self.headers {
                text.push_str(&format!("{name}: {value}\r\n"));
            }
            text.push_str("\r\n");
            text.push_str(&self.body);
            let _ = stream.write_all(text.as_bytes());
        }
    }

    fn reason(status: u16) -> &'static str {
        match status {
            200 => "OK",
            304 => "Not Modified",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "Bad Request",
        }
    }

    #[test]
    fn a_request_that_lands_late_is_read_rather_than_hung_up_on() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().unwrap().port();
        let writer = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            std::thread::sleep(Duration::from_millis(50));
            stream.write_all(b"GET /late HTTP/1.1\r\n\r\n").unwrap();
            std::thread::sleep(Duration::from_millis(50));
        });
        let (stream, _) = listener.accept().unwrap();
        // The connection a platform hands out this way is one the first read
        // finds nothing on, which is what the reader has to wait out.
        stream.set_nonblocking(true).unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        assert_eq!(read_line(&mut reader, &mut line).unwrap(), 20);
        assert_eq!(line, "GET /late HTTP/1.1\r\n");
        writer.join().unwrap();
    }

    /// One request the server answered, for a test to assert on.
    #[derive(Debug, Clone)]
    struct Request {
        path: String,
        headers: Vec<String>,
    }

    impl Request {
        fn header(&self, name: &str) -> Option<&str> {
            let prefix = format!("{name}: ");
            self.headers
                .iter()
                .find(|header| header.starts_with(&prefix))
                .map(|header| header[prefix.len()..].trim())
        }
    }

    /// A one-request-at-a-time HTTP server the fetch tests answer from, since
    /// no dependency of this crate serves a canned response.
    struct TestServer {
        port: u16,
        requests: Arc<Mutex<Vec<Request>>>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl TestServer {
        fn start(responses: Vec<Response>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
            let port = listener.local_addr().unwrap().port();
            listener.set_nonblocking(true).unwrap();
            let queued = Arc::new(Mutex::new(VecDeque::from(responses)));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let thread = std::thread::spawn({
                let (queued, requests, stop) = (
                    Arc::clone(&queued),
                    Arc::clone(&requests),
                    Arc::clone(&stop),
                );
                move || loop {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    match listener.accept() {
                        Ok((stream, _)) => answer(stream, &queued, &requests),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                port,
                requests,
                stop,
                thread: Some(thread),
            }
        }

        fn url(&self) -> String {
            format!("http://127.0.0.1:{}", self.port)
        }

        fn requests(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }

        fn stop(mut self) {
            self.stop.store(true, Ordering::SeqCst);
            // Wake the accept loop so it notices.
            let _ = TcpStream::connect(("127.0.0.1", self.port));
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// Read one line, waiting out a socket that is not ready yet rather than
    /// hanging up on the client: a platform whose `accept` hands out a
    /// non-blocking connection answers `WouldBlock` until the request lands.
    fn read_line(reader: &mut BufReader<TcpStream>, line: &mut String) -> std::io::Result<usize> {
        loop {
            match reader.read_line(line) {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                other => return other,
            }
        }
    }

    fn answer(
        stream: TcpStream,
        queued: &Mutex<VecDeque<Response>>,
        requests: &Mutex<Vec<Request>>,
    ) {
        // A connection an accept took from a listening socket set non-blocking
        // can be one a read finds nothing on yet (macOS hands them out that way,
        // Linux does not), which would hang up on the client mid-request.
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        if read_line(&mut reader, &mut request_line).is_err() {
            return;
        }
        let path = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_string();
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            match read_line(&mut reader, &mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    headers.push(line.to_ascii_lowercase());
                }
                Err(_) => return,
            }
        }
        requests.lock().unwrap().push(Request {
            path,
            headers: headers.clone(),
        });
        let mut stream = stream;
        let response = queued.lock().unwrap().pop_front();
        match response {
            Some(response)
                if response.requires.iter().all(|required| {
                    let required = required.to_ascii_lowercase();
                    headers.iter().any(|header| header == &required)
                }) =>
            {
                response.write(&mut stream)
            }
            Some(_) => {
                let _ = stream.write_all(
                    b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                );
            }
            None => {
                let _ = stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            }
        }
        // Finish the response before the socket goes, and take anything the
        // client left behind with it, since an orderly close is what keeps a
        // reader from losing the body to a reset.
        let _ = stream.flush();
        let _ = stream.shutdown(std::net::Shutdown::Write);
        let mut rest = [0u8; 64];
        let _ = reader.read(&mut rest);
    }
}
