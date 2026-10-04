use crate::config::AuthStyle;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const AUTH_FILE: &str = "auth.json";

/// One provider as the login picker draws it, read from the single provider
/// table in [`crate::config::PROVIDERS`].
#[derive(Debug, Clone, Copy)]
pub struct ProviderOption {
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub key_url: &'static str,
    pub auth: AuthStyle,
    /// Whether the provider needs no credential (a server on this machine).
    pub local: bool,
}

/// Every provider the login picker offers, in the order the provider table
/// declares them. Built once from that table, so a provider is never listed in
/// one place and missing from another.
pub fn known_providers() -> &'static [ProviderOption] {
    static OPTIONS: OnceLock<Vec<ProviderOption>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        crate::config::PROVIDERS
            .iter()
            .map(|preset| ProviderOption {
                name: preset.name,
                label: preset.label,
                description: preset.description,
                key_url: preset.key_url,
                auth: preset.auth,
                local: preset.local,
            })
            .collect()
    })
}

/// One provider as a front-end's picker draws it: the table's own words, plus
/// whether a credential is stored for it and whether it is the one in use. The
/// desktop app, the VS Code panel and `oxide providers --json` all draw this
/// listing rather than reading the provider table themselves.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProviderView {
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Where a key for this provider is issued.
    #[serde(rename = "keyUrl")]
    pub key_url: &'static str,
    /// Whether the provider needs no credential (a server on this machine).
    pub local: bool,
    /// Whether a credential is already stored for it.
    pub stored: bool,
    /// Whether it is the provider `config.json` selects.
    pub active: bool,
}

/// Every provider a picker lists, in the table's own order, each carrying the
/// state a row shows beside it.
pub fn provider_views() -> Vec<ProviderView> {
    provider_views_at(&AuthStore::path(), &crate::config::Config::config_path())
}

fn provider_views_at(auth_path: &Path, config_path: &Path) -> Vec<ProviderView> {
    let store = AuthStore::load_from(auth_path).unwrap_or_default();
    let active = crate::config::Config::active_provider_at(config_path);
    known_providers()
        .iter()
        .map(|option| ProviderView {
            name: option.name,
            label: option.label,
            description: option.description,
            key_url: option.key_url,
            local: option.local,
            stored: store.key(option.name).is_some(),
            active: active.as_deref() == Some(option.name),
        })
        .collect()
}

/// What connecting a provider resolved to, which is what a front-end reports
/// back after a login.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LoginOutcome {
    pub provider: String,
    pub label: String,
    pub model: String,
    /// Whether the provider needs no credential (a server on this machine).
    pub local: bool,
}

/// Connects a provider the way the desktop app's Connect dialog and
/// `oxide login` both do: stores the credential it was given (or reuses the
/// stored one), switches the selection to the provider, and applies the model
/// and endpoint the caller named without disturbing the ones it did not.
pub fn login_provider(
    provider: &str,
    key: &str,
    model: Option<&str>,
    base_url: Option<&str>,
    cwd: &Path,
) -> Result<LoginOutcome> {
    // The selection is read before the credential is stored: resolving a login
    // writes the provider into `config.json`, and a config loaded after that
    // would name the incoming provider as the outgoing one — losing the model
    // and endpoint the previous one is remembered by.
    let mut config = crate::config::Config::load(cwd, None, None, None, None)?;
    let (name, credential) = resolve_login(provider, key)?;
    apply_login_choice(&mut config, &name, &credential, model, base_url);
    config.persist_selection_at(&crate::config::Config::config_path())?;
    Ok(LoginOutcome {
        label: provider_label(&name).to_string(),
        local: is_local(&name),
        provider: name,
        model: config.model,
    })
}

/// The selection a login leaves behind: the provider's own model and endpoint,
/// with whatever the reader named substituted for them. Split out from
/// [`login_provider`] because this is the whole of the decision and making it
/// needs no filesystem.
fn apply_login_choice(
    config: &mut crate::config::Config,
    provider: &str,
    credential: &str,
    model: Option<&str>,
    base_url: Option<&str>,
) {
    config.apply_provider(provider, credential);
    if let Some(model) = non_empty(model) {
        config.model = model.to_string();
    }
    if let Some(url) = non_empty(base_url) {
        config.base_url = url.to_string();
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthEntry {
    #[serde(rename = "type", default = "default_type")]
    pub kind: String,
    pub key: String,
}

fn default_type() -> String {
    "api".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthStore {
    pub entries: BTreeMap<String, AuthEntry>,
}

impl AuthStore {
    pub fn path() -> PathBuf {
        crate::config::config_dir_or_default().join(AUTH_FILE)
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading credentials at {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("parsing credentials at {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)
            .with_context(|| format!("writing credentials to {}", path.display()))?;
        restrict_permissions(path)?;
        Ok(())
    }

    pub fn key(&self, provider: &str) -> Option<&str> {
        self.entries
            .get(&canonical_provider(provider))
            .map(|entry| entry.key.as_str())
    }

    /// Every provider with a stored credential, sorted by name.
    pub fn providers(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    pub fn set(&mut self, provider: &str, key: &str) {
        self.entries.insert(
            canonical_provider(provider),
            AuthEntry {
                kind: default_type(),
                key: key.to_string(),
            },
        );
    }

    pub fn remove(&mut self, provider: &str) -> bool {
        self.entries.remove(&canonical_provider(provider)).is_some()
    }
}

/// The OAuth application a provider's browser login identifies itself as, or
/// `None` for a provider whose login is a key the reader pastes. A provider
/// that mints a short-lived credential from a stored one is the one that needs
/// the flow; GitHub Copilot is the one today, exchanging its GitHub token for a
/// Copilot session token per turn.
pub fn device_flow_client(name: &str) -> Option<&'static str> {
    let name = canonical_provider(name);
    match provider_option(&name).map(|option| option.auth) {
        Some(AuthStyle::Copilot) => Some(crate::llm::copilot::CLIENT_ID),
        _ => None,
    }
}

pub fn connect(provider: &str, key: &str) -> Result<String> {
    connect_with(
        &AuthStore::path(),
        &crate::config::Config::config_path(),
        provider,
        key,
    )
}

fn connect_with(auth_path: &Path, config_path: &Path, provider: &str, key: &str) -> Result<String> {
    let name = canonical_provider(provider);
    let mut store = AuthStore::load_from(auth_path)?;
    store.set(&name, key);
    store.save_to(auth_path)?;
    crate::config::Config::set_active_provider_at(config_path, &name)?;
    Ok(name)
}

/// The providers with a stored credential, or an empty list when `auth.json`
/// is missing or unreadable.
pub fn stored_providers() -> Vec<String> {
    AuthStore::load()
        .map(|store| store.providers())
        .unwrap_or_default()
}

/// Switches to a provider that already has a stored credential, so logging in
/// to a second provider never asks for the key again. Returns the canonical
/// provider name and its key.
pub fn select_stored(provider: &str) -> Result<(String, String)> {
    select_stored_with(
        &AuthStore::path(),
        &crate::config::Config::config_path(),
        provider,
    )
}

fn select_stored_with(
    auth_path: &Path,
    config_path: &Path,
    provider: &str,
) -> Result<(String, String)> {
    let name = canonical_provider(provider);
    let store = AuthStore::load_from(auth_path)?;
    let Some(key) = store.key(&name).map(str::to_string) else {
        anyhow::bail!("no stored credentials for `{name}` — run `/login {name}` to add one");
    };
    crate::config::Config::set_active_provider_at(config_path, &name)?;
    Ok((name, key))
}

pub use crate::config::canonical_provider;

/// What logging in to a provider resolves to: the canonical name, and the
/// credential a request to it carries. A key the reader typed is what gets
/// stored; an empty one reuses what is already stored, or — for a model server
/// on this machine, which asks for nothing — is simply empty, so choosing one is
/// a login rather than a key nobody has.
pub fn resolve_login(provider: &str, typed: &str) -> Result<(String, String)> {
    resolve_login_with(
        &AuthStore::path(),
        &crate::config::Config::config_path(),
        provider,
        typed,
    )
}

fn resolve_login_with(
    auth_path: &Path,
    config_path: &Path,
    provider: &str,
    typed: &str,
) -> Result<(String, String)> {
    if !typed.trim().is_empty() {
        let name = connect_with(auth_path, config_path, provider, typed)?;
        return Ok((name, typed.to_string()));
    }
    if is_local(provider) {
        crate::config::Config::set_active_provider_at(config_path, &canonical_provider(provider))?;
        return Ok((canonical_provider(provider), String::new()));
    }
    select_stored_with(auth_path, config_path, provider)
}

/// Whether a provider is a model server on this machine, which is reached
/// without a credential at all: there is none to store, so logging in to one is
/// choosing it and its endpoint rather than presenting a key.
pub fn is_local(provider: &str) -> bool {
    provider_option(provider).is_some_and(|option| option.local)
}

pub fn provider_option(name: &str) -> Option<&'static ProviderOption> {
    let name = canonical_provider(name);
    known_providers().iter().find(|option| option.name == name)
}

pub fn provider_label(name: &str) -> &str {
    provider_option(name)
        .map(|option| option.label)
        .unwrap_or(name)
}

#[cfg(unix)]
pub(crate) fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting permissions on {}", path.display()))
}

#[cfg(not(unix))]
pub(crate) fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_provider_aliases() {
        assert_eq!(canonical_provider("gpt"), "openai");
        assert_eq!(canonical_provider("GPT-4o"), "openai");
        assert_eq!(canonical_provider("Anthropic"), "anthropic");
        assert_eq!(canonical_provider(" deepseek "), "deepseek");
        assert_eq!(canonical_provider("Port-Key"), "portkey");
        assert_eq!(canonical_provider("GLM"), "zai");
        assert_eq!(canonical_provider(" z.ai "), "zai");
        assert_eq!(canonical_provider("Zhipu"), "zai");
        assert_eq!(canonical_provider("Grok"), "xai");
        assert_eq!(canonical_provider("Gemini"), "google");
        assert_eq!(canonical_provider("Claude"), "anthropic");
        assert_eq!(canonical_provider("Amazon-Bedrock"), "bedrock");
        // A provider nobody declared keeps the name it was configured with.
        assert_eq!(canonical_provider("my-endpoint"), "my-endpoint");
    }

    #[test]
    fn known_providers_cover_every_declared_preset() {
        // The picker lists the provider table itself, so a provider can never
        // be declared without being offered at login.
        assert_eq!(known_providers().len(), crate::config::PROVIDERS.len());
        let names: Vec<&str> = known_providers().iter().map(|option| option.name).collect();
        assert_eq!(
            names,
            crate::config::PROVIDERS
                .iter()
                .map(|preset| preset.name)
                .collect::<Vec<_>>()
        );
        let zai = provider_option("glm").expect("glm resolves to the Z.AI preset");
        assert_eq!(zai.label, "Z.AI");
        assert!(zai.key_url.contains("z.ai"));
        assert!(provider_option("ollama").expect("ollama is offered").local);
        assert_eq!(
            provider_option("copilot").expect("copilot is offered").auth,
            AuthStyle::Copilot
        );
    }

    /// The picker's rows are the provider table plus what is on disk: the same
    /// listing answers for the desktop app, the VS Code panel and the terminal.
    #[test]
    fn provider_views_pair_the_table_with_what_is_connected() {
        let dir = temp_dir("provider-views");
        let auth_path = dir.join("auth.json");
        let config_path = dir.join("config.json");
        let mut store = AuthStore::default();
        store.set("openai", "sk-openai");
        store.save_to(&auth_path).unwrap();
        std::fs::write(&config_path, r#"{"provider":"anthropic"}"#).unwrap();

        let views = provider_views_at(&auth_path, &config_path);
        assert_eq!(views.len(), crate::config::PROVIDERS.len());
        let row = |name: &str| views.iter().find(|view| view.name == name).unwrap();
        assert!(row("openai").stored && !row("openai").active);
        assert!(row("anthropic").active && !row("anthropic").stored);
        assert!(row("ollama").local && !row("ollama").stored);
        // A server on this machine is never held to a key it does not have, and
        // the JSON a front-end reads carries the aliases it matches on.
        let json = serde_json::to_value(row("openai")).unwrap();
        assert_eq!(json["name"], "openai");
        assert_eq!(json["keyUrl"], row("openai").key_url);
        assert_eq!(json["active"], false);

        // An unreadable store still lists every provider rather than nothing.
        std::fs::write(&auth_path, "not json").unwrap();
        assert_eq!(
            provider_views_at(&auth_path, &config_path).len(),
            views.len()
        );
        assert!(!provider_views_at(&auth_path, &config_path)[0].stored);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A login is the reader's own choice over the provider's defaults: the
    /// model and endpoint they named win, and the ones they left alone are the
    /// provider's, never the previous provider's gateway.
    #[test]
    fn a_login_keeps_every_choice_that_was_not_made() {
        let mut config = crate::config::Config {
            provider: "portkey".to_string(),
            model: "gpt-4o".to_string(),
            base_url: "https://gateway.example/v1".to_string(),
            ..Default::default()
        };

        apply_login_choice(&mut config, "openai", "sk-openai", None, None);
        assert_eq!(config.provider, "openai");
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(
            config.provider_base_urls["portkey"],
            "https://gateway.example/v1"
        );

        apply_login_choice(
            &mut config,
            "openai",
            "sk-openai",
            Some(" gpt-5.1 "),
            Some("https://proxy.example/v1"),
        );
        assert_eq!(config.model, "gpt-5.1");
        assert_eq!(config.base_url, "https://proxy.example/v1");

        // An empty string is not a choice, so it cannot blank a remembered one.
        apply_login_choice(&mut config, "openai", "sk-openai", Some("  "), Some(""));
        assert_eq!(config.model, "gpt-5.1");
        assert_eq!(config.base_url, "https://proxy.example/v1");
    }

    #[test]
    fn stores_several_providers_at_once() {
        let mut store = AuthStore::default();
        store.set("openai", "sk-openai");
        store.set("Anthropic", "sk-ant-test");
        store.set("deepseek", "sk-deepseek");

        assert_eq!(store.providers(), vec!["anthropic", "deepseek", "openai"]);
        assert_eq!(store.key("openai"), Some("sk-openai"));
        assert_eq!(store.key("anthropic"), Some("sk-ant-test"));
        assert_eq!(store.key("deepseek"), Some("sk-deepseek"));
    }

    /// What logging in resolves to: a key the reader typed is stored, an empty
    /// one reuses the stored credential, and a server on this machine answers
    /// with the empty key it needs rather than a lookup that would find nothing.
    #[test]
    fn a_local_provider_logs_in_without_a_credential() {
        let dir = temp_dir("local-login");
        let auth_path = dir.join("auth.json");
        let config_path = dir.join("config.json");

        let (name, key) = resolve_login_with(&auth_path, &config_path, "Ollama", "").unwrap();
        assert_eq!(name, "ollama");
        assert!(key.is_empty());
        assert!(!auth_path.exists(), "nothing is stored for a local server");
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(config["provider"], "ollama");

        // A key it was given anyway is still stored, so a proxied server or a
        // gateway fronting one keeps whatever it was handed.
        let (name, key) =
            resolve_login_with(&auth_path, &config_path, "ollama", "sk-local").unwrap();
        assert_eq!((name.as_str(), key.as_str()), ("ollama", "sk-local"));
        assert_eq!(
            AuthStore::load_from(&auth_path).unwrap().key("ollama"),
            Some("sk-local")
        );

        // A provider that does keep a credential still needs one.
        let error = resolve_login_with(&auth_path, &config_path, "portkey", "").unwrap_err();
        assert!(error
            .to_string()
            .contains("no stored credentials for `portkey`"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn select_stored_reuses_a_saved_key() {
        let dir = temp_dir("select");
        let auth_path = dir.join("auth.json");
        let config_path = dir.join("config.json");

        connect_with(&auth_path, &config_path, "openai", "sk-openai").unwrap();
        connect_with(&auth_path, &config_path, "DeepSeek", "sk-deepseek").unwrap();

        let (name, key) = select_stored_with(&auth_path, &config_path, "OPENAI").unwrap();
        assert_eq!(name, "openai");
        assert_eq!(key, "sk-openai");

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(config["provider"], "openai");

        let error = select_stored_with(&auth_path, &config_path, "portkey").unwrap_err();
        assert!(error
            .to_string()
            .contains("no stored credentials for `portkey`"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_key_remove_round_trip() {
        let mut store = AuthStore::default();
        store.set("Anthropic", "sk-ant-test");
        assert_eq!(store.key("anthropic"), Some("sk-ant-test"));
        assert!(store.remove("anthropic"));
        assert_eq!(store.key("anthropic"), None);
        assert!(!store.remove("anthropic"));
    }

    #[test]
    fn serializes_as_provider_map() {
        let mut store = AuthStore::default();
        store.set("openai", "sk-test");
        let value: serde_json::Value = serde_json::to_value(&store).unwrap();
        assert_eq!(value["openai"]["type"], "api");
        assert_eq!(value["openai"]["key"], "sk-test");
    }

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = format!(
            "oxide-auth-test-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn connect_stores_key_and_sets_provider() {
        let dir = temp_dir("connect");
        let auth_path = dir.join("auth.json");
        let config_path = dir.join("config.json");

        let name = connect_with(&auth_path, &config_path, "DeepSeek", "sk-test").unwrap();
        assert_eq!(name, "deepseek");
        assert_eq!(
            AuthStore::load_from(&auth_path).unwrap().key("deepseek"),
            Some("sk-test")
        );

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(config["provider"], "deepseek");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn connect_preserves_existing_config_settings() {
        let dir = temp_dir("preserve");
        let auth_path = dir.join("auth.json");
        let config_path = dir.join("config.json");
        std::fs::write(&config_path, r#"{"auto_approve":false,"mode":"plan"}"#).unwrap();

        connect_with(&auth_path, &config_path, "anthropic", "sk-ant").unwrap();

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(config["provider"], "anthropic");
        assert_eq!(config["auto_approve"], false);
        assert_eq!(config["mode"], "plan");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn connect_recovers_from_invalid_config() {
        let dir = temp_dir("recover");
        let auth_path = dir.join("auth.json");
        let config_path = dir.join("config.json");
        std::fs::write(&config_path, "not json").unwrap();

        connect_with(&auth_path, &config_path, "openai", "sk-openai").unwrap();

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(config["provider"], "openai");

        std::fs::remove_dir_all(&dir).ok();
    }
}
