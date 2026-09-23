//! OAuth 2.0 (authorization code + PKCE) support for remote MCP servers.
//!
//! Remote MCP servers can require the client to obtain a bearer token before
//! calling `tools/*`. This module discovers the authorization
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
    #[serde(default)]
    pub resource_url: Option<String>,
}

#[derive(Debug, Clone)]
struct Metadata {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    resource_scopes_supported: Vec<String>,
}

/// Shared OAuth state for one remote server. Cheap to clone via `Arc`.
pub struct OAuthState {
    name: String,
    config: McpOAuth,
    resource_url: String,
    client: reqwest::Client,
    auth: Mutex<Option<StoredAuth>>,
    metadata: Mutex<Option<Metadata>>,
    resource_metadata_url: Mutex<Option<String>>,
    challenge_scopes: Mutex<Vec<String>>,
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
            auth: Mutex::new(
                load_stored(name).filter(|auth| auth.resource_url.as_deref() == Some(resource_url)),
            ),
            metadata: Mutex::new(None),
            resource_metadata_url: Mutex::new(None),
            challenge_scopes: Mutex::new(Vec::new()),
        }
    }

    /// Records OAuth discovery hints from a 401 `WWW-Authenticate` challenge.
    pub async fn note_unauthorized(&self, headers: &reqwest::header::HeaderMap) {
        if let Some(auth) = self.auth.lock().await.as_mut() {
            auth.expires_at = Some(0);
        }
        let challenges = headers
            .get_all(reqwest::header::WWW_AUTHENTICATE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>()
            .join(",");
        if let Some(url) = challenge_parameter(&challenges, "resource_metadata") {
            *self.resource_metadata_url.lock().await = Some(url);
            *self.metadata.lock().await = None;
        }
        if let Some(scope) = challenge_parameter(&challenges, "scope") {
            *self.challenge_scopes.lock().await =
                scope.split_whitespace().map(str::to_string).collect();
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

    /// Returns a stored token when available without starting an interactive flow.
    pub async fn access_token_if_available(&self) -> Option<String> {
        if let Some(token) = self.valid_token().await {
            return Some(token);
        }
        if self.try_refresh().await {
            return self.valid_token().await;
        }
        None
    }

    async fn valid_token(&self) -> Option<String> {
        let guard = self.auth.lock().await;
        guard
            .as_ref()
            .filter(|auth| !is_expired(auth))
            .map(|auth| auth.access_token.clone())
    }

    async fn store(&self, mut auth: StoredAuth) {
        auth.resource_url = Some(self.resource_url.clone());
        if let Err(err) = save_stored(&self.name, &auth) {
            eprintln!(
                "[mcp] failed to store OAuth token for `{}`: {err:#}",
                self.name
            );
        }
        *self.auth.lock().await = Some(auth);
    }

    /// Drops the stored credentials after the provider permanently rejects
    /// them, so later status checks report `Needs Auth` instead of retrying a
    /// refresh token that can never work again.
    async fn clear(&self) {
        if let Some(path) = token_path(&self.name) {
            let _ = std::fs::remove_file(path);
        }
        *self.auth.lock().await = None;
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
            ("resource", self.resource_url.clone()),
        ];
        if let Some(secret) = &self.config.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        let response = match self.client.post(&token_endpoint).form(&form).send().await {
            Ok(response) => response,
            Err(_) => return false,
        };
        let status = response.status();
        let value = response.json::<Value>().await.unwrap_or(Value::Null);
        let mut auth = match parse_token(&value, status) {
            Ok(auth) => auth,
            Err(_) => {
                if refresh_token_rejected(&value) {
                    self.clear().await;
                }
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
            let challenge_scopes = self.challenge_scopes.lock().await.clone();
            if challenge_scopes.is_empty() {
                metadata.resource_scopes_supported.clone()
            } else {
                challenge_scopes
            }
        } else {
            self.config.scopes.clone()
        };
        let scope_param = self
            .config
            .scope_param
            .clone()
            .unwrap_or_else(|| "scope".to_string());

        let verifier = pkce_verifier();
        let challenge = pkce_challenge(&verifier);
        let state = base64url(&random_bytes(16));
        let url = authorize_url(AuthorizationRequest {
            endpoint: &metadata.authorization_endpoint,
            client_id: &client_id,
            redirect_uri: &redirect_uri,
            scopes: &scopes,
            scope_param: &scope_param,
            challenge: &challenge,
            state: &state,
            resource: &self.resource_url,
        })?;

        eprintln!("[mcp] authorizing `{}` — opening browser", self.name);
        eprintln!("[mcp] if it does not open, visit:\n{url}");
        open_browser(&url);
        let code = wait_for_code(listener, &state, &self.name).await?;

        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code),
            ("redirect_uri", redirect_uri.clone()),
            ("client_id", client_id.clone()),
            ("code_verifier", verifier),
            ("resource", self.resource_url.clone()),
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
            "client_name": "Oxide",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "application_type": "native",
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
        let resource_metadata_url = self.resource_metadata_url.lock().await.clone();
        let metadata = discover(
            &self.client,
            &self.resource_url,
            resource_metadata_url.as_deref(),
        )
        .await?;
        *self.metadata.lock().await = Some(metadata.clone());
        Ok(metadata)
    }
}

/// Whether a token-endpoint error payload means the stored refresh token can
/// never be used again (for example `invalid_grant` or `access_denied`).
fn refresh_token_rejected(value: &Value) -> bool {
    matches!(
        value.get("error").and_then(Value::as_str),
        Some("invalid_grant" | "invalid_token" | "expired_token" | "access_denied")
    )
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

fn challenge_parameter(challenge: &str, name: &str) -> Option<String> {
    let lower = challenge.to_ascii_lowercase();
    let needle = format!("{}=", name.to_ascii_lowercase());
    for (index, _) in lower.match_indices(&needle) {
        if index > 0 {
            let previous = lower[..index].chars().next_back()?;
            if previous != ',' && !previous.is_ascii_whitespace() {
                continue;
            }
        }
        let rest = challenge[index + needle.len()..].trim_start();
        if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"')?;
            return Some(quoted[..end].to_string());
        }
        let end = rest
            .find(|character: char| character == ',' || character.is_ascii_whitespace())
            .unwrap_or(rest.len());
        if end > 0 {
            return Some(rest[..end].to_string());
        }
    }
    None
}

struct AuthorizationRequest<'a> {
    endpoint: &'a str,
    client_id: &'a str,
    redirect_uri: &'a str,
    scopes: &'a [String],
    scope_param: &'a str,
    challenge: &'a str,
    state: &'a str,
    resource: &'a str,
}

fn authorize_url(request: AuthorizationRequest<'_>) -> Result<String> {
    let mut url = reqwest::Url::parse(request.endpoint)
        .with_context(|| format!("invalid authorization endpoint `{}`", request.endpoint))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", request.client_id);
        query.append_pair("redirect_uri", request.redirect_uri);
        query.append_pair("code_challenge", request.challenge);
        query.append_pair("code_challenge_method", "S256");
        query.append_pair("state", request.state);
        query.append_pair("resource", request.resource);
        if !request.scopes.is_empty() {
            query.append_pair(request.scope_param, &request.scopes.join(" "));
        }
    }
    Ok(url.to_string())
}

async fn discover(
    client: &reqwest::Client,
    resource_url: &str,
    resource_metadata_url: Option<&str>,
) -> Result<Metadata> {
    let resource = reqwest::Url::parse(resource_url)
        .with_context(|| format!("invalid MCP url `{resource_url}`"))?;
    let origin = resource.origin().ascii_serialization();
    let mut resource_candidates = Vec::new();
    if let Some(url) = resource_metadata_url {
        resource_candidates.push(url.to_string());
    }
    let path = resource.path();
    if !path.is_empty() && path != "/" {
        resource_candidates.push(format!(
            "{origin}/.well-known/oauth-protected-resource{path}"
        ));
    }
    resource_candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));

    let mut authorization_servers = Vec::new();
    let mut resource_scopes_supported = Vec::new();
    for candidate in &resource_candidates {
        let Ok(value) = get_json(client, candidate).await else {
            continue;
        };
        if let Some(scopes) = value.get("scopes_supported").and_then(Value::as_array) {
            resource_scopes_supported = scopes
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
        }
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
                resource_scopes_supported,
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

async fn wait_for_code(
    listener: TcpListener,
    expected_state: &str,
    server: &str,
) -> Result<String> {
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

    let (status, page, failure) = if let Some(error) = params.get("error") {
        let detail = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or(error);
        (
            400,
            callback_error(server, detail),
            Some(format!("Authorization failed: {detail}")),
        )
    } else if params.get("state").map(String::as_str) != Some(expected_state) {
        (
            400,
            callback_error(
                server,
                "The response did not match this request (state mismatch).",
            ),
            Some("Authorization failed: state mismatch".to_string()),
        )
    } else if params.contains_key("code") {
        (200, callback_success(server), None)
    } else {
        (
            400,
            callback_error(server, "The authorization response did not include a code."),
            Some("Authorization failed: missing code".to_string()),
        )
    };

    let response = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{page}",
        if status == 200 { "OK" } else { "Bad Request" },
        page.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;

    if let Some(failure) = failure {
        bail!("{failure}");
    }
    params
        .get("code")
        .cloned()
        .context("authorization response missing code")
}

// Design tokens for the loopback callback page. The default scheme is light;
// dark applies through `prefers-color-scheme`.
const CALLBACK_STYLE: &str = r#":root {
  color-scheme: light dark;
  --ox-bg: #f6f7f9;
  --ox-card: #fdfdfd;
  --ox-text-strong: #16181d;
  --ox-text-base: #5f6572;
  --ox-text-weak: #878e9a;
  --ox-border: #e4e7ec;
  --ox-success: #1a9e4b;
  --ox-error: #d3382c;
  --ox-detail-bg: #fff7f5;
  --ox-detail-border: #f6c4bc;
  --ox-shadow: 0 16px 48px -6px rgb(15 23 42 / 10%), 0 6px 12px -2px rgb(15 23 42 / 5%), 0 1px 2px rgb(15 23 42 / 6%);
  --ox-font-sans: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  --ox-font-mono: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace;
}
@media (prefers-color-scheme: dark) {
  :root {
    --ox-bg: #0d0f13;
    --ox-card: #16181d;
    --ox-text-strong: rgb(255 255 255 / 94%);
    --ox-text-base: rgb(255 255 255 / 62%);
    --ox-text-weak: rgb(255 255 255 / 42%);
    --ox-border: #262a32;
    --ox-success: #37c76a;
    --ox-error: #ff6b5a;
    --ox-detail-bg: #2a1410;
    --ox-detail-border: #5c2118;
    --ox-shadow: 0 16px 48px -6px rgb(0 0 0 / 55%), 0 6px 12px -2px rgb(0 0 0 / 35%), 0 1px 2px rgb(0 0 0 / 40%);
  }
}
* { box-sizing: border-box; }
html, body { margin: 0; height: 100%; }
body {
  display: grid;
  place-items: center;
  padding: 24px;
  background: var(--ox-bg);
  color: var(--ox-text-base);
  font-family: var(--ox-font-sans);
  font-size: 16px;
  line-height: 1.5;
  -webkit-font-smoothing: antialiased;
}
.card {
  width: min(100%, 28rem);
  padding: 2.25rem 2rem 1.75rem;
  text-align: center;
  background: var(--ox-card);
  border: 1px solid var(--ox-border);
  border-radius: 14px;
  box-shadow: var(--ox-shadow);
}
.brand {
  display: flex;
  justify-content: center;
  margin-bottom: 1.75rem;
}
.wordmark {
  display: inline-block;
  margin-right: -0.28em;
  font-size: 0.8125rem;
  font-weight: 600;
  letter-spacing: 0.28em;
  text-transform: uppercase;
  color: var(--ox-text-strong);
}
.status {
  display: flex;
  justify-content: center;
  margin-bottom: 1.125rem;
  line-height: 0;
}
.status svg { display: block; }
.card[data-status="success"] .icon-success { color: var(--ox-success); }
.card[data-status="error"] .icon-error { color: var(--ox-error); }
.headline {
  margin: 0;
  font-size: 1.1875rem;
  font-weight: 600;
  line-height: 1.3;
  letter-spacing: -0.012em;
  color: var(--ox-text-strong);
}
.message { margin: 0.5rem 0 0; font-size: 0.9375rem; }
.detail {
  margin: 1.25rem 0 0;
  padding: 0.75rem 0.875rem;
  overflow: auto;
  max-height: 9.5rem;
  font-family: var(--ox-font-mono);
  font-size: 0.8125rem;
  line-height: 1.55;
  text-align: left;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  color: var(--ox-text-strong);
  background: var(--ox-detail-bg);
  border: 1px solid var(--ox-detail-border);
  border-radius: 8px;
}
.footnote { margin: 1.5rem 0 0; font-size: 0.8125rem; color: var(--ox-text-weak); }
"#;

// The block-letter wordmark from the TUI banner is font-dependent, so the page
// uses a letter-spaced text mark instead of an SVG.
const WORDMARK: &str = r#"<span class="wordmark" role="img" aria-label="Oxide">Oxide</span>"#;

const ICON_CHECK: &str = r#"<svg viewBox="0 0 24 24" width="30" height="30" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="m8.5 12.5 2.4 2.4 4.6-5.4"/></svg>"#;

const ICON_CROSS: &str = r#"<svg viewBox="0 0 24 24" width="30" height="30" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="m9 9 6 6m0-6-6 6"/></svg>"#;

// Best-effort close: browsers only honor it for script-opened windows.
const AUTO_CLOSE_SCRIPT: &str = "setTimeout(function(){try{window.close()}catch(e){}},2500)";

fn callback_success(server: &str) -> String {
    render_document(
        "Authorization successful",
        &render_card(Card {
            status: "success",
            headline: "Authorization successful",
            message: &format!("Oxide is now connected to {}.", escape_html(server)),
            detail: None,
            footnote: "You can close this window.",
        }),
        Some(AUTO_CLOSE_SCRIPT),
    )
}

fn callback_error(server: &str, detail: &str) -> String {
    render_document(
        "Authorization failed",
        &render_card(Card {
            status: "error",
            headline: "Authorization failed",
            message: &format!(
                "Oxide could not finish connecting to {}.",
                escape_html(server)
            ),
            detail: Some(detail),
            footnote: "Close this window and try again from Oxide.",
        }),
        None,
    )
}

struct Card<'a> {
    status: &'a str,
    headline: &'a str,
    message: &'a str,
    detail: Option<&'a str>,
    footnote: &'a str,
}

fn render_card(card: Card<'_>) -> String {
    let icon = if card.status == "success" {
        ICON_CHECK
    } else {
        ICON_CROSS
    };
    let detail = card.detail.map(str::trim).filter(|text| !text.is_empty());
    let detail_block = match detail {
        Some(text) => format!("<pre class=\"detail\">{}</pre>\n", escape_html(text)),
        None => String::new(),
    };
    format!(
        concat!(
            "<main class=\"card\" data-status=\"{status}\" role=\"status\" aria-live=\"polite\">\n",
            "<div class=\"brand\">{wordmark}</div>\n",
            "<div class=\"status icon-{status}\">{icon}</div>\n",
            "<h1 class=\"headline\">{headline}</h1>\n",
            "<p class=\"message\">{message}</p>\n",
            "{detail}",
            "<p class=\"footnote\">{footnote}</p>\n",
            "</main>",
        ),
        status = card.status,
        wordmark = WORDMARK,
        icon = icon,
        headline = escape_html(card.headline),
        message = card.message,
        detail = detail_block,
        footnote = escape_html(card.footnote),
    )
}

fn render_document(title: &str, body: &str, script: Option<&str>) -> String {
    let script = script
        .map(|script| format!("\n  <script>{script}</script>"))
        .unwrap_or_default();
    format!(
        concat!(
            "<!doctype html>\n",
            "<html lang=\"en\">\n",
            "<head>\n",
            "  <meta charset=\"utf-8\">\n",
            "  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n",
            "  <meta name=\"robots\" content=\"noindex\">\n",
            "  <title>{title} · Oxide</title>\n",
            "  <style>{style}</style>\n",
            "</head>\n",
            "<body>\n",
            "  {body}{script}\n",
            "</body>\n",
            "</html>\n",
        ),
        title = escape_html(title),
        style = CALLBACK_STYLE,
        body = body,
        script = script,
    )
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Parses a standard OAuth token response.
fn parse_token(value: &Value, status: reqwest::StatusCode) -> Result<StoredAuth> {
    let access_token = value
        .get("access_token")
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
    let expires_in = value.get("expires_in").and_then(Value::as_u64);
    Ok(StoredAuth {
        access_token,
        refresh_token: value
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        token_type: value
            .get("token_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        scope: value
            .get("scope")
            .and_then(Value::as_str)
            .map(str::to_string),
        expires_at: expires_in.map(|seconds| now() + seconds),
        client_id: None,
        token_endpoint: None,
        resource_url: None,
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
    fn base64url_encodes_without_padding() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn pkce_challenge_matches_known_vector() {
        assert_eq!(
            pkce_challenge("secretpassword"),
            "ldMBaaWcQYtSATMV_IG8mf3wp7A6EW80arYoSW80ntU"
        );
    }

    #[test]
    fn builds_authorize_url_with_pkce_and_state() {
        let url = authorize_url(AuthorizationRequest {
            endpoint: "https://auth.example.com/oauth/authorize",
            client_id: "client-123",
            redirect_uri: "http://localhost:3000/callback",
            scopes: &["files:read".to_string(), "files:write".to_string()],
            scope_param: "scope",
            challenge: "challenge",
            state: "state123",
            resource: "https://mcp.example.com/mcp",
        })
        .unwrap();
        assert!(url.starts_with("https://auth.example.com/oauth/authorize?"));
        assert!(url.contains("client_id=client-123"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state123"));
        assert!(url.contains("resource=https%3A%2F%2Fmcp.example.com%2Fmcp"));
        assert!(url.contains("scope=files%3Aread+files%3Awrite"));
    }

    #[test]
    fn parses_oauth_challenge_parameters() {
        let challenge = concat!(
            "Bearer resource_metadata=\"https://mcp.example.com/auth?scope=ignored\", ",
            "scope=\"files:read files:write\""
        );
        assert_eq!(
            challenge_parameter(challenge, "resource_metadata").as_deref(),
            Some("https://mcp.example.com/auth?scope=ignored")
        );
        assert_eq!(
            challenge_parameter(challenge, "scope").as_deref(),
            Some("files:read files:write")
        );
    }

    #[test]
    fn callback_pages_brand_each_outcome() {
        let success = callback_success("context7");
        assert!(success.starts_with("<!doctype html>"));
        assert!(success.contains("<title>Authorization successful · Oxide</title>"));
        assert!(success.contains("data-status=\"success\""));
        assert!(success.contains("Oxide is now connected to context7."));
        assert!(success.contains("aria-label=\"Oxide\""));
        assert!(success.contains("window.close"));

        let failure = callback_error("context7", "invalid <client_id> & \"id\"");
        assert!(failure.contains("<title>Authorization failed · Oxide</title>"));
        assert!(failure.contains("data-status=\"error\""));
        assert!(failure.contains("Oxide could not finish connecting to context7."));
        assert!(failure.contains(
            "<pre class=\"detail\">invalid &lt;client_id&gt; &amp; &quot;id&quot;</pre>"
        ));
        assert!(!failure.contains("window.close"));
    }

    #[test]
    fn callback_error_omits_an_empty_detail_block() {
        let page = callback_error("mcp", "   ");
        assert!(!page.contains("class=\"detail\""));
        assert!(page.contains("mcp"));
    }

    #[test]
    fn callback_page_escapes_text() {
        assert_eq!(
            escape_html("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&#39;f"
        );
        assert!(callback_success("<script>").contains("&lt;script&gt;"));
    }

    #[tokio::test]
    async fn discovery_uses_protected_resource_scopes() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server_origin = origin.clone();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = vec![0; 4096];
                let read = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..read]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap();
                let body = if path == "/.well-known/oauth-protected-resource/mcp" {
                    json!({
                        "authorization_servers": [server_origin],
                        "scopes_supported": ["resource:read", "offline_access"]
                    })
                } else {
                    json!({
                        "authorization_endpoint": format!("{server_origin}/authorize"),
                        "token_endpoint": format!("{server_origin}/token"),
                        "scopes_supported": ["openid", "resource:read", "offline_access"]
                    })
                }
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let metadata = discover(&reqwest::Client::new(), &format!("{origin}/mcp"), None)
            .await
            .unwrap();
        server.await.unwrap();

        assert_eq!(
            metadata.resource_scopes_supported,
            ["resource:read", "offline_access"]
        );
    }

    #[test]
    fn parses_standard_token_response() {
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
    }

    #[test]
    fn reports_token_errors() {
        let error = json!({ "ok": false, "error": "invalid_code" });
        let err = parse_token(&error, reqwest::StatusCode::OK).unwrap_err();
        assert!(err.to_string().contains("invalid_code"));
    }

    #[test]
    fn detects_rejected_refresh_tokens() {
        assert!(refresh_token_rejected(&json!({ "error": "invalid_grant" })));
        assert!(refresh_token_rejected(&json!({ "error": "access_denied" })));
        assert!(!refresh_token_rejected(&json!({ "error": "server_error" })));
        assert!(!refresh_token_rejected(&json!({})));
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
        assert_eq!(sanitize("atlassian"), "atlassian");
        assert_eq!(sanitize("my/server name"), "my_server_name");
    }
}
