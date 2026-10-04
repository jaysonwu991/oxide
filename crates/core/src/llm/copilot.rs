//! GitHub Copilot's own two-step login.
//!
//! A Copilot subscription is reached with a GitHub token, but not directly: the
//! token is exchanged at GitHub for a short-lived Copilot session token, which
//! is what the API accepts and what names the endpoint this account is served
//! from. So a login is the GitHub device flow (a code the user approves in a
//! browser, since oxide ships no callback URL), and a turn is that exchange
//! repeated whenever the session token it cached has expired.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::Value;

/// The public OAuth application the Copilot plugins log in with. A device flow
/// identifies the client rather than a secret, so the same id is the one every
/// client uses.
pub const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

pub const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
pub const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
pub const SESSION_URL: &str = "https://api.github.com/copilot_internal/v2/token";

/// The headers GitHub and Copilot expect a client to identify itself with.
pub const INTEGRATION_ID: &str = "vscode-chat";
pub const EDITOR_VERSION: &str = "vscode/1.99.3";
pub const PLUGIN_VERSION: &str = "copilot-chat/0.26.7";

/// What GitHub answered when asked to start a device flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLogin {
    pub device_code: String,
    /// The short code the user types into the browser.
    pub user_code: String,
    /// Where they type it.
    pub verification_uri: String,
    pub interval: Duration,
    pub expires_in: Duration,
}

/// A Copilot session: the token to send, and the endpoint it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotSession {
    pub token: String,
    pub expires_at: i64,
    /// The account's own API host, which may not be the public one.
    pub api: String,
}

/// The answer to one poll of the device flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    Token(String),
    /// The user has not approved yet.
    Pending,
    /// GitHub wants the polling slowed down.
    SlowDown,
    Denied,
    Expired,
}

/// The interval between polls when GitHub does not name one.
const DEFAULT_INTERVAL_SECS: u64 = 5;
/// How long a device flow is watched before giving up when GitHub does not say.
const DEFAULT_EXPIRES_SECS: u64 = 15 * 60;
/// A session token is reused until it is this close to expiring, so a turn
/// never starts with a token that dies mid-request.
const REFRESH_MARGIN_SECS: i64 = 60;

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// Asks GitHub for a code the user approves in a browser.
pub async fn start_device_login(http: &reqwest::Client, client_id: &str) -> Result<DeviceLogin> {
    let response = http
        .post(DEVICE_CODE_URL)
        .header("accept", "application/json")
        .form(&[("client_id", client_id), ("scope", "read:user")])
        .send()
        .await
        .context("starting the GitHub device flow")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("GitHub returned {status}: {}", body.trim());
    }
    let value: Value = serde_json::from_str(&body).context("parsing the device code")?;
    parse_device_code(&value)
}

pub fn parse_device_code(value: &Value) -> Result<DeviceLogin> {
    let field = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .with_context(|| format!("the device code has no `{name}`"))
    };
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        let detail = value
            .get("error_description")
            .and_then(Value::as_str)
            .unwrap_or(error);
        anyhow::bail!("GitHub refused the device flow: {detail}");
    }
    let seconds = |name: &str, fallback: u64| {
        value
            .get(name)
            .and_then(Value::as_u64)
            .filter(|seconds| *seconds > 0)
            .unwrap_or(fallback)
    };
    Ok(DeviceLogin {
        device_code: field("device_code")?,
        user_code: field("user_code")?,
        verification_uri: field("verification_uri")?,
        interval: Duration::from_secs(seconds("interval", DEFAULT_INTERVAL_SECS)),
        expires_in: Duration::from_secs(seconds("expires_in", DEFAULT_EXPIRES_SECS)),
    })
}

/// Polls once for the token the user approved.
pub async fn poll_for_token(
    http: &reqwest::Client,
    login: &DeviceLogin,
    client_id: &str,
) -> Result<Poll> {
    let response = http
        .post(ACCESS_TOKEN_URL)
        .header("accept", "application/json")
        .form(&[
            ("client_id", client_id),
            ("device_code", login.device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .context("polling the GitHub device flow")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("GitHub returned {status}: {}", body.trim());
    }
    let value: Value = serde_json::from_str(&body).context("parsing the device flow answer")?;
    Ok(parse_poll(&value))
}

pub fn parse_poll(value: &Value) -> Poll {
    if let Some(token) = value
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        return Poll::Token(token.to_string());
    }
    match value.get("error").and_then(Value::as_str).unwrap_or("") {
        "authorization_pending" => Poll::Pending,
        "slow_down" => Poll::SlowDown,
        "expired_token" => Poll::Expired,
        "access_denied" => Poll::Denied,
        _ => Poll::Pending,
    }
}

/// Runs the whole login: asks for a code, hands it to `announce` so the
/// front-end can show the URL and the code, then polls until the user approves,
/// GitHub gives up, or the code expires.
pub async fn login(
    http: &reqwest::Client,
    client_id: &str,
    mut announce: impl FnMut(&DeviceLogin),
) -> Result<String> {
    let login = start_device_login(http, client_id).await?;
    announce(&login);
    let deadline = tokio::time::Instant::now() + login.expires_in;
    let mut wait = login.interval;
    loop {
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("the GitHub code expired before it was approved");
        }
        tokio::time::sleep(wait.min(deadline - tokio::time::Instant::now())).await;
        match poll_for_token(http, &login, client_id).await? {
            Poll::Token(token) => return Ok(token),
            Poll::Denied => anyhow::bail!("the GitHub login was denied"),
            Poll::Expired => anyhow::bail!("the GitHub code expired before it was approved"),
            poll => wait = next_interval(wait, &poll),
        }
    }
}

/// The interval the poll after `poll` is made after. RFC 8628 raises the
/// interval on `slow_down` for the rest of the flow, so it is added to what is
/// already being waited and an ordinary `authorization_pending` keeps it —
/// returning to the flow's original interval is what earns the throttle again.
fn next_interval(wait: Duration, poll: &Poll) -> Duration {
    match poll {
        Poll::SlowDown => wait + Duration::from_secs(DEFAULT_INTERVAL_SECS),
        _ => wait,
    }
}

/// Exchanges a GitHub token for the Copilot session token and endpoint it
/// authorizes. A GitHub account without a Copilot subscription is what the
/// refusal means, so the message says so.
pub async fn exchange(http: &reqwest::Client, github_token: &str) -> Result<CopilotSession> {
    let response = http
        .get(SESSION_URL)
        .header("authorization", format!("token {github_token}"))
        .header("accept", "application/json")
        .header("editor-version", EDITOR_VERSION)
        .header("copilot-integration-id", INTEGRATION_ID)
        .send()
        .await
        .context("exchanging the GitHub token for a Copilot session")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::UNAUTHORIZED {
            anyhow::bail!(
                "GitHub refused the Copilot session ({status}) - the account may have no Copilot \
                 subscription, or the token may lack access: {}",
                body.trim()
            );
        }
        anyhow::bail!("GitHub returned {status}: {}", body.trim());
    }
    let value: Value = serde_json::from_str(&body).context("parsing the Copilot session")?;
    parse_session(&value)
}

pub fn parse_session(value: &Value) -> Result<CopilotSession> {
    let token = value
        .get("token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .context("the Copilot session has no token")?
        .to_string();
    let expires_at = value.get("expires_at").and_then(Value::as_i64).unwrap_or(0);
    let api = value
        .get("endpoints")
        .and_then(|endpoints| endpoints.get("api"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|api| !api.is_empty())
        .unwrap_or(crate::config::COPILOT_API)
        .trim_end_matches('/')
        .to_string();
    Ok(CopilotSession {
        token,
        expires_at,
        api,
    })
}

/// Whether a session token is still worth sending. A token GitHub gave no
/// expiry for is kept until it is refused.
pub fn session_is_fresh(session: &CopilotSession, now: i64) -> bool {
    session.expires_at == 0 || session.expires_at - REFRESH_MARGIN_SECS > now
}

fn cache() -> &'static Mutex<HashMap<String, CopilotSession>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CopilotSession>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The Copilot session a stored GitHub token authorizes, minting one when the
/// cached session is missing or about to expire. The token is not a secret to
/// keep on disk - it dies within the half hour - so it lives here and nowhere
/// else, and the GitHub token in `auth.json` is what outlives it.
pub async fn session(http: &reqwest::Client, github_token: &str) -> Result<CopilotSession> {
    let now = now_secs();
    if let Some(session) = cache()
        .lock()
        .ok()
        .and_then(|held| held.get(github_token).cloned())
        .filter(|session| session_is_fresh(session, now))
    {
        return Ok(session);
    }
    let session = exchange(http, github_token).await?;
    if let Ok(mut held) = cache().lock() {
        held.retain(|_, cached| session_is_fresh(cached, now));
        held.insert(github_token.to_string(), session.clone());
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_code_the_reader_types_in_the_browser() {
        let login = parse_device_code(&json!({
            "device_code": "3584d83530557fdd1f46af8289938c8ef79f9dc5",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 5,
        }))
        .unwrap();
        assert_eq!(login.user_code, "WDJB-MJHT");
        assert_eq!(login.verification_uri, "https://github.com/login/device");
        assert_eq!(login.interval, Duration::from_secs(5));
        assert_eq!(login.expires_in, Duration::from_secs(900));
    }

    #[test]
    fn a_device_code_without_its_parts_is_an_error() {
        assert!(parse_device_code(&json!({ "user_code": "WDJB-MJHT" })).is_err());
        assert!(
            parse_device_code(&json!({ "error": "unauthorized_client" })).is_err(),
            "a refusal is reported rather than polled"
        );
    }

    #[test]
    fn a_device_code_defaults_what_github_leaves_out() {
        let login = parse_device_code(&json!({
            "device_code": "d",
            "user_code": "u",
            "verification_uri": "https://github.com/login/device",
        }))
        .unwrap();
        assert_eq!(login.interval, Duration::from_secs(DEFAULT_INTERVAL_SECS));
        assert_eq!(login.expires_in, Duration::from_secs(DEFAULT_EXPIRES_SECS));
    }

    #[test]
    fn a_poll_reads_the_token_or_the_reason_there_is_none() {
        assert_eq!(
            parse_poll(&json!({ "access_token": "ghu_abc", "token_type": "bearer" })),
            Poll::Token("ghu_abc".to_string())
        );
        assert_eq!(
            parse_poll(&json!({ "error": "authorization_pending" })),
            Poll::Pending
        );
        assert_eq!(parse_poll(&json!({ "error": "slow_down" })), Poll::SlowDown);
        assert_eq!(
            parse_poll(&json!({ "error": "access_denied" })),
            Poll::Denied
        );
        assert_eq!(
            parse_poll(&json!({ "error": "expired_token" })),
            Poll::Expired
        );
    }

    /// RFC 8628: the wait a `slow_down` answer asks for holds for the rest of
    /// the flow. A second throttle adds to it again, and the ordinary
    /// "not yet" leaves it alone rather than dropping back to the interval the
    /// flow started with.
    #[test]
    fn a_throttled_poll_stays_throttled() {
        let start = Duration::from_secs(DEFAULT_INTERVAL_SECS);
        let slowed = next_interval(start, &Poll::SlowDown);
        assert_eq!(slowed, start + Duration::from_secs(DEFAULT_INTERVAL_SECS));
        assert_eq!(next_interval(slowed, &Poll::Pending), slowed);
        assert_eq!(
            next_interval(slowed, &Poll::SlowDown),
            slowed + Duration::from_secs(DEFAULT_INTERVAL_SECS)
        );
        // A wait a server named in the device code is what the first poll uses,
        // and nothing but a throttle moves it.
        let announced = Duration::from_secs(30);
        assert_eq!(next_interval(announced, &Poll::Pending), announced);
    }

    #[test]
    fn a_session_carries_its_own_endpoint() {
        let session = parse_session(&json!({
            "token": "tid=1;exp=1700000000;",
            "expires_at": 1700000000,
            "endpoints": { "api": "https://api.githubcopilot.com", "proxy": "https://proxy" },
        }))
        .unwrap();
        assert_eq!(session.token, "tid=1;exp=1700000000;");
        assert_eq!(session.expires_at, 1700000000);
        assert_eq!(session.api, "https://api.githubcopilot.com");
    }

    #[test]
    fn a_session_without_an_endpoint_falls_back_to_the_public_one() {
        let session = parse_session(&json!({ "token": "t" })).unwrap();
        assert_eq!(session.api, crate::config::COPILOT_API);
        assert!(parse_session(&json!({ "expires_at": 1 })).is_err());
    }

    #[test]
    fn a_session_is_reused_until_it_is_about_to_expire() {
        let session = CopilotSession {
            token: "t".to_string(),
            expires_at: 1_000,
            api: crate::config::COPILOT_API.to_string(),
        };
        assert!(session_is_fresh(&session, 1_000 - REFRESH_MARGIN_SECS - 1));
        assert!(!session_is_fresh(&session, 1_000 - REFRESH_MARGIN_SECS));
        let endless = CopilotSession {
            expires_at: 0,
            ..session
        };
        assert!(session_is_fresh(&endless, i64::MAX / 2));
    }
}
