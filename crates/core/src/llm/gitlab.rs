//! GitLab Duo, reached through GitLab's own AI gateway.
//!
//! A Duo subscription is not an endpoint a token opens directly. The token a
//! reader stores — a personal access token with the `ai_features` scope — is
//! presented to the GitLab instance, which answers with a short-lived gateway
//! token and the headers that token goes with; only then does a chat request to
//! the gateway, which serves a proxy in front of the models Duo is subscribed
//! to, mean anything. So the exchange is repeated whenever the token it cached
//! is about to lapse, and the token that outlives it is the one in `auth.json`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::Value;

/// The instance a Duo subscription is served from when the configuration does
/// not name another one, and the gateway its proxy lives behind.
pub const INSTANCE: &str = "https://gitlab.com";
pub const GATEWAY: &str = "https://cloud.gitlab.com";

/// Where an instance mints a gateway token.
const DIRECT_ACCESS_PATH: &str = "/api/v4/ai/third_party_agents/direct_access";
/// GitLab's gateway tokens last half an hour, so one is dropped well before
/// then rather than discovered to be dead mid-turn.
const TOKEN_TTL_SECS: i64 = 25 * 60;

/// What an instance answered: the token to send, and any header that goes with
/// it. GitLab may name headers the proxy requires, so they travel with the
/// token rather than being assumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectAccess {
    pub token: String,
    pub headers: Vec<(String, String)>,
}

#[derive(Clone)]
struct Cached {
    access: DirectAccess,
    expires_at: i64,
}

fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// The base a chat or model request is built from: one of the two proxies the
/// gateway serves in front of the models a Duo subscription reaches.
pub fn openai_base(gateway: &str) -> String {
    format!("{}/ai/v1/proxy/openai/v1", gateway_base(gateway))
}

pub fn anthropic_base(gateway: &str) -> String {
    format!("{}/ai/v1/proxy/anthropic/v1", gateway_base(gateway))
}

fn gateway_base(gateway: &str) -> String {
    gateway.trim().trim_end_matches('/').to_string()
}

/// The gateway token a stored credential authorizes, minting one when the
/// cached token is missing or about to lapse. The token is not a secret worth
/// keeping on disk — it dies within the half hour — so it lives here, and the
/// credential in `auth.json` is what outlives it.
pub async fn direct_access(
    http: &reqwest::Client,
    instance: &str,
    credential: &str,
) -> Result<DirectAccess> {
    let instance = instance.trim().trim_end_matches('/');
    let key = format!("{instance}\n{credential}");
    let now = now_secs();
    if let Some(cached) = cache()
        .lock()
        .ok()
        .and_then(|held| held.get(&key).cloned())
        .filter(|cached| cached.expires_at > now)
    {
        return Ok(cached.access);
    }
    let access = exchange(http, instance, credential).await?;
    if let Ok(mut held) = cache().lock() {
        held.retain(|_, cached| cached.expires_at > now);
        held.insert(
            key,
            Cached {
                access: access.clone(),
                expires_at: now + TOKEN_TTL_SECS,
            },
        );
    }
    Ok(access)
}

async fn exchange(
    http: &reqwest::Client,
    instance: &str,
    credential: &str,
) -> Result<DirectAccess> {
    let url = format!("{instance}{DIRECT_ACCESS_PATH}");
    let response = http
        .post(&url)
        .bearer_auth(credential)
        .json(&serde_json::json!({}))
        .send()
        .await
        .with_context(|| format!("requesting {url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let detail = body.trim();
        match status.as_u16() {
            401 => anyhow::bail!(
                "GitLab refused the credential for `{instance}`. Create a personal access token with the `ai_features` scope at https://gitlab.com/-/user_settings/personal_access_tokens and run `/login gitlab`"
            ),
            403 => anyhow::bail!(
                "GitLab Duo is not available to this account on `{instance}` — Duo Agent Platform needs GitLab Ultimate with the Duo Enterprise add-on, and the AI features enabled for the account"
            ),
            404 => anyhow::bail!(
                "`{instance}` has no third-party agent endpoint — Duo Agent Platform has to be enabled on the instance"
            ),
            _ => anyhow::bail!("GitLab returned {status}: {detail}"),
        }
    }
    let value: Value = serde_json::from_str(&body).context("parsing the GitLab direct access")?;
    parse_direct_access(&value)
}

/// Reads the token and headers out of a direct-access answer. The `x-api-key`
/// header some answers carry is dropped: the token is what authorizes the
/// request, and sending both invites a proxy to prefer the wrong one.
pub fn parse_direct_access(value: &Value) -> Result<DirectAccess> {
    let token = value
        .get("token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .context("GitLab answered without a gateway token")?
        .to_string();
    let headers = value
        .get("headers")
        .and_then(Value::as_object)
        .map(|headers| {
            headers
                .iter()
                .filter(|(name, _)| !name.eq_ignore_ascii_case("x-api-key"))
                .filter_map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| (name.to_string(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(DirectAccess { token, headers })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn reads_the_token_and_the_headers_gitlab_hands_over() {
        let access = parse_direct_access(&json!({
            "token": "jwt-abc",
            "headers": {
                "X-Gitlab-Realm": "saas",
                "X-Gitlab-Feature-Enabled-With-Token": "duo_agent_platform",
                "x-api-key": "ignored"
            }
        }))
        .unwrap();
        assert_eq!(access.token, "jwt-abc");
        assert_eq!(
            access.headers,
            vec![
                (
                    "X-Gitlab-Feature-Enabled-With-Token".to_string(),
                    "duo_agent_platform".to_string()
                ),
                ("X-Gitlab-Realm".to_string(), "saas".to_string()),
            ]
        );
    }

    #[test]
    fn an_answer_without_a_token_is_an_error() {
        assert!(parse_direct_access(&json!({ "headers": {} })).is_err());
        assert!(parse_direct_access(&json!({ "token": "  " })).is_err());
    }

    #[test]
    fn an_answer_without_headers_still_carries_its_token() {
        let access = parse_direct_access(&json!({ "token": "jwt-abc" })).unwrap();
        assert!(access.headers.is_empty());
    }

    #[test]
    fn a_chat_goes_to_the_gateways_proxy() {
        assert_eq!(
            openai_base("https://cloud.gitlab.com"),
            "https://cloud.gitlab.com/ai/v1/proxy/openai/v1"
        );
        assert_eq!(
            anthropic_base("https://gitlab.example.com/"),
            "https://gitlab.example.com/ai/v1/proxy/anthropic/v1"
        );
    }

    /// Answers one request per connection, and stops once the caller has been
    /// quiet for `idle_ms`, returning the raw requests it read so a test can
    /// assert on what really went out.
    async fn stub_server(
        status: &str,
        body: &str,
        idle_ms: u64,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let status = status.to_string();
        let body = body.to_string();
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            while let Ok(Ok((mut socket, _))) =
                tokio::time::timeout(Duration::from_millis(idle_ms), listener.accept()).await
            {
                let mut buf = vec![0u8; 8192];
                let read = socket.read(&mut buf).await.unwrap_or(0);
                seen.push(String::from_utf8_lossy(&buf[..read]).to_string());
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
            seen
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn the_instance_mints_the_token_a_turn_sends() {
        let (instance, server) = stub_server(
            "200 OK",
            &json!({
                "token": "jwt-abc",
                "headers": { "X-Gitlab-Realm": "saas", "x-api-key": "ignored" }
            })
            .to_string(),
            300,
        )
        .await;
        let http = reqwest::Client::new();
        let access = direct_access(&http, &instance, "glpat-secret")
            .await
            .unwrap();
        assert_eq!(access.token, "jwt-abc");
        assert_eq!(
            access.headers,
            vec![("X-Gitlab-Realm".into(), "saas".into())]
        );
        // A second turn reuses the token rather than asking again.
        assert_eq!(
            direct_access(&http, &instance, "glpat-secret")
                .await
                .unwrap(),
            access
        );
        let seen = server.await.unwrap();
        assert_eq!(seen.len(), 1, "the minted token is cached");
        let request = &seen[0];
        assert!(request.starts_with(&format!("POST {DIRECT_ACCESS_PATH} HTTP/1.1")));
        let lower = request.to_ascii_lowercase();
        assert!(
            lower.contains("authorization: bearer glpat-secret"),
            "{request}"
        );
    }

    #[tokio::test]
    async fn a_refused_credential_names_the_scope_to_grant() {
        let (instance, _server) = stub_server("401 Unauthorized", "{}", 50).await;
        let error = direct_access(&reqwest::Client::new(), &instance, "bad")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("ai_features"), "{error}");
    }

    #[tokio::test]
    async fn a_licence_without_duo_is_reported_as_such() {
        let (instance, _server) = stub_server("403 Forbidden", "{}", 50).await;
        let error = direct_access(&reqwest::Client::new(), &instance, "glpat")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Ultimate"), "{error}");
    }

    /// A 404 is not a bad credential: the instance has no third-party agent
    /// endpoint at all, and telling the reader to make another token would send
    /// them round the same loop.
    #[tokio::test]
    async fn an_instance_without_the_endpoint_says_so() {
        let (instance, _server) = stub_server("404 Not Found", "{}", 50).await;
        let error = direct_access(&reqwest::Client::new(), &instance, "glpat")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("third-party agent endpoint"), "{error}");
        assert!(!error.contains("ai_features"), "{error}");
    }
}
