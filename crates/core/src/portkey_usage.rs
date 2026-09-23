//! The Portkey spend status bar.
//!
//! `/usage` configures it; the settings live in `portkey-usage.json` in the
//! Oxide config directory (mode 0600, since they hold an API key). The bar reads
//! the spend of the logged-in Portkey account, so it only runs while the active
//! provider is Portkey with a usable credential. Today's and the month's spend
//! come from the Portkey analytics API (`GET <base_url>/analytics/graphs/cost`)
//! filtered by the configured user metadata, the session column comes from the
//! session's own totals, and the budget is formatted in the configured currency.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::Config;

const CONFIG_FILE: &str = "portkey-usage.json";
const DEFAULT_METADATA_KEY: &str = "_user";
const DEFAULT_BASE_URL: &str = "https://api.portkey.ai/v1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_ERROR_CHARS: usize = 160;

/// The currency a budget is denominated in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Currency {
    #[default]
    Usd,
    Cny,
}

impl Currency {
    pub fn symbol(self) -> &'static str {
        match self {
            Currency::Usd => "$",
            Currency::Cny => "¥",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Currency::Usd => "usd",
            Currency::Cny => "cny",
        }
    }

    /// Parses a `/usage currency` argument (`$`, `¥`, `usd`, `cny`, `rmb`).
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "$" | "usd" | "dollar" | "dollars" => Some(Currency::Usd),
            "¥" | "￥" | "cny" | "rmb" | "yuan" => Some(Currency::Cny),
            _ => None,
        }
    }

    /// Formats an amount in this currency (`$600.00`, `¥600.00`).
    pub fn format(self, value: f64) -> String {
        format!("{}{value:.2}", self.symbol())
    }
}

/// How to run the bar, as stored in `portkey-usage.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub budget: Option<f64>,
    #[serde(default)]
    pub currency: Currency,
    #[serde(default = "default_metadata_key")]
    pub metadata_key: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
}

fn default_metadata_key() -> String {
    DEFAULT_METADATA_KEY.to_string()
}

fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}

impl Default for UsageSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            user: String::new(),
            api_key: String::new(),
            budget: None,
            currency: Currency::default(),
            metadata_key: default_metadata_key(),
            base_url: default_base_url(),
        }
    }
}

impl UsageSettings {
    pub fn path() -> PathBuf {
        if let Ok(path) = std::env::var("OXIDE_USAGE_FILE") {
            if !path.trim().is_empty() {
                return PathBuf::from(path);
            }
        }
        crate::config::config_dir_or_default().join(CONFIG_FILE)
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading Portkey usage settings at {}", path.display()))?;
        let mut settings: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing Portkey usage settings at {}", path.display()))?;
        if settings.metadata_key.trim().is_empty() {
            settings.metadata_key = default_metadata_key();
        }
        if settings.base_url.trim().is_empty() {
            settings.base_url = default_base_url();
        }
        Ok(settings)
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
        std::fs::write(path, format!("{text}\n"))
            .with_context(|| format!("writing Portkey usage settings to {}", path.display()))?;
        crate::auth::restrict_permissions(path)?;
        Ok(())
    }

    /// The API key used for usage queries: the one set with `/usage key`, or
    /// else the active Portkey credential once the user logged in.
    pub fn effective_key<'a>(&'a self, config: &'a Config) -> Option<&'a str> {
        let key = self.api_key.trim();
        if !key.is_empty() {
            return Some(key);
        }
        let provider_key = config.api_key.trim();
        (config.is_portkey() && !provider_key.is_empty()).then_some(provider_key)
    }

    /// The bar reads the spend of the logged-in Portkey account, so it only
    /// runs while the active provider is Portkey with a usable credential.
    pub fn available(&self, config: &Config) -> bool {
        config.is_portkey() && self.effective_key(config).is_some()
    }

    /// The analytics host, normalized without a trailing slash.
    fn endpoint(&self) -> String {
        self.base_url.trim().trim_end_matches('/').to_string()
    }

    /// The metadata filter that scopes a query to one user, per Portkey's
    /// stringified-json `metadata` query parameter.
    fn metadata_filter(&self) -> String {
        json!({ self.metadata_key.trim(): self.user.trim() }).to_string()
    }
}

/// The spend columns fetched from Portkey, in USD.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Snapshot {
    pub today: f64,
    pub month: f64,
}

/// The status bar contents. A column stays `None` until the first fetch
/// succeeds, and the last known values are kept when a later fetch fails.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageBar {
    pub user: String,
    pub today: Option<f64>,
    pub month: Option<f64>,
    pub budget: Option<f64>,
    pub currency: Currency,
    pub error: Option<String>,
}

impl UsageBar {
    pub fn new(settings: &UsageSettings) -> Self {
        Self {
            user: settings.user.trim().to_string(),
            today: None,
            month: None,
            budget: settings.budget,
            currency: settings.currency,
            error: None,
        }
    }

    /// Copies the user, budget, and currency after a `/usage` change.
    pub fn update(&mut self, settings: &UsageSettings) {
        self.user = settings.user.trim().to_string();
        self.budget = settings.budget;
        self.currency = settings.currency;
    }

    pub fn apply(&mut self, result: Result<Snapshot, String>) {
        match result {
            Ok(snapshot) => {
                self.today = Some(snapshot.today);
                self.month = Some(snapshot.month);
                self.error = None;
            }
            Err(error) => self.error = Some(collapse(&error)),
        }
    }

    /// The columns after the user label, e.g.
    /// `Session: $0.00 | Today: $20.61 | Month: $220.69 / $600.00`.
    ///
    /// The spend columns stay in USD (that is what Portkey reports); the
    /// currency only formats the budget the user set.
    pub fn columns(&self, session: f64) -> String {
        let mut text = format!(
            "Session: {} | Today: {} | Month: {}",
            format_cost(session),
            money(self.today),
            money(self.month)
        );
        if let Some(budget) = self.budget {
            text.push_str(&format!(" / {}", self.currency.format(budget)));
        }
        if let Some(error) = &self.error {
            text.push_str(&format!(" | {error}"));
        }
        text
    }
}

/// The `/usage` status block shown in the transcript.
pub fn status_text(settings: &UsageSettings, config: &Config) -> String {
    let key = if !settings.api_key.trim().is_empty() {
        "set with /usage key"
    } else if settings.effective_key(config).is_some() {
        "from the active Portkey provider"
    } else {
        "missing — set it with /usage key <pk-...>"
    };
    let usage = "/usage on|off · /usage user <firstname.lastname> · /usage key <pk-...> · \
                 /usage budget <amount|off> · /usage currency <usd|cny> · /usage metadata <key>";
    let provider = if config.is_portkey() {
        "portkey"
    } else {
        "not Portkey — run /login portkey"
    };
    format!(
        "portkey usage bar: {}\nuser: {} (metadata `{}`) · budget: {} {} · provider: {provider} · api key: {key}\n{usage}",
        if settings.enabled && settings.available(config) {
            "on"
        } else if settings.enabled {
            "off (needs the Portkey provider)"
        } else {
            "off"
        },
        match settings.user.trim() {
            "" => "(unset)",
            user => user,
        },
        settings.metadata_key.trim(),
        settings
            .budget
            .map(|budget| settings.currency.format(budget))
            .unwrap_or_else(|| "none".to_string()),
        settings.currency.name(),
    )
}

/// A shared client so refreshes reuse connections instead of rebuilding the
/// pool (and re-doing TLS setup) on every tick.
fn usage_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Queries today's and the current month's spend for the configured user.
pub async fn spend(settings: &UsageSettings, key: &str) -> Result<Snapshot> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("the system clock is before the unix epoch")?
        .as_secs() as i64;
    let offset = local_offset_seconds();
    let (today_start, month_start) = windows(now, offset);
    let max = iso8601(now, offset);
    // The two windows are independent, so query them together: a refresh then
    // costs one round trip instead of two.
    let (today, month) = tokio::join!(
        window_cost(usage_client(), settings, key, today_start, &max, offset),
        window_cost(usage_client(), settings, key, month_start, &max, offset),
    );
    Ok(Snapshot {
        today: today?,
        month: month?,
    })
}

/// The epoch-second starts of the local day and local month containing `now`.
fn windows(now: i64, offset: i64) -> (i64, i64) {
    let day = (now + offset).div_euclid(86_400);
    let (year, month, _) = civil_from_days(day);
    (
        day * 86_400 - offset,
        days_from_civil(year, month, 1) * 86_400 - offset,
    )
}

async fn window_cost(
    client: &reqwest::Client,
    settings: &UsageSettings,
    key: &str,
    min: i64,
    max: &str,
    offset: i64,
) -> Result<f64> {
    let url = format!("{}/analytics/graphs/cost", settings.endpoint());
    let response = client
        .get(url)
        .header("x-portkey-api-key", key)
        .query(&[
            ("time_of_generation_min", iso8601(min, offset)),
            ("time_of_generation_max", max.to_string()),
            ("metadata", settings.metadata_filter()),
        ])
        .send()
        .await
        .context("querying Portkey usage")?;
    let status = response.status();
    let text = response
        .text()
        .await
        .context("reading the Portkey usage response")?;
    if !status.is_success() {
        bail!("Portkey usage failed ({status}): {}", api_error(&text));
    }
    let value: Value =
        serde_json::from_str(&text).context("parsing the Portkey usage response as JSON")?;
    let cents = value
        .get("summary")
        .and_then(|summary| summary.get("total"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Ok(cents / 100.0)
}

fn money(value: Option<f64>) -> String {
    value.map(format_cost).unwrap_or_else(|| "…".to_string())
}

/// The message from a Portkey error body (`{"data":{"message":...}}`),
/// falling back to the raw, collapsed text.
fn api_error(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("data")
                .and_then(|data| data.get("message"))
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| collapse(text))
}

/// Formats a USD amount the way the bar shows it (`$20.61`).
pub fn format_cost(value: f64) -> String {
    format!("${value:.2}")
}

/// The local UTC offset in seconds: `date +%z` where available, UTC otherwise.
fn local_offset_seconds() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .map(|output| parse_offset(&String::from_utf8_lossy(&output.stdout)))
            .unwrap_or(0)
    })
}

/// Parses a `date +%z` value such as `+0530` or `-08:00` into seconds.
fn parse_offset(text: &str) -> i64 {
    let text = text.trim().replace(':', "");
    let digits = text.strip_prefix(['+', '-']).unwrap_or(&text);
    if digits.len() < 4 || !digits[..4].chars().all(|c| c.is_ascii_digit()) {
        return 0;
    }
    let hours: i64 = digits[..2].parse().unwrap_or(0);
    let minutes: i64 = digits[2..4].parse().unwrap_or(0);
    let seconds = hours * 3600 + minutes * 60;
    if text.starts_with('-') {
        -seconds
    } else {
        seconds
    }
}

/// Formats epoch seconds as ISO8601 with the given offset
/// (`2026-02-23T14:20:31+05:30`), which is what the analytics API expects.
fn iso8601(timestamp: i64, offset: i64) -> String {
    let local = timestamp + offset;
    let day = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);
    let (year, month, date) = civil_from_days(day);
    let sign = if offset < 0 { '-' } else { '+' };
    let absolute = offset.abs();
    format!(
        "{year:04}-{month:02}-{date:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60,
        absolute / 3600,
        (absolute % 3600) / 60,
    )
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let date = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, date)
}

/// A civil date to days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, date: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + date as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// Collapses whitespace and truncates, so an API error stays on one line.
fn collapse(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= MAX_ERROR_CHARS {
        return text;
    }
    text.chars().take(MAX_ERROR_CHARS).collect::<String>() + "..."
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "oxide_portkey_usage_{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn settings_round_trip_with_defaults() {
        let dir = temp_dir("settings");
        let path = dir.join(CONFIG_FILE);

        let empty = UsageSettings::load_from(&path).unwrap();
        assert_eq!(empty, UsageSettings::default());
        assert_eq!(empty.metadata_key, "_user");

        let filled = UsageSettings {
            enabled: true,
            user: "firstname.lastname".to_string(),
            api_key: "pk-test".to_string(),
            budget: Some(600.0),
            currency: Currency::Cny,
            ..UsageSettings::default()
        };
        filled.save_to(&path).unwrap();
        assert_eq!(UsageSettings::load_from(&path).unwrap(), filled);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn blank_keys_fall_back_to_defaults() {
        let dir = temp_dir("blank_keys");
        let path = dir.join(CONFIG_FILE);
        std::fs::write(&path, r#"{"user":"a.b","metadata_key":" ","base_url":""}"#).unwrap();

        let settings = UsageSettings::load_from(&path).unwrap();
        assert_eq!(settings.metadata_key, "_user");
        assert_eq!(settings.base_url, DEFAULT_BASE_URL);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn effective_key_prefers_its_own_credential() {
        let mut config = Config::default();
        config.apply_provider("portkey", "pk-provider");
        let mut settings = UsageSettings::default();
        assert_eq!(settings.effective_key(&config), Some("pk-provider"));

        settings.api_key = "pk-usage".to_string();
        assert_eq!(settings.effective_key(&config), Some("pk-usage"));

        config.apply_provider("openai", "sk-openai");
        assert_eq!(UsageSettings::default().effective_key(&config), None);
    }

    #[test]
    fn the_bar_only_runs_for_a_logged_in_portkey_provider() {
        let mut config = Config::default();
        let settings = UsageSettings {
            user: "firstname.lastname".to_string(),
            ..UsageSettings::default()
        };
        assert!(!settings.available(&config));

        config.apply_provider("portkey", "pk-provider");
        assert!(settings.available(&config));

        config.apply_provider("openai", "sk-openai");
        assert!(!settings.available(&config));
        let own_key = UsageSettings {
            api_key: "pk-usage".to_string(),
            ..settings
        };
        assert!(!own_key.available(&config));
    }

    #[test]
    fn currencies_parse_and_format() {
        assert_eq!(Currency::parse("$"), Some(Currency::Usd));
        assert_eq!(Currency::parse("USD"), Some(Currency::Usd));
        assert_eq!(Currency::parse("¥"), Some(Currency::Cny));
        assert_eq!(Currency::parse("rmb"), Some(Currency::Cny));
        assert_eq!(Currency::parse("eur"), None);
        assert_eq!(Currency::Usd.format(600.0), "$600.00");
        assert_eq!(Currency::Cny.format(600.0), "¥600.00");
        assert_eq!(Currency::default(), Currency::Usd);
    }

    #[test]
    fn metadata_filter_scopes_to_the_user() {
        let settings = UsageSettings {
            user: "firstname.lastname".to_string(),
            ..UsageSettings::default()
        };
        let value: Value = serde_json::from_str(&settings.metadata_filter()).unwrap();
        assert_eq!(value["_user"], "firstname.lastname");

        let custom = UsageSettings {
            user: "a.b@example.com".to_string(),
            metadata_key: "email".to_string(),
            ..UsageSettings::default()
        };
        let value: Value = serde_json::from_str(&custom.metadata_filter()).unwrap();
        assert_eq!(value["email"], "a.b@example.com");
    }

    #[test]
    fn endpoint_drops_trailing_slashes() {
        let settings = UsageSettings {
            base_url: "https://portkey.example.com/v1/".to_string(),
            ..UsageSettings::default()
        };
        assert_eq!(settings.endpoint(), "https://portkey.example.com/v1");
    }

    #[test]
    fn columns_render_spend_budget_and_errors() {
        let settings = UsageSettings {
            user: "firstname.lastname".to_string(),
            budget: Some(600.0),
            ..UsageSettings::default()
        };
        let mut bar = UsageBar::new(&settings);
        assert_eq!(bar.user, "firstname.lastname");
        assert_eq!(
            bar.columns(0.0),
            "Session: $0.00 | Today: … | Month: … / $600.00"
        );

        bar.apply(Ok(Snapshot {
            today: 20.61,
            month: 220.69,
        }));
        assert_eq!(
            bar.columns(1.5),
            "Session: $1.50 | Today: $20.61 | Month: $220.69 / $600.00"
        );

        bar.apply(Err("boom\nfailed".to_string()));
        assert_eq!(
            bar.columns(1.5),
            "Session: $1.50 | Today: $20.61 | Month: $220.69 / $600.00 | boom failed"
        );

        let bare = UsageBar::new(&UsageSettings::default());
        assert_eq!(bare.columns(0.0), "Session: $0.00 | Today: … | Month: …");
    }

    #[test]
    fn the_budget_uses_the_configured_currency() {
        let settings = UsageSettings {
            user: "firstname.lastname".to_string(),
            budget: Some(600.0),
            currency: Currency::Cny,
            ..UsageSettings::default()
        };
        let mut bar = UsageBar::new(&settings);
        assert_eq!(bar.currency, Currency::Cny);
        bar.apply(Ok(Snapshot {
            today: 20.61,
            month: 220.69,
        }));
        assert_eq!(
            bar.columns(0.0),
            "Session: $0.00 | Today: $20.61 | Month: $220.69 / ¥600.00"
        );

        bar.update(&UsageSettings::default());
        assert_eq!(bar.currency, Currency::Usd);
        assert_eq!(
            bar.columns(0.0),
            "Session: $0.00 | Today: $20.61 | Month: $220.69"
        );
    }

    #[test]
    fn status_text_reports_configuration() {
        let mut config = Config::default();
        let text = status_text(&UsageSettings::default(), &config);
        assert!(text.contains("bar: off"));
        assert!(text.contains("user: (unset)"));
        assert!(text.contains("api key: missing"));
        assert!(text.contains("provider: not Portkey"));
        assert!(text.contains("/usage currency <usd|cny>"));

        let enabled = UsageSettings {
            enabled: true,
            user: "firstname.lastname".to_string(),
            budget: Some(600.0),
            ..UsageSettings::default()
        };
        assert!(status_text(&enabled, &config).contains("bar: off (needs the Portkey provider)"));

        config.apply_provider("portkey", "pk-provider");
        let text = status_text(&enabled, &config);
        assert!(text.contains("bar: on"));
        assert!(text.contains("user: firstname.lastname"));
        assert!(text.contains("budget: $600.00 usd"));
        assert!(text.contains("provider: portkey"));
        assert!(text.contains("api key: from the active Portkey provider"));
    }

    #[test]
    fn offsets_parse_and_fall_back_to_utc() {
        assert_eq!(parse_offset("+0530\n"), 19_800);
        assert_eq!(parse_offset("-0800"), -28_800);
        assert_eq!(parse_offset("+00:00"), 0);
        assert_eq!(parse_offset("-05:30"), -19_800);
        assert_eq!(parse_offset("UTC"), 0);
        assert_eq!(parse_offset(""), 0);
    }

    #[test]
    fn civil_dates_round_trip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        for days in [0, 1, 365, 11_017, 19_723, 20_454, -1, -365] {
            let (year, month, date) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, date), days);
        }
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(-719_468), (0, 3, 1));
    }

    #[test]
    fn iso8601_matches_the_api_format() {
        assert_eq!(iso8601(0, 0), "1970-01-01T00:00:00+00:00");
        assert_eq!(iso8601(0, 19_800), "1970-01-01T05:30:00+05:30");
        assert_eq!(iso8601(0, -28_800), "1969-12-31T16:00:00-08:00");
        assert_eq!(iso8601(1_710_496_800, 19_800), "2024-03-15T15:30:00+05:30");
    }

    #[test]
    fn windows_start_at_the_local_day_and_month() {
        let now = 1_710_496_800;
        let (today, month) = windows(now, 19_800);
        assert_eq!(iso8601(today, 19_800), "2024-03-15T00:00:00+05:30");
        assert_eq!(iso8601(month, 19_800), "2024-03-01T00:00:00+05:30");

        let (today, month) = windows(now, 0);
        assert_eq!(iso8601(today, 0), "2024-03-15T00:00:00+00:00");
        assert_eq!(iso8601(month, 0), "2024-03-01T00:00:00+00:00");
    }

    #[test]
    fn windows_use_the_local_calendar() {
        // 2024-03-31T20:00:00Z is already April in +05:30.
        let now = 1_711_915_200;
        let (today, month) = windows(now, 19_800);
        assert_eq!(iso8601(today, 19_800), "2024-04-01T00:00:00+05:30");
        assert_eq!(iso8601(month, 19_800), "2024-04-01T00:00:00+05:30");
    }

    #[test]
    fn errors_stay_on_one_short_line() {
        assert_eq!(collapse(" boom\nfailed \t hard "), "boom failed hard");
        let long = collapse(&"x".repeat(400));
        assert_eq!(long.chars().count(), MAX_ERROR_CHARS + 3);
        assert!(long.ends_with("..."));
    }

    #[test]
    fn api_errors_use_the_portkey_message() {
        let body = r#"{"success":false,"data":{"message":"Invalid API key","errorCode":"AB05"}}"#;
        assert_eq!(api_error(body), "Invalid API key");
        assert_eq!(api_error(r#"{"message":"nope"}"#), "nope");
        assert_eq!(api_error("<html>boom</html>"), "<html>boom</html>");
    }

    /// Serves `responses` (status, body) pairs in order and returns the raw
    /// request lines it saw.
    async fn serve(
        responses: Vec<(u16, &'static str)>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = vec![0u8; 4096];
                let read = socket.read(&mut buffer).await.unwrap();
                requests.push(String::from_utf8_lossy(&buffer[..read]).to_string());
                let response = format!(
                    "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn spend_queries_the_cost_graph_and_reads_cents() {
        const COST: &str =
            r#"{"summary":{"total":2061,"avg":10},"data_points":[],"object":"analytics-graph"}"#;
        // Both windows return the same body: the two requests are concurrent,
        // so the server cannot depend on their order.
        let (addr, server) = serve(vec![(200, COST), (200, COST)]).await;
        let settings = UsageSettings {
            user: "firstname.lastname".to_string(),
            base_url: format!("http://{addr}"),
            ..UsageSettings::default()
        };

        let snapshot = spend(&settings, "pk-test").await.unwrap();
        assert_eq!(snapshot.today, 20.61);
        assert_eq!(snapshot.month, 20.61);

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2);
        for request in &requests {
            assert!(
                request.starts_with("GET /analytics/graphs/cost?"),
                "{request}"
            );
            assert!(request.contains("time_of_generation_min="), "{request}");
            assert!(request.contains("time_of_generation_max="), "{request}");
            assert!(
                request.contains("metadata=%7B%22_user%22%3A%22firstname.lastname%22%7D"),
                "{request}"
            );
            assert!(
                request
                    .to_lowercase()
                    .contains("x-portkey-api-key: pk-test"),
                "{request}"
            );
        }
        // Today's window is narrower than the month's, and both end at now.
        let query = |request: &str| -> std::collections::BTreeMap<String, String> {
            let line = request.lines().next().unwrap();
            let query = line.split_once('?').map(|(_, q)| q).unwrap_or_default();
            query
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect()
        };
        let first = query(&requests[0]);
        let second = query(&requests[1]);
        let (month, today) = if first["time_of_generation_min"] <= second["time_of_generation_min"]
        {
            (first, second)
        } else {
            (second, first)
        };
        // Equal on the first of the month, otherwise the day window is later.
        assert!(today["time_of_generation_min"] >= month["time_of_generation_min"]);
        assert_eq!(
            today["time_of_generation_max"],
            month["time_of_generation_max"]
        );
    }

    #[tokio::test]
    async fn spend_surfaces_api_errors() {
        let body = r#"{"success":false,"data":{"message":"Invalid API key","errorCode":"AB05"}}"#;
        // The two windows are fetched concurrently, so serve both connections.
        let (addr, _server) = serve(vec![(401, body), (401, body)]).await;
        let settings = UsageSettings {
            user: "firstname.lastname".to_string(),
            base_url: format!("http://{addr}"),
            ..UsageSettings::default()
        };

        let error = spend(&settings, "pk-bad").await.unwrap_err().to_string();
        assert!(error.contains("401"), "{error}");
        assert!(error.contains("Invalid API key"), "{error}");
    }

    #[tokio::test]
    async fn spend_queries_both_windows_concurrently() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Two connections, each answered after a delay. Sequentially the two
        // windows cost two delays; concurrently they cost one.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 4096];
                    let _ = socket.read(&mut buffer).await;
                    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                    let body = r#"{"summary":{"total":100}}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });

        let settings = UsageSettings {
            user: "firstname.lastname".to_string(),
            base_url: format!("http://{addr}"),
            ..UsageSettings::default()
        };
        let start = std::time::Instant::now();
        let snapshot = spend(&settings, "pk-test").await.unwrap();
        let elapsed = start.elapsed();
        assert_eq!(snapshot.today, 1.0);
        assert_eq!(snapshot.month, 1.0);
        assert!(
            elapsed < std::time::Duration::from_millis(1000),
            "spend took {elapsed:?}; the windows were not queried concurrently"
        );
        server.await.unwrap();
    }
}
