//! OAuth 2.0 (authorization code + PKCE) support for remote MCP servers.
//!
//! Remote MCP servers such as Slack's require the client to obtain a bearer
//! token before calling `tools/*`. This module discovers the authorization
//! server, runs the browser-based authorization-code flow with PKCE, stores the
//! resulting token under the oxide config dir, and refreshes it as needed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::ecosystem::McpOAuth;

const AUTHORIZE_TIMEOUT: Duration = Duration::from_secs(300);
const REFRESH_MARGIN: u64 = 60;

/// A persisted OAuth token plus the endpoints needed to refresh it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredAuth {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub token_endpoint: Option<String>,
}

#[derive(Debug, Clone)]
struct Metadata {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    scopes_supported: Vec<String>,
}

/// Shared OAuth state for one remote server. Cheap to clone via `Arc`.
pub struct OAuthState {
    name: String,
    config: McpOAuth,
    resource_url: String,
    client: reqwest::Client,
    auth: Mutex<Option<StoredAuth>>,
    metadata: Mutex<Option<Metadata>>,
}

impl OAuthState {
    pub fn new(name: &str, config: &McpOAuth, resource_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            name: name.to_string(),
            config: config.clone(),
            resource_url: resource_url.to_string(),
            client,
            auth: Mutex::new(load_stored(name)),
            metadata: Mutex::new(None),
        }
    }

    /// Ensures a usable token is available, running the interactive browser
    /// flow when `interactive` is true. When false, returns an error instead of
    /// opening a browser (for non-interactive `-p` runs).
    pub async fn ensure_authorized(&self, interactive: bool) -> Result<()> {
        if self.valid_token().await.is_some() {
            return Ok(());
        }
        if self.try_refresh().await {
            return Ok(());
        }
        if !interactive {
            bail!(
                "`{}` requires OAuth authorization; run `oxide mcp auth {}` first",
                self.name,
                self.name
            );
        }
        let auth = self.authorize().await?;
        self.store(auth).await;
        Ok(())
    }

    /// Returns a valid bearer token, refreshing it when it is close to expiry.
    pub async fn access_token(&self) -> Result<String> {
        if let Some(token) = self.valid_token().await {
            return Ok(token);
        }
        if self.try_refresh().await {
            if let Some(token) = self.valid_token().await {
                return Ok(token);
            }
        }
        bail!(
            "`{}` is not authenticated; run `oxide mcp auth {}`",
            self.name,
            self.name
        )
    }

    async fn valid_token(&self) -> Option<String> {
        let guard = self.auth.lock().await;
        guard
            .as_ref()
            .filter(|auth| !is_expired(auth))
            .map(|auth| auth.access_token.clone())
    }

    async fn store(&self, auth: StoredAuth) {
        if let Err(err) = save_stored(&self.name, &auth) {
            eprintln!(
                "[mcp] failed to store OAuth token for `{}`: {err:#}",
                self.name
            );
        }
        *self.auth.lock().await = Some(auth);
    }

    async fn try_refresh(&self) -> bool {
        let (refresh_token, stored_client, stored_endpoint) = {
            let guard = self.auth.lock().await;
            match guard.as_ref() {
                Some(auth) => (
                    auth.refresh_token.clone(),
                    auth.client_id.clone(),
                    auth.token_endpoint.clone(),
                ),
                None => return false,
            }
        };
        let Some(refresh_token) = refresh_token else {
            return false;
        };
        let client_id = stored_client.or_else(|| self.config.client_id.clone());
        let Some(client_id) = client_id else {
            return false;
        };
        let token_endpoint = match stored_endpoint {
            Some(endpoint) => endpoint,
            None => match self.metadata().await {
                Ok(metadata) => metadata.token_endpoint,
                Err(_) => return false,
            },
        };

        let mut form = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh_token.clone()),
            ("client_id", client_id.clone()),
        ];
        if let Some(secret) = &self.config.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        let response = match self.client.post(&token_endpoint).form(&form).send().await {
            Ok(response) => response,
            Err(err) => {
                eprintln!("[mcp] OAuth refresh for `{}` failed: {err}", self.name);
                return false;
            }
        };
        let status = response.status();
        let value = response.json::<Value>().await.unwrap_or(Value::Null);
        let mut auth = match parse_token(&value, status) {
            Ok(auth) => auth,
            Err(err) => {
                eprintln!("[mcp] OAuth refresh for `{}` failed: {err:#}", self.name);
                return false;
            }
        };
        if auth.refresh_token.is_none() {
            auth.refresh_token = Some(refresh_token);
        }
        auth.client_id = Some(client_id);
        auth.token_endpoint = Some(token_endpoint);
        self.store(auth).await;
        true
    }

    async fn authorize(&self) -> Result<StoredAuth> {
        let metadata = self.metadata().await?;
        let port = self.config.callback_port.unwrap_or(0);
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .with_context(|| format!("binding OAuth callback port {port}"))?;
        let bound_port = listener.local_addr()?.port();
        let redirect_uri = match &self.config.redirect_uri {
            Some(uri) => uri.clone(),
            None => format!("http://localhost:{bound_port}/callback"),
        };

        let client_id = match self.config.client_id.clone() {
            Some(client_id) => client_id,
            None => self.register_client(&metadata, &redirect_uri).await?,
        };

        let scopes = if self.config.scopes.is_empty() {
            metadata.scopes_supported.clone()
        } else {
            self.config.scopes.clone()
        };
        let scope_param = self
            .config
            .scope_param
            .clone()
            .unwrap_or_else(|| default_scope_param(&metadata.authorization_endpoint));

        let verifier = pkce_verifier();
        let challenge = pkce_challenge(&verifier);
        let state = base64url(&random_bytes(16));
        let url = authorize_url(
            &metadata.authorization_endpoint,
            &client_id,
            &redirect_uri,
            &scopes,
            &scope_param,
            &challenge,
            &state,
        )?;

        eprintln!("[mcp] authorizing `{}` — opening browser", self.name);
        eprintln!("[mcp] if it does not open, visit:\n{url}");
        open_browser(&url);
        let code = wait_for_code(listener, &state).await?;

        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code),
            ("redirect_uri", redirect_uri.clone()),
            ("client_id", client_id.clone()),
            ("code_verifier", verifier),
        ];
        if let Some(secret) = &self.config.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        let response = self
            .client
            .post(&metadata.token_endpoint)
            .form(&form)
            .send()
            .await
            .context("exchanging OAuth code for a token")?;
        let status = response.status();
        let value = response.json::<Value>().await.unwrap_or(Value::Null);
        let mut auth = parse_token(&value, status)?;
        auth.client_id = Some(client_id);
        auth.token_endpoint = Some(metadata.token_endpoint.clone());
        Ok(auth)
    }

    async fn register_client(&self, metadata: &Metadata, redirect_uri: &str) -> Result<String> {
        let Some(endpoint) = &metadata.registration_endpoint else {
            bail!(
                "`{}` has no clientId configured and the server does not support dynamic client registration",
                self.name
            );
        };
        let payload = json!({
            "client_name": "oxide",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        });
        let response = self
            .client
            .post(endpoint)
            .json(&payload)
            .send()
            .await
            .context("registering OAuth client")?;
        let status = response.status();
        let value = response.json::<Value>().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!("dynamic client registration failed ({status}): {value}");
        }
        value
            .get("client_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("registration response missing client_id")
    }

    async fn metadata(&self) -> Result<Metadata> {
        if let Some(metadata) = self.metadata.lock().await.clone() {
            return Ok(metadata);
        }
        let metadata = discover(&self.client, &self.resource_url).await?;
        *self.metadata.lock().await = Some(metadata.clone());
        Ok(metadata)
    }
}

fn is_expired(auth: &StoredAuth) -> bool {
    match auth.expires_at {
        Some(expires_at) => now() + REFRESH_MARGIN >= expires_at,
        None => false,
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn default_scope_param(authorization_endpoint: &str) -> String {
    if authorization_endpoint.contains("v2_user") {
        "user_scope".to_string()
    } else {
        "scope".to_string()
    }
}

fn authorize_url(
    endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    scopes: &[String],
    scope_param: &str,
    challenge: &str,
    state: &str,
) -> Result<String> {
    let mut url = reqwest::Url::parse(endpoint)
        .with_context(|| format!("invalid authorization endpoint `{endpoint}`"))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", client_id);
        query.append_pair("redirect_uri", redirect_uri);
        query.append_pair("code_challenge", challenge);
        query.append_pair("code_challenge_method", "S256");
        query.append_pair("state", state);
        if !scopes.is_empty() {
            query.append_pair(scope_param, &scopes.join(" "));
        }
    }
    Ok(url.to_string())
}

async fn discover(client: &reqwest::Client, resource_url: &str) -> Result<Metadata> {
    let resource = reqwest::Url::parse(resource_url)
        .with_context(|| format!("invalid MCP url `{resource_url}`"))?;
    let origin = resource.origin().ascii_serialization();
    let mut resource_candidates = vec![format!("{origin}/.well-known/oauth-protected-resource")];
    let path = resource.path();
    if !path.is_empty() && path != "/" {
        resource_candidates.push(format!(
            "{origin}/.well-known/oauth-protected-resource{path}"
        ));
    }

    let mut authorization_servers = Vec::new();
    for candidate in &resource_candidates {
        let Ok(value) = get_json(client, candidate).await else {
            continue;
        };
        if let Some(servers) = value.get("authorization_servers").and_then(Value::as_array) {
            authorization_servers = servers
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            if !authorization_servers.is_empty() {
                break;
            }
        }
    }
    if authorization_servers.is_empty() {
        authorization_servers.push(origin.clone());
    }

    for server in &authorization_servers {
        let Ok(parsed) = reqwest::Url::parse(server) else {
            continue;
        };
        let base = parsed.origin().ascii_serialization();
        for suffix in [
            "/.well-known/oauth-authorization-server",
            "/.well-known/openid-configuration",
        ] {
            let Ok(value) = get_json(client, &format!("{base}{suffix}")).await else {
                continue;
            };
            let (Some(authorization_endpoint), Some(token_endpoint)) = (
                value.get("authorization_endpoint").and_then(Value::as_str),
                value.get("token_endpoint").and_then(Value::as_str),
            ) else {
                continue;
            };
            return Ok(Metadata {
                authorization_endpoint: authorization_endpoint.to_string(),
                token_endpoint: token_endpoint.to_string(),
                registration_endpoint: value
                    .get("registration_endpoint")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                scopes_supported: value
                    .get("scopes_supported")
                    .and_then(Value::as_array)
                    .map(|scopes| {
                        scopes
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
    }

    bail!("could not discover OAuth endpoints for `{resource_url}`")
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value> {
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("GET {url} -> {status}");
    }
    serde_json::from_str(&text).with_context(|| format!("parsing JSON from {url}"))
}

async fn wait_for_code(listener: TcpListener, expected_state: &str) -> Result<String> {
    let (mut socket, _) = tokio::time::timeout(AUTHORIZE_TIMEOUT, listener.accept())
        .await
        .context("timed out waiting for the OAuth callback")?
        .context("accepting the OAuth callback")?;

    let mut buffer = vec![0u8; 8192];
    let read = socket.read(&mut buffer).await.unwrap_or(0);
    let request = String::from_utf8_lossy(&buffer[..read]);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    let params: BTreeMap<String, String> =
        reqwest::Url::parse(&format!("http://localhost{target}"))
            .map(|url| url.query_pairs().into_owned().collect())
            .unwrap_or_default();

    let (status, message) = if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or(error);
        (400, format!("Authorization failed: {description}"))
    } else if params.get("state") != Some(&expected_state.to_string()) {
        (400, "Authorization failed: state mismatch".to_string())
    } else if params.contains_key("code") {
        (
            200,
            "Authorization complete. You can close this tab.".to_string(),
        )
    } else {
        (400, "Authorization failed: missing code".to_string())
    };

    let body = format!(
        "<!doctype html><html><body style=\"font-family:sans-serif;padding:2rem\"><p>{message}</p></body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        if status == 200 { "OK" } else { "Bad Request" },
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;

    if status != 200 {
        bail!("{message}");
    }
    params
        .get("code")
        .cloned()
        .context("authorization response missing code")
}

/// Parses a token response, accepting both standard OAuth fields and Slack's
/// nested `authed_user` object.
fn parse_token(value: &Value, status: reqwest::StatusCode) -> Result<StoredAuth> {
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        let error = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        bail!("token endpoint returned error: {error}");
    }
    let source = value
        .get("authed_user")
        .filter(|v| v.is_object())
        .unwrap_or(value);
    let access_token = source
        .get("access_token")
        .or_else(|| value.get("access_token"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(access_token) = access_token else {
        let error = value
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        bail!("token endpoint returned no access_token ({status}): {error}");
    };
    let expires_in = source
        .get("expires_in")
        .or_else(|| value.get("expires_in"))
        .and_then(Value::as_u64);
    Ok(StoredAuth {
        access_token,
        refresh_token: source
            .get("refresh_token")
            .or_else(|| value.get("refresh_token"))
            .and_then(Value::as_str)
            .map(str::to_string),
        token_type: source
            .get("token_type")
            .or_else(|| value.get("token_type"))
            .and_then(Value::as_str)
            .map(str::to_string),
        scope: source
            .get("scope")
            .or_else(|| value.get("scope"))
            .and_then(Value::as_str)
            .map(str::to_string),
        expires_at: expires_in.map(|seconds| now() + seconds),
        client_id: None,
        token_endpoint: None,
    })
}

fn token_path(name: &str) -> Option<PathBuf> {
    let dir = dirs::config_dir()?.join("oxide").join("mcp-oauth");
    Some(dir.join(format!("{}.json", sanitize(name))))
}

fn load_stored(name: &str) -> Option<StoredAuth> {
    let path = token_path(name)?;
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_stored(name: &str, auth: &StoredAuth) -> Result<()> {
    let path = token_path(name).context("no config directory available")?;
    write_private(&path, &serde_json::to_string_pretty(auth)?)
}

fn write_private(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut buffer = vec![0u8; len];
    getrandom::getrandom(&mut buffer).expect("gathering randomness");
    buffer
}

fn pkce_verifier() -> String {
    base64url(&random_bytes(32))
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64url(&digest)
}

fn base64url(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        output.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        output.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[(triple & 0x3f) as usize] as char);
        }
    }
    output
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(not(any(unix, target_os = "windows")))]
    let result: std::io::Result<std::process::Child> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "unsupported platform",
    ));
    if let Err(err) = result {
        eprintln!("[mcp] could not open a browser: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_slack_pkce_vector() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn pkce_challenge_matches_slack_example() {
        assert_eq!(
            pkce_challenge("secretpassword"),
            "ldMBaaWcQYtSATMV_IG8mf3wp7A6EW80arYoSW80ntU"
        );
    }

    #[test]
    fn builds_authorize_url_with_pkce_and_state() {
        let url = authorize_url(
            "https://slack.com/oauth/v2_user/authorize",
            "123.456",
            "http://localhost:3118/callback",
            &["chat:write".to_string(), "search:read.public".to_string()],
            "user_scope",
            "challenge",
            "state123",
        )
        .unwrap();
        assert!(url.starts_with("https://slack.com/oauth/v2_user/authorize?"));
        assert!(url.contains("client_id=123.456"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state123"));
        assert!(url.contains("user_scope=chat%3Awrite+search%3Aread.public"));
    }

    #[test]
    fn detects_slack_user_scope_parameter() {
        assert_eq!(
            default_scope_param("https://slack.com/oauth/v2_user/authorize"),
            "user_scope"
        );
        assert_eq!(
            default_scope_param("https://example.com/oauth/authorize"),
            "scope"
        );
    }

    #[test]
    fn parses_standard_and_slack_token_responses() {
        let standard = json!({
            "access_token": "abc",
            "refresh_token": "def",
            "expires_in": 3600,
            "token_type": "Bearer",
            "scope": "read"
        });
        let parsed = parse_token(&standard, reqwest::StatusCode::OK).unwrap();
        assert_eq!(parsed.access_token, "abc");
        assert_eq!(parsed.refresh_token.as_deref(), Some("def"));
        assert!(parsed.expires_at.unwrap() > now());

        let slack = json!({
            "ok": true,
            "access_token": "xoxb-bot",
            "authed_user": {
                "access_token": "xoxp-user",
                "refresh_token": "xoxe-refresh",
                "expires_in": 43200,
                "scope": "chat:write"
            }
        });
        let parsed = parse_token(&slack, reqwest::StatusCode::OK).unwrap();
        assert_eq!(parsed.access_token, "xoxp-user");
        assert_eq!(parsed.refresh_token.as_deref(), Some("xoxe-refresh"));
        assert_eq!(parsed.scope.as_deref(), Some("chat:write"));
    }

    #[test]
    fn reports_token_errors() {
        let error = json!({ "ok": false, "error": "invalid_code" });
        let err = parse_token(&error, reqwest::StatusCode::OK).unwrap_err();
        assert!(err.to_string().contains("invalid_code"));
    }

    #[test]
    fn expiry_respects_refresh_margin() {
        let mut auth = StoredAuth {
            access_token: "a".into(),
            expires_at: Some(now() + 10),
            ..Default::default()
        };
        assert!(is_expired(&auth));
        auth.expires_at = Some(now() + 3600);
        assert!(!is_expired(&auth));
        auth.expires_at = None;
        assert!(!is_expired(&auth));
    }

    #[test]
    fn sanitizes_server_names_for_token_files() {
        assert_eq!(sanitize("slack"), "slack");
        assert_eq!(sanitize("my/server name"), "my_server_name");
    }
}
