//! Keeps a conversation's prompt cache warm while the reader is idle.
//!
//! A provider caches the prefix a request sent and charges a fraction of the
//! input price when the next request reuses it, but the entry expires after a
//! few minutes (Anthropic's short tier is five). Pi replays the last request
//! with a one-token output cap just before the entry expires, so the next turn
//! starts from a warm cache instead of paying full input for a long transcript.
//!
//! The warmer is deliberately conservative: it only runs for a model whose
//! catalog publishes a `promptCache` lifetime, only for a conversation large
//! enough for the saved input to be worth the replay, and only for a short idle
//! window. Any new model request cancels it (the new request writes the entry
//! itself), and a request that cannot be replayed safely — a non-adaptive
//! Anthropic model whose thinking budget is derived from `max_tokens` — is left
//! alone, matching Pi's `isReplayable` rule.
//!
//! `cacheWarming` in `settings.json` (the project's `.oxide/settings.json`
//! winning per key, `OXIDE_CACHE_WARMING` overriding both) turns it off.

use crate::config::{Config, ProviderKind, Reasoning};
use crate::llm::{LlmClient, Message, Retry, StreamHooks, ToolSpec};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Notify;

/// How long one idle window may be kept warm. Pi uses a shorter horizon for
/// idle warming because a continuation estimate gets less reliable with age.
const MAX_WARMING_AGE: Duration = Duration::from_secs(30 * 60);
/// How many refreshes one idle window may spend. A five-minute cache is warmed
/// a few times; a reader away longer starts from a cold cache.
const MAX_REFRESHES: u32 = 3;
/// A conversation smaller than this is not worth a replay: the cache-read
/// saving Pi's economics gate asks for is around this many input tokens.
const MIN_WARM_TOKENS: u64 = 20_000;
/// A cache lifetime this short is not worth scheduling for; Pi declines the
/// same way (its delay margin is ten seconds).
const MIN_CACHE_TTL: Duration = Duration::from_secs(10);

/// Bumped by every new request and every re-arm, so a task that was sleeping
/// when the conversation moved on stops instead of replaying a stale request.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static WAKE: OnceLock<Notify> = OnceLock::new();

fn wake() -> &'static Notify {
    WAKE.get_or_init(Notify::new)
}

/// Whether `cacheWarming` allows it. The project's settings win over the global
/// file, and `OXIDE_CACHE_WARMING` overrides both.
pub fn enabled(cwd: &Path) -> bool {
    std::env::var("OXIDE_CACHE_WARMING")
        .ok()
        .and_then(|text| text.trim().parse::<bool>().ok())
        .or_else(|| resolve_enabled(&settings_paths(cwd)))
        .unwrap_or(true)
}

fn settings_paths(cwd: &Path) -> Vec<PathBuf> {
    vec![
        crate::config::settings_path(),
        cwd.join(".oxide").join("settings.json"),
    ]
}

/// The last file that states an opinion wins, so the project's setting beats
/// the global one.
fn resolve_enabled(paths: &[PathBuf]) -> Option<bool> {
    paths.iter().rev().find_map(|path| read_enabled(path))
}

fn read_enabled(path: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    value.get("cacheWarming")?.as_bool()
}

/// Stops any scheduled warm. Called before every model request, which writes
/// the cache entry itself.
pub fn cancel() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    wake().notify_waiters();
}

/// How long to wait before replaying a cache entry with `ttl`. Ninety percent
/// of the lifetime, with at least ten seconds of margin, as Pi does.
fn warm_delay(ttl: Duration) -> Option<Duration> {
    if ttl <= MIN_CACHE_TTL {
        return None;
    }
    Some(ttl.mul_f64(0.9).min(ttl - MIN_CACHE_TTL))
}

/// The delay a scheduled warm uses. `OXIDE_CACHE_WARM_DELAY_MS` shortens it for
/// a test (and for a caller that wants a tighter interval than the default).
fn scheduled_delay(ttl: Duration) -> Option<Duration> {
    if let Some(millis) = std::env::var("OXIDE_CACHE_WARM_DELAY_MS")
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
    {
        return Some(Duration::from_millis(millis));
    }
    warm_delay(ttl)
}

/// Arms the warmer for the conversation a finished turn left, if the active
/// model's cache has a lifetime the catalog publishes.
pub fn schedule(
    config: &Config,
    cwd: &Path,
    request: Vec<Message>,
    tools: Vec<ToolSpec>,
    session_id: Option<String>,
    session_log: Option<Arc<crate::session::SessionLog>>,
) {
    cancel();
    let Some(ttl) = config.prompt_cache_ttl() else {
        return;
    };
    let Some(delay) = scheduled_delay(ttl) else {
        return;
    };
    if !enabled(cwd) || !replayable(config) {
        return;
    }
    if estimate_tokens(&request, &tools) < MIN_WARM_TOKENS {
        return;
    }
    let config = config.clone();
    let generation = GENERATION.load(Ordering::SeqCst);
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + MAX_WARMING_AGE;
        for _ in 0..MAX_REFRESHES {
            tokio::select! {
                _ = wake().notified() => return,
                _ = tokio::time::sleep(delay) => {}
            }
            if GENERATION.load(Ordering::SeqCst) != generation
                || tokio::time::Instant::now() >= deadline
            {
                return;
            }
            tokio::select! {
                _ = wake().notified() => return,
                result = warm_once(&config, &request, &tools, session_id.as_deref()) => {
                    match result {
                        Ok(usage) => {
                            if let Some(log) = &session_log {
                                if let Err(error) = log.append_background_usage(usage.into()) {
                                    crate::notice::warn(format!("saving cache warming usage: {error:#}"));
                                }
                            }
                        }
                        Err(_) => return,
                    }
                }
            }
        }
    });
}

/// Whether replaying this model's request with a one-token cap leaves the cache
/// entry intact. Anthropic derives a thinking budget from `max_tokens` on the
/// models that do not use adaptive thinking, so a replay would write a
/// different entry (and could think for thousands of tokens) instead.
fn replayable(config: &Config) -> bool {
    if config.provider_kind() != ProviderKind::Anthropic {
        return true;
    }
    match config.effective_reasoning() {
        Reasoning::Auto | Reasoning::Off => true,
        _ => crate::config::supports_adaptive_thinking(&config.model),
    }
}

/// Estimates what a request costs, in tokens, closely enough to decide whether
/// warming it is worthwhile.
fn estimate_tokens(request: &[Message], tools: &[ToolSpec]) -> u64 {
    let messages: usize = request
        .iter()
        .map(|message| message.display().map_or(0, |text| text.len()))
        .sum();
    let tools = serde_json::to_string(tools).map_or(0, |text| text.len());
    ((messages + tools) / 4) as u64
}

/// Replays one request with a one-token output cap. Its text is discarded;
/// reported usage is returned for accounting.
pub async fn warm_once(
    config: &Config,
    request: &[Message],
    tools: &[ToolSpec],
    session_id: Option<&str>,
) -> anyhow::Result<crate::llm::Usage> {
    let mut warm = config.clone();
    warm.max_tokens = 1;
    // A cap this call chose, not a reader's; without this the catalog's output
    // limit would raise it back.
    warm.max_tokens_explicit = true;
    let client = match session_id {
        Some(id) => LlmClient::new(warm).with_session_id(id.to_string()),
        None => LlmClient::new(warm),
    };
    let mut ignore_text = |_: String| {};
    let mut ignore_thinking = |_: String| {};
    let mut ignore_retry = |_: Retry| {};
    let mut hooks = StreamHooks {
        text: &mut ignore_text,
        thinking: &mut ignore_thinking,
        retry: &mut ignore_retry,
    };
    client.stream_cache_warm(request, tools, &mut hooks).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn a_cache_lifetime_gets_a_safe_margin() {
        assert_eq!(warm_delay(Duration::from_secs(5)), None);
        assert_eq!(
            warm_delay(Duration::from_secs(300)),
            Some(Duration::from_secs(270))
        );
        // A lifetime just over the margin keeps ten seconds rather than none.
        assert_eq!(
            warm_delay(Duration::from_secs(12)),
            Some(Duration::from_secs(2))
        );
    }

    #[test]
    fn the_project_setting_wins_over_the_global_one() {
        let base = std::env::temp_dir().join(format!("oxide_warm_enabled_{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let global = settings(&base, "settings.json", r#"{"cacheWarming": false}"#);
        let project = settings(
            &base.join("project"),
            ".oxide/settings.json",
            r#"{"cacheWarming": true}"#,
        );

        assert_eq!(read_enabled(&global), Some(false));
        assert_eq!(read_enabled(&project), Some(true));
        assert_eq!(read_enabled(&base.join("missing.json")), None);

        // The project's file is later in the list, so it decides.
        assert_eq!(resolve_enabled(&[global, project]), Some(true));

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_non_adaptive_anthropic_budget_is_not_replayed() {
        let mut config = Config {
            provider: "anthropic".into(),
            model: "claude-haiku-4-5".into(),
            ..Config::default()
        };
        config.reasoning = Reasoning::High;
        assert!(!replayable(&config));
        config.reasoning = Reasoning::Auto;
        assert!(replayable(&config));

        let adaptive = Config {
            provider: "anthropic".into(),
            model: "claude-sonnet-5".into(),
            reasoning: Reasoning::High,
            ..Config::default()
        };
        assert!(replayable(&adaptive));

        let openai = Config {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            reasoning: Reasoning::High,
            ..Config::default()
        };
        assert!(replayable(&openai));
    }

    #[tokio::test]
    async fn a_truncated_warm_replay_asks_for_one_token_without_retrying() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
            let mut raw = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let read = socket.read(&mut chunk).await.unwrap_or(0);
                if read == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&raw);
                let Some(head_end) = text.find("\r\n\r\n") else {
                    continue;
                };
                let expected = text[..head_end].lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
                if expected.is_some_and(|len| raw.len() - (head_end + 4) >= len) {
                    break;
                }
            }
            String::from_utf8_lossy(&raw).into_owned()
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
            request
        });

        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Config::default()
        };
        tokio::time::timeout(
            Duration::from_secs(2),
            warm_once(&config, &[Message::user("hello")], &[], None),
        )
        .await
        .expect("a truncated warm-up must not enter the normal retry loop")
        .expect("intentional truncation still refreshes the cache");
        let request = server.await.unwrap();
        assert!(
            request.contains("\"max_tokens\":1") || request.contains("\"max_tokens\": 1"),
            "{request}"
        );
    }

    #[test]
    fn a_request_is_estimated_from_its_text_and_tools() {
        let long = "x".repeat(80_000);
        let request = vec![Message::user(long)];
        assert!(estimate_tokens(&request, &[]) >= MIN_WARM_TOKENS);
        assert!(estimate_tokens(&[Message::user("hi")], &[]) < MIN_WARM_TOKENS);
    }
}
