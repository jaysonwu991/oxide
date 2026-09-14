use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

const AUTH_FILE: &str = "auth.json";
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderOption {
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub key_url: &'static str,
}

pub(crate) const KNOWN_PROVIDERS: [ProviderOption; 3] = [
    ProviderOption {
        name: "openai",
        label: "OpenAI",
        description: "GPT models",
        key_url: "https://platform.openai.com/api-keys",
    },
    ProviderOption {
        name: "deepseek",
        label: "DeepSeek",
        description: "DeepSeek chat and reasoning models",
        key_url: "https://platform.deepseek.com/api_keys",
    },
    ProviderOption {
        name: "anthropic",
        label: "Anthropic",
        description: "Claude models",
        key_url: "https://console.anthropic.com/settings/keys",
    },
];

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
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("oxide")
            .join(AUTH_FILE)
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

pub fn login(provider: Option<String>, key: Option<String>) -> Result<()> {
    let interactive = io::stdin().is_terminal();
    if interactive {
        println!("Oxide provider setup\n");
    }
    let provider = match provider {
        Some(provider) => provider,
        None => prompt_provider()?,
    };
    let name = canonical_provider(&provider);
    let key = match key {
        Some(key) => key,
        None => {
            if interactive {
                if let Some(option) = provider_option(&name) {
                    println!("Create or copy a key: {}", option.key_url);
                }
                println!("Your key is hidden while you type or paste it.");
            }
            read_secret(&format!("API key for {}: ", provider_label(&name)))?
        }
    };
    let key = key.trim().to_string();
    if key.is_empty() {
        anyhow::bail!("no API key provided");
    }
    connect(&name, &key)?;
    println!("Connected to {}.", provider_label(&name));
    println!(
        "Credentials saved securely at {}",
        AuthStore::path().display()
    );
    println!("Next: run `oxide` to start coding.");
    Ok(())
}

/// Stores a credential and makes it the active provider, returning the
/// canonical provider name.
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

pub fn list() -> Result<()> {
    let store = AuthStore::load()?;
    if store.entries.is_empty() {
        println!("no credentials stored ({})", AuthStore::path().display());
        return Ok(());
    }
    println!("stored credentials:");
    for (name, entry) in &store.entries {
        println!("  {name} ({}) key {}", entry.kind, mask(&entry.key));
    }
    Ok(())
}

pub fn logout(provider: Option<String>) -> Result<()> {
    let mut store = AuthStore::load()?;
    let provider = match provider {
        Some(provider) => provider,
        None => prompt_provider()?,
    };
    let name = canonical_provider(&provider);
    if store.remove(&name) {
        store.save()?;
        println!("removed credentials for {name}");
    } else {
        println!("no credentials stored for {name}");
    }
    Ok(())
}

pub fn canonical_provider(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "gpt" | "gpt-4" | "gpt-4o" => "openai".to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn provider_option(name: &str) -> Option<&'static ProviderOption> {
    let name = canonical_provider(name);
    KNOWN_PROVIDERS.iter().find(|option| option.name == name)
}

pub(crate) fn provider_label(name: &str) -> &str {
    provider_option(name)
        .map(|option| option.label)
        .unwrap_or(name)
}

fn prompt_provider() -> Result<String> {
    println!("Choose a provider:");
    for (index, option) in KNOWN_PROVIDERS.iter().enumerate() {
        println!(
            "  {}. {:<10} {}",
            index + 1,
            option.label,
            option.description
        );
    }
    println!("  Or type the name of a custom OpenAI-compatible provider.");
    print!("Provider [1]: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("reading provider")?;
    let line = line.trim();
    if let Ok(index) = line.parse::<usize>() {
        if (1..=KNOWN_PROVIDERS.len()).contains(&index) {
            return Ok(KNOWN_PROVIDERS[index - 1].name.to_string());
        }
    }
    if line.is_empty() {
        return Ok(KNOWN_PROVIDERS[0].name.to_string());
    }
    Ok(line.to_string())
}

fn read_secret(prompt: &str) -> Result<String> {
    if !io::stdin().is_terminal() {
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .context("reading API key")?;
        return Ok(line.trim().to_string());
    }
    read_secret_interactive(prompt)
}

/// Reads a secret from the terminal in raw mode. Raw mode keeps pasted text
/// (including bracketed paste) working while the input stays hidden, unlike
/// `stty -echo` which drops paste events in some terminals.
fn read_secret_interactive(prompt: &str) -> Result<String> {
    use crossterm::cursor::{Hide, RestorePosition, SavePosition, Show};
    use crossterm::event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    };
    use crossterm::execute;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size, Clear, ClearType};

    enable_raw_mode().context("enabling terminal input")?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnableBracketedPaste, Hide, SavePosition) {
        let _ = disable_raw_mode();
        return Err(error).context("preparing secure API key input");
    }

    let mut secret = String::new();
    let result = (|| -> Result<()> {
        draw_secret_box(&mut stdout, prompt, 0, size().ok().map(|(width, _)| width))?;
        loop {
            match event::read() {
                Ok(Event::Key(key)) => {
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    match key.code {
                        KeyCode::Enter => break Ok(()),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break Err(anyhow::anyhow!("cancelled"));
                        }
                        KeyCode::Backspace => {
                            secret.pop();
                            draw_secret_box(
                                &mut stdout,
                                prompt,
                                secret.chars().count(),
                                size().ok().map(|(width, _)| width),
                            )?;
                        }
                        KeyCode::Char(c)
                            if !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                        {
                            secret.push(c);
                            draw_secret_box(
                                &mut stdout,
                                prompt,
                                secret.chars().count(),
                                size().ok().map(|(width, _)| width),
                            )?;
                        }
                        _ => {}
                    }
                }
                Ok(Event::Paste(text)) => {
                    secret.push_str(&text);
                    draw_secret_box(
                        &mut stdout,
                        prompt,
                        secret.chars().count(),
                        size().ok().map(|(width, _)| width),
                    )?;
                }
                Ok(Event::Resize(width, _)) => {
                    draw_secret_box(&mut stdout, prompt, secret.chars().count(), Some(width))?;
                }
                Ok(_) => {}
                Err(error) => break Err(anyhow::anyhow!("reading API key: {error}")),
            }
        }
    })();

    let _ = execute!(
        stdout,
        RestorePosition,
        Clear(ClearType::FromCursorDown),
        DisableBracketedPaste,
        Show
    );
    disable_raw_mode().context("restoring terminal input")?;

    result?;
    Ok(secret.trim().to_string())
}

fn draw_secret_box(
    output: &mut impl Write,
    prompt: &str,
    secret_len: usize,
    terminal_width: Option<u16>,
) -> Result<()> {
    use crossterm::cursor::RestorePosition;
    use crossterm::execute;
    use crossterm::terminal::{Clear, ClearType};

    let width = terminal_width.unwrap_or(60).clamp(24, 72) as usize;
    let box_width = width.saturating_sub(2);
    let visible = secret_len.min(box_width.saturating_sub(2));
    let hidden = "*".repeat(visible);
    let padding = " ".repeat(box_width.saturating_sub(visible + 2));
    let overflow = if secret_len > visible { "…" } else { " " };

    execute!(output, RestorePosition, Clear(ClearType::FromCursorDown))?;
    writeln!(output, "{prompt}")?;
    writeln!(output, "┌{}┐", "─".repeat(box_width))?;
    writeln!(output, "│ {hidden}{padding}{overflow}│")?;
    writeln!(output, "└{}┘", "─".repeat(box_width))?;
    write!(
        output,
        "Paste or type your key · Enter to connect · Ctrl+C to cancel"
    )?;
    output.flush()?;
    Ok(())
}

fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        "****".to_string()
    } else {
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("****{tail}")
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
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

    #[test]
    fn masks_keys() {
        assert_eq!(mask("short"), "****");
        assert_eq!(mask("sk-abcdefghijkl"), "****ijkl");
    }

    #[test]
    fn secret_box_shows_one_star_per_character() {
        let mut output = Vec::new();
        draw_secret_box(&mut output, "API key:", 7, Some(40)).unwrap();
        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("*******"));
        assert!(!rendered.contains("********"));
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
