use crate::config::Config;
use crate::ecosystem::AgentMode;
use crate::llm::{FunctionSpec, LlmClient, Message, Retry, StreamHooks, ToolCall, ToolSpec};
use crate::lsp::LspManager;
use crate::mcp::McpRegistry;
use crate::memory::QueryScope;
use crate::permission::{subject_for, Action, Permissions};
use crate::plugin::PluginHost;
use crate::session::SessionLog;
use crate::snapshots::Snapshots;
use crate::tools;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

const MAX_TASK_DEPTH: usize = 2;

pub type RunFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

pub type Approver =
    Arc<dyn Fn(String, String) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

pub struct ApprovalRequest {
    pub tool: String,
    pub detail: String,
    pub respond: tokio::sync::oneshot::Sender<bool>,
}

/// A queue of user messages typed while the agent is busy. They are injected
/// into the conversation between steps, so the model sees the guidance without
/// interrupting the in-flight tool batch.
#[derive(Clone, Default)]
pub struct Steering {
    queue: Arc<Mutex<Vec<Message>>>,
}

impl Steering {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, message: Message) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.push(message);
        }
    }

    pub fn drain(&self) -> Vec<Message> {
        self.queue
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.queue.lock().map(|queue| queue.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A cooperative cancellation flag shared with a running turn. Setting it makes
/// the loop stop at the next step boundary (finishing the in-flight model call
/// and tool batch), so the session is left in a valid state rather than torn
/// mid-entry.
#[derive(Clone, Default)]
pub struct Cancel {
    flag: Arc<AtomicBool>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
pub struct Runtime {
    pub mcp: Arc<McpRegistry>,
    pub plugins: Arc<PluginHost>,
    pub session: Option<Arc<SessionLog>>,
    pub snapshots: Option<Arc<Snapshots>>,
    pub lsp: Arc<LspManager>,
    pub approve: Approver,
    pub steering: Steering,
    pub follow_ups: Steering,
    pub cancel: Cancel,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Text(String),
    /// A fragment of the model's reasoning, streamed before its answer.
    ThinkingDelta(String),
    Thought {
        millis: u64,
    },
    /// The total wall-clock time the current model step took, sent once the
    /// stream finishes so the `Thought` line can be updated.
    ThoughtDone {
        millis: u64,
    },
    /// A transient provider failure that is about to be retried.
    Retrying {
        attempt: u32,
        max: u32,
        delay_ms: u64,
    },
    ToolCall {
        name: String,
        args: String,
    },
    /// A nested subagent's current tool call. A `task` run otherwise emits
    /// nothing until it returns, which is indistinguishable from a hang.
    SubagentActivity {
        agent: String,
        tool: String,
        args: String,
    },
    ToolProgress {
        name: String,
        chunk: String,
    },
    ToolResult {
        name: String,
        args: String,
        output: String,
        diff: Option<tools::DiffPreview>,
        /// Wall-clock time the tool spent running, in milliseconds.
        millis: u64,
    },
    /// Token usage reported by the provider for the turn just completed.
    Usage {
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        cost: f64,
    },
    /// Older turns were replaced with a summary to free context.
    Compaction {
        summary: String,
        summarized: usize,
        tokens_before: u64,
        read_files: Vec<String>,
        modified_files: Vec<String>,
    },
    /// The TUI navigated the session tree and continued from another entry.
    Branch {
        history: Vec<Message>,
        prompt: String,
        message: String,
    },
    Error(String),
    Finished(Vec<Message>),
}

/// Drive one conversation turn to completion: stream assistant output, execute
/// any requested tools, feed the results back, and repeat until the model stops
/// asking for tools.
pub fn run(
    config: Config,
    cwd: PathBuf,
    history: Vec<Message>,
    tx: UnboundedSender<AgentEvent>,
    runtime: Runtime,
) -> RunFuture {
    run_depth(config, cwd, history, tx, runtime, 0)
}

/// Runs a slash command in an isolated subagent context. The subagent streams
/// into `tx`, and its final text is appended to `history` as an assistant
/// message before `Finished` is emitted with the merged history.
pub fn run_subagent(
    config: Config,
    cwd: PathBuf,
    history: Vec<Message>,
    agent_name: String,
    prompt: String,
    tx: UnboundedSender<AgentEvent>,
    runtime: Runtime,
) -> RunFuture {
    Box::pin(async move {
        let agent = match config.ecosystem.agent(&agent_name).cloned() {
            Some(agent) if agent.mode != AgentMode::Primary => agent,
            Some(_) => {
                let _ = tx.send(AgentEvent::Error(format!(
                    "agent `{agent_name}` is primary and cannot run as a subagent"
                )));
                let _ = tx.send(AgentEvent::Finished(history));
                return;
            }
            None => {
                let _ = tx.send(AgentEvent::Error(format!("unknown agent `{agent_name}`")));
                let _ = tx.send(AgentEvent::Finished(history));
                return;
            }
        };

        let session = runtime.session.clone();
        let mut sub = config;
        sub.active_agent = Some(agent);
        let sub_runtime = Runtime {
            session: None,
            follow_ups: Steering::new(),
            ..runtime
        };

        let (sub_tx, mut sub_rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = tokio::spawn(run_depth(
            sub,
            cwd,
            vec![Message::user(prompt)],
            sub_tx,
            sub_runtime,
            1,
        ));

        let mut report = String::new();
        while let Some(event) = sub_rx.recv().await {
            match event {
                AgentEvent::Text(delta) => {
                    report.push_str(&delta);
                    let _ = tx.send(AgentEvent::Text(delta));
                }
                AgentEvent::ThinkingDelta(delta) => {
                    let _ = tx.send(AgentEvent::ThinkingDelta(delta));
                }
                AgentEvent::Thought { millis } => {
                    let _ = tx.send(AgentEvent::Thought { millis });
                }
                AgentEvent::ThoughtDone { millis } => {
                    let _ = tx.send(AgentEvent::ThoughtDone { millis });
                }
                AgentEvent::Retrying {
                    attempt,
                    max,
                    delay_ms,
                } => {
                    let _ = tx.send(AgentEvent::Retrying {
                        attempt,
                        max,
                        delay_ms,
                    });
                }
                AgentEvent::ToolCall { name, args } => {
                    let _ = tx.send(AgentEvent::ToolCall { name, args });
                }
                AgentEvent::SubagentActivity { agent, tool, args } => {
                    let _ = tx.send(AgentEvent::SubagentActivity { agent, tool, args });
                }
                AgentEvent::ToolProgress { name, chunk } => {
                    let _ = tx.send(AgentEvent::ToolProgress { name, chunk });
                }
                AgentEvent::ToolResult {
                    name,
                    args,
                    output,
                    diff,
                    millis,
                } => {
                    let _ = tx.send(AgentEvent::ToolResult {
                        name,
                        args,
                        output,
                        diff,
                        millis,
                    });
                }
                AgentEvent::Usage {
                    input,
                    output,
                    cache_read,
                    cache_write,
                    cost,
                } => {
                    let _ = tx.send(AgentEvent::Usage {
                        input,
                        output,
                        cache_read,
                        cache_write,
                        cost,
                    });
                }
                AgentEvent::Compaction {
                    summary,
                    summarized,
                    tokens_before,
                    read_files,
                    modified_files,
                } => {
                    let _ = tx.send(AgentEvent::Compaction {
                        summary,
                        summarized,
                        tokens_before,
                        read_files,
                        modified_files,
                    });
                }
                AgentEvent::Branch {
                    history,
                    prompt,
                    message,
                } => {
                    let _ = tx.send(AgentEvent::Branch {
                        history,
                        prompt,
                        message,
                    });
                }
                AgentEvent::Error(message) => {
                    let _ = tx.send(AgentEvent::Error(message));
                }
                AgentEvent::Finished(_) => break,
            }
        }
        let _ = handle.await;

        let mut merged = history;
        if !report.trim().is_empty() {
            let message = Message::assistant(report, Vec::new());
            if let Some(log) = &session {
                let _ = log.append(&message);
            }
            merged.push(message);
        }
        let _ = tx.send(AgentEvent::Finished(merged));
    })
}

fn run_depth(
    config: Config,
    cwd: PathBuf,
    history: Vec<Message>,
    tx: UnboundedSender<AgentEvent>,
    runtime: Runtime,
    depth: usize,
) -> RunFuture {
    Box::pin(run_loop(config, cwd, history, tx, runtime, depth))
}

async fn run_loop(
    config: Config,
    cwd: PathBuf,
    history: Vec<Message>,
    tx: UnboundedSender<AgentEvent>,
    runtime: Runtime,
    depth: usize,
) {
    let client = match runtime.session.as_ref() {
        // The session id doubles as the provider's cache-affinity key, so
        // successive turns reuse the cached conversation prefix.
        Some(log) => LlmClient::new(config.clone()).with_session_id(log.id()),
        None => LlmClient::new(config.clone()),
    };
    let permissions = Permissions::from_config(&config);
    let mut messages = history;
    let context_window = config.context_window();
    let compaction_budget = config.compaction.resolve(&config.provider, &config.model);
    let mut context_tokens: u64 = 0;
    auto_load_mcp_for_user_text(&runtime, messages.iter().filter(|m| m.role == "user")).await;

    // Set once a file edit succeeds and cleared when a later tool call could
    // confirm it. If a run tries to finish while an edit is unconfirmed, the
    // model gets one hidden reminder to verify before its summary.
    let mut verification = VerificationState::default();
    let mut verification_reminder: Option<String> = None;
    // Verification commands already run this turn, so an exact repeat can be
    // flagged instead of silently re-run (a slow build or test suite).
    let mut seen_verifications: BTreeSet<String> = BTreeSet::new();

    loop {
        if runtime.cancel.is_cancelled() {
            let _ = tx.send(AgentEvent::Finished(messages));
            return;
        }
        for steered in runtime.steering.drain() {
            auto_load_mcp_for_user_text(&runtime, std::iter::once(&steered)).await;
            record(&runtime.session, depth, &steered);
            messages.push(steered);
        }

        // Pi-style auto-compaction: once the outgoing context approaches the
        // model window, replace older turns with a summary and keep the recent
        // tokens verbatim. The record is stored on the session branch.
        if depth == 0 && compaction_budget.enabled {
            let current = context_tokens.max(crate::compact::estimate_tokens(&messages) as u64);
            if crate::compact::needs_compaction(current, context_window, compaction_budget) {
                if let Some(log) = &runtime.session {
                    match crate::compact::generate(
                        &config,
                        &messages,
                        &[],
                        compaction_budget,
                        current,
                        None,
                    )
                    .await
                    {
                        Ok(Some(compaction)) => {
                            let first_kept = log
                                .context_ids()
                                .get(compaction.first_kept)
                                .cloned()
                                .unwrap_or_else(|| compaction.first_kept.to_string());
                            let _ = log.append_compaction(
                                compaction.summary.clone(),
                                first_kept,
                                compaction.tokens_before,
                                Some(compaction.details.clone()),
                                compaction.usage,
                            );
                            let _ = tx.send(AgentEvent::Compaction {
                                summary: compaction.summary.clone(),
                                summarized: compaction.summarized,
                                tokens_before: compaction.tokens_before,
                                read_files: compaction.details.read_files.clone(),
                                modified_files: compaction.details.modified_files.clone(),
                            });
                            if let Some(usage) = compaction.usage {
                                let _ = tx.send(AgentEvent::Usage {
                                    input: usage.input,
                                    output: usage.output,
                                    cache_read: usage.cache_read,
                                    cache_write: usage.cache_write,
                                    cost: usage.cost,
                                });
                            }
                            if let Ok(refreshed) = log.messages() {
                                messages = refreshed;
                            }
                            context_tokens = 0;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            let _ = tx.send(AgentEvent::Error(format!("compaction: {err:#}")));
                        }
                    }
                }
            }
        }

        let mut request = Vec::with_capacity(messages.len() + 3);
        request.push(Message::system(config.compose_system_prompt()));
        request.extend(messages.iter().cloned());
        // A reminder makes this request the hidden nudge: it asks the model to
        // confirm work it already summarized, so a provider that answers it
        // with nothing must not fail the turn the user already saw complete.
        let nudging = match verification_reminder.take() {
            Some(reminder) => {
                request.push(Message::system(reminder));
                true
            }
            None => false,
        };
        let tool_specs = build_tool_specs(&config, &runtime, depth);

        let started = std::time::Instant::now();
        let mut thought_sent = false;
        let mut on_text = |delta: String| {
            if !thought_sent {
                thought_sent = true;
                let _ = tx.send(AgentEvent::Thought {
                    millis: started.elapsed().as_millis() as u64,
                });
            }
            let _ = tx.send(AgentEvent::Text(delta));
        };
        let mut on_thinking = |delta: String| {
            let _ = tx.send(AgentEvent::ThinkingDelta(delta));
        };
        let mut on_retry = |retry: Retry| {
            let _ = tx.send(AgentEvent::Retrying {
                attempt: retry.attempt,
                max: retry.max,
                delay_ms: retry.delay.as_millis() as u64,
            });
        };
        let turn = {
            let mut hooks = StreamHooks {
                text: &mut on_text,
                thinking: &mut on_thinking,
                retry: &mut on_retry,
            };
            match client.stream_chat(&request, &tool_specs, &mut hooks).await {
                Ok(turn) => turn,
                Err(err) if nudge_failed_quietly(nudging, &err) => {
                    // The reminder is advisory: the model already summarized its
                    // work and nothing more will arrive, so finish with the
                    // answer in hand instead of reporting `err`. The thinking
                    // block this attempt may have streamed is closed first, so
                    // the transcript does not keep it open as still thinking.
                    let _ = tx.send(AgentEvent::Thought {
                        millis: started.elapsed().as_millis() as u64,
                    });
                    let _ = tx.send(AgentEvent::Finished(messages));
                    return;
                }
                Err(err) => {
                    let _ = tx.send(AgentEvent::Error(format!("{err:#}")));
                    let _ = tx.send(AgentEvent::Finished(messages));
                    return;
                }
            }
        };
        if !thought_sent {
            let _ = tx.send(AgentEvent::Thought {
                millis: started.elapsed().as_millis() as u64,
            });
        }
        let _ = tx.send(AgentEvent::ThoughtDone {
            millis: started.elapsed().as_millis() as u64,
        });

        if turn.content.trim().is_empty() && turn.tool_calls.is_empty() {
            let _ = tx.send(AgentEvent::Error(
                "the model returned an empty response".to_string(),
            ));
            let _ = tx.send(AgentEvent::Finished(messages));
            return;
        }

        let tool_calls = turn.tool_calls.clone();
        let assistant = Message::assistant(turn.content, tool_calls.clone())
            .with_thinking(turn.thinking.clone());
        record_usage(&runtime.session, depth, &assistant, Some(turn.usage.into()));
        messages.push(assistant);
        if turn.usage.total() > 0 {
            context_tokens = turn.usage.input
                + turn.usage.cache_read
                + turn.usage.cache_write
                + turn.usage.output;
            let _ = tx.send(AgentEvent::Usage {
                input: turn.usage.input,
                output: turn.usage.output,
                cache_read: turn.usage.cache_read,
                cache_write: turn.usage.cache_write,
                cost: turn.usage.cost,
            });
        }

        if tool_calls.is_empty() {
            let steered = runtime.steering.drain();
            if steered.is_empty() {
                // Follow-up messages are delivered only once all work is done,
                // so they are drained here, just before finishing.
                let follow_ups = runtime.follow_ups.drain();
                if follow_ups.is_empty() {
                    if let Some(reminder) = verification.reminder() {
                        verification_reminder = Some(reminder);
                        continue;
                    }
                    let _ = tx.send(AgentEvent::Finished(messages));
                    return;
                }
                for message in follow_ups {
                    record(&runtime.session, depth, &message);
                    messages.push(message);
                }
                continue;
            }
            for message in steered {
                record(&runtime.session, depth, &message);
                messages.push(message);
            }
            continue;
        }

        // Cancelled after the model planned tools: record a result for each
        // pending call so the session stays a valid call/result sequence, then
        // stop without running them.
        if runtime.cancel.is_cancelled() {
            for call in &tool_calls {
                let _ = tx.send(AgentEvent::ToolResult {
                    name: call.function.name.clone(),
                    args: call.function.arguments.clone(),
                    output: "error: cancelled by the user".to_string(),
                    diff: None,
                    millis: 0,
                });
                let message =
                    Message::tool(call.id.clone(), "error: cancelled by the user".to_string());
                record(&runtime.session, depth, &message);
                messages.push(message);
            }
            let _ = tx.send(AgentEvent::Finished(messages));
            return;
        }

        let mut terminated: Vec<bool> = Vec::with_capacity(tool_calls.len());
        let mut snapshot_needed = depth == 0 && runtime.plugins.is_active();

        // Dispatch the batch in the model's requested order, but run each maximal
        // run of concurrency-safe calls together. A state-changing call always gets
        // its own run, so read/write dependencies keep their order while a mixed
        // batch still parallelizes its reads.
        for tool_calls in batch_runs(tool_calls) {
            let parallel = tool_calls.len() > 1
                && tool_calls
                    .iter()
                    .all(|call| concurrency_safe(&call.function.name));

            if parallel {
                enum Prepared {
                    Immediate(tools::ToolOutput),
                    Run { call: ToolCall, args: Value },
                }

                let mut prepared = Vec::with_capacity(tool_calls.len());
                for original in &tool_calls {
                    let name = original.function.name.clone();
                    let _ = tx.send(AgentEvent::ToolCall {
                        name: name.clone(),
                        args: original.function.arguments.clone(),
                    });

                    let mut call = original.clone();
                    let args = serde_json::from_str::<Value>(&call.function.arguments)
                        .unwrap_or(Value::Null);
                    let effective_args = match runtime.plugins.tool_before(&name, &args).await {
                        Some(mutated) if mutated != args => {
                            if let Ok(text) = serde_json::to_string(&mutated) {
                                call.function.arguments = text;
                            }
                            mutated
                        }
                        _ => args,
                    };
                    let subject = subject_for(&name, &effective_args);
                    prepared.push(
                        if permission_granted(
                            permissions.decide(&name, &subject),
                            config.auto_approve,
                            &runtime.approve,
                            &name,
                            &subject,
                        )
                        .await
                        {
                            Prepared::Run {
                                call,
                                args: effective_args,
                            }
                        } else {
                            Prepared::Immediate(tools::ToolOutput::text(format!(
                                "error: permission denied for `{name}`"
                            )))
                        },
                    );
                }

                let mut handles = Vec::with_capacity(prepared.len());
                for item in prepared {
                    let config = config.clone();
                    let cwd = cwd.clone();
                    let runtime = runtime.clone();
                    let tx = tx.clone();
                    handles.push(tokio::spawn(async move {
                        match item {
                            Prepared::Immediate(output) => (output, 0),
                            Prepared::Run { call, args } => {
                                let name = call.function.name.clone();
                                let progress = tools::Progress::new(Arc::new({
                                    let tx = tx.clone();
                                    let name = name.clone();
                                    move |chunk: &str| {
                                        let _ = tx.send(AgentEvent::ToolProgress {
                                            name: name.clone(),
                                            chunk: chunk.to_string(),
                                        });
                                    }
                                }));
                                let started = std::time::Instant::now();
                                let mut output =
                                    dispatch(&config, &cwd, &runtime, &tx, &call, depth, &progress)
                                        .await;
                                if let Some(result) =
                                    runtime.plugins.tool_after(&name, &args, &output.text).await
                                {
                                    output.text = result.output;
                                    output.terminate |= result.terminate;
                                }
                                (output, started.elapsed().as_millis() as u64)
                            }
                        }
                    }));
                }

                for (handle, original) in handles.into_iter().zip(&tool_calls) {
                    let (output, millis) = match handle.await {
                        Ok(result) => result,
                        Err(err) => (
                            tools::ToolOutput::text(format!("error: tool task failed: {err}")),
                            0,
                        ),
                    };
                    let args = serde_json::from_str::<Value>(&original.function.arguments)
                        .unwrap_or(Value::Null);
                    verification.record(&original.function.name, &args, &output.text);
                    terminated.push(output.terminate);
                    let _ = tx.send(AgentEvent::ToolResult {
                        name: original.function.name.clone(),
                        args: original.function.arguments.clone(),
                        output: output.text.clone(),
                        diff: output.diff.clone(),
                        millis,
                    });
                    let tool_message = if output.media.is_empty() {
                        Message::tool(original.id.clone(), output.text)
                    } else {
                        Message::tool_parts(original.id.clone(), output.text, output.media)
                    };
                    record(&runtime.session, depth, &tool_message);
                    messages.push(tool_message);
                }
            } else {
                for original in &tool_calls {
                    let name = original.function.name.clone();
                    let _ = tx.send(AgentEvent::ToolCall {
                        name: name.clone(),
                        args: original.function.arguments.clone(),
                    });

                    let mut call = original.clone();
                    let args = serde_json::from_str::<Value>(&call.function.arguments)
                        .unwrap_or(Value::Null);
                    let effective_args = match runtime.plugins.tool_before(&name, &args).await {
                        Some(mutated) if mutated != args => {
                            if let Ok(text) = serde_json::to_string(&mutated) {
                                call.function.arguments = text;
                            }
                            mutated
                        }
                        _ => args,
                    };

                    let subject = subject_for(&name, &effective_args);
                    let progress = tools::Progress::new(Arc::new({
                        let tx = tx.clone();
                        let name = name.clone();
                        move |chunk: &str| {
                            let _ = tx.send(AgentEvent::ToolProgress {
                                name: name.clone(),
                                chunk: chunk.to_string(),
                            });
                        }
                    }));
                    let started = std::time::Instant::now();
                    let canonical = crate::tools::canonical_tool_name(&name);
                    let block_reply = if canonical == "bash" {
                        match effective_args.get("command").and_then(Value::as_str) {
                            Some(command) if posts_review_reply(command) => {
                                let repo_pending = repo_has_pending_delivery(&cwd).await;
                                verification.blocks_review_reply(command, repo_pending)
                            }
                            _ => false,
                        }
                    } else {
                        false
                    };
                    let mut dispatched = false;
                    let mut output = if block_reply {
                        tools::ToolOutput::text(
                        "error: commit and push the code changes before replying to the review, so \
                         the reply does not claim a fix that is not on the branch under review"
                            .to_string(),
                    )
                    } else if permission_granted(
                        permissions.decide(&name, &subject),
                        config.auto_approve,
                        &runtime.approve,
                        &name,
                        &subject,
                    )
                    .await
                    {
                        snapshot_needed |= tool_may_mutate_workspace(&name);
                        dispatched = true;
                        dispatch(&config, &cwd, &runtime, &tx, &call, depth, &progress).await
                    } else {
                        tools::ToolOutput::text(format!("error: permission denied for `{name}`"))
                    };
                    let canonical_name = canonical;
                    if matches!(canonical_name, "write_file" | "edit")
                        && !output.text.starts_with("error:")
                    {
                        if let Some(path) = effective_args.get("path").and_then(Value::as_str) {
                            if let Some(diagnostics) =
                                runtime.lsp.diagnostics(&cwd, Path::new(path)).await
                            {
                                output.text.push_str("\n\n");
                                output.text.push_str(&diagnostics);
                            }
                        }
                    }
                    if let Some(result) = runtime
                        .plugins
                        .tool_after(&name, &effective_args, &output.text)
                        .await
                    {
                        output.text = result.output;
                        output.terminate |= result.terminate;
                    }
                    let millis = started.elapsed().as_millis() as u64;
                    verification.record(&name, &effective_args, &output.text);
                    // An edit invalidates a verifier result: the next run of the same
                    // build/test is a fresh check, not a repeat, so it must not get the
                    // "already ran" note. A failed edit left the workspace unchanged, so
                    // it must not clear the tracking.
                    if mutation_invalidates_verifier(canonical_name, &output.text) {
                        seen_verifications.clear();
                    }
                    if canonical_name == "bash" {
                        if let Some(command) = effective_args.get("command").and_then(Value::as_str)
                        {
                            if note_verifier(&mut seen_verifications, dispatched, command) {
                                output.text.push_str(
                                "\n\n[note: this build or test already ran in this run; its result \
                                 is unchanged unless you edited files, so there is no need to run it \
                                 again]",
                            );
                            }
                        }
                    }
                    terminated.push(output.terminate);
                    let text = output.text.clone();

                    let _ = tx.send(AgentEvent::ToolResult {
                        name,
                        args: serde_json::to_string(&effective_args).unwrap_or_default(),
                        output: text.clone(),
                        diff: output.diff.clone(),
                        millis,
                    });
                    let tool_message = if output.media.is_empty() {
                        Message::tool(call.id.clone(), text)
                    } else {
                        Message::tool_parts(call.id.clone(), text, output.media)
                    };
                    record(&runtime.session, depth, &tool_message);
                    messages.push(tool_message);
                }
            }
        }

        if snapshot_needed {
            if let Some(snapshots) = &runtime.snapshots {
                // `git add -A` is synchronous and can take a while on a large
                // tree; keep it off the async worker so the TUI stays live.
                let snapshots = Arc::clone(snapshots);
                let _ = tokio::task::spawn_blocking(move || snapshots.commit("turn")).await;
            }
        }

        if batch_terminates(&terminated) {
            let _ = tx.send(AgentEvent::Finished(messages));
            return;
        }
    }
}

/// Loads any configured MCP servers whose routing domains appear in a user
/// message before the next model call, so URL-driven requests hit the right
/// tools on the first turn instead of burning a round trip on `mcp_load`.
async fn auto_load_mcp_for_user_text<'a>(
    runtime: &Runtime,
    messages: impl IntoIterator<Item = &'a Message>,
) {
    let mut names = Vec::new();
    for message in messages {
        let text = message
            .content
            .as_ref()
            .map(|content| content.display())
            .unwrap_or_default();
        for name in runtime.mcp.servers_for_text(&text) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    if names.is_empty() {
        return;
    }
    let mut loads = tokio::task::JoinSet::new();
    for name in names {
        let mcp = Arc::clone(&runtime.mcp);
        loads.spawn(async move { mcp.load(&name).await });
    }
    while loads.join_next().await.is_some() {}
}

fn build_tool_specs(config: &Config, runtime: &Runtime, depth: usize) -> Vec<ToolSpec> {
    let mut specs = tools::specs(&runtime.mcp);
    if depth < MAX_TASK_DEPTH
        && config
            .ecosystem
            .agents
            .iter()
            .any(|agent| agent.mode != AgentMode::Primary)
    {
        specs.push(task_spec(config));
    }
    if !config.ecosystem.skills.is_empty() {
        specs.push(skill_spec(config));
    }
    if !config.ecosystem.commands.is_empty() || !config.ecosystem.prompt_templates.is_empty() {
        specs.push(command_spec(config));
    }
    specs.push(memory_spec());
    specs.push(lsp_spec());
    if config.tool_filter.is_restrictive() {
        specs.retain(|spec| config.tool_filter.permits(&spec.function.name));
    }
    specs
}

/// A batch ends the turn when every tool result in it requested termination.
fn batch_terminates(terminated: &[bool]) -> bool {
    !terminated.is_empty() && terminated.iter().all(|value| *value)
}

async fn permission_granted(
    action: Action,
    auto_approve: bool,
    approve: &Approver,
    tool: &str,
    subject: &str,
) -> bool {
    action == Action::Allow || auto_approve || approve(tool.to_string(), subject.to_string()).await
}
/// Splits a tool batch into dispatch runs. A maximal run of concurrency-safe
/// calls becomes one run (dispatched together), while every other call becomes
/// its own run so state-changing calls stay sequential and keep their order.
fn batch_runs(calls: Vec<ToolCall>) -> Vec<Vec<ToolCall>> {
    let mut runs: Vec<Vec<ToolCall>> = Vec::new();
    let mut current: Vec<ToolCall> = Vec::new();
    let mut current_safe = false;
    for call in calls {
        let safe = concurrency_safe(&call.function.name);
        if !current.is_empty() && !(safe && current_safe) {
            runs.push(std::mem::take(&mut current));
        }
        current.push(call);
        current_safe = safe;
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

/// Tools with no cross-call side effects can run concurrently when the model
/// batches several of them. Anything that mutates the workspace (`write_file`,
/// `patch`, `bash`), spawns work (`task`), or has unknown remote effects (MCP)
/// stays on the sequential path so result ordering and side effects are stable.
fn concurrency_safe(name: &str) -> bool {
    matches!(
        crate::tools::canonical_tool_name(name),
        "read_file"
            | "list_dir"
            | "glob"
            | "grep"
            | "webfetch"
            | "memory"
            | "skill"
            | "diagnostics"
    )
}

fn tool_may_mutate_workspace(name: &str) -> bool {
    let canonical = crate::tools::canonical_tool_name(name);
    matches!(canonical, "write_file" | "patch" | "edit" | "bash" | "task")
        || canonical.contains("__")
}

/// A state-changing action that should be confirmed before a run reports
/// success, with the commands that count as confirming it.
struct DoneRule {
    key: &'static str,
    /// Substrings that mark a command as performing the action.
    actions: &'static [&'static str],
    /// Substrings that mark a command as confirming the outcome.
    checks: &'static [&'static str],
    /// What the model is told to run when the action is unconfirmed.
    reminder: &'static str,
}

/// Outcomes that outlive the workspace — beyond files and builds — which the
/// Definition of Done requires confirming. A rule is a no-op until its action
/// (and, after that, a check) appears in a shell command.
const DONE_RULES: &[DoneRule] = &[
    DoneRule {
        key: "pull request",
        actions: &[
            "gh pr create",
            "gh pr edit",
            "gh pr ready",
            "glab mr create",
        ],
        checks: &[
            "gh pr checks",
            "gh pr view",
            "gh pr status",
            "gh pr list",
            "gh run list",
            "gh run watch",
            "gh run view",
            "glab mr view",
            "glab ci status",
            "pulls",
        ],
        reminder: "You opened or updated a pull request but have not checked it. Run \
                   `gh pr checks <url>` and `gh pr view <url> --json \
                   state,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup` (or the \
                   `glab` equivalents), fix and push again if a check failed, and report the URL \
                   with its final state.",
    },
    DoneRule {
        key: "comment",
        actions: &[
            "gh pr comment",
            "gh pr review",
            "gh issue comment",
            "glab mr note",
            "glab issue note",
        ],
        checks: &[
            "gh pr view",
            "gh issue view",
            "glab mr view",
            "glab issue view",
            "comments",
        ],
        reminder: "You posted a comment or review but have not confirmed it appears. Read the \
                   thread back (`gh pr view <url> --comments` or `gh issue view <url> --comments`) \
                   and confirm the reply landed in the right place.",
    },
    DoneRule {
        key: "release",
        actions: &["gh release create", "gh release edit"],
        checks: &["gh release view", "gh release list"],
        reminder: "You created or edited a release but have not checked it. Confirm with \
                   `gh release view <tag>` and report the result.",
    },
    DoneRule {
        key: "deployment",
        actions: &[
            "terraform apply",
            "kubectl apply",
            "kubectl rollout restart",
            "helm upgrade",
            "npm publish",
            "cargo publish",
            "docker push",
            "fly deploy",
            "vercel deploy",
            "wrangler deploy",
            "serverless deploy",
            "sam deploy",
            "gcloud run deploy",
        ],
        checks: &[
            "terraform plan",
            "kubectl get",
            "kubectl rollout status",
            "kubectl describe",
            "helm status",
            "npm view",
            "cargo search",
            "docker inspect",
            "fly status",
            "vercel inspect",
            "wrangler deployments",
            "gcloud run services describe",
        ],
        reminder: "You changed deployed or published state but have not confirmed it. Query the \
                   resulting status (for example `kubectl rollout status`, `terraform plan`, \
                   `npm view <pkg> version`, `helm status`) and report it.",
    },
    DoneRule {
        key: "scope",
        actions: &[
            "git add -a",
            "git add --all",
            "git add .",
            "git add :/",
            "git stage -a",
            "git commit -a",
            "git commit --all",
        ],
        checks: &[
            "git status",
            "git diff --staged",
            "git diff --cached",
            "git diff --stat",
            "git diff --name-only",
            "git show",
            "git log --stat",
        ],
        reminder: "You staged or committed with a blanket flag. Review exactly what is included \
                   (`git status`, `git diff --staged`) and drop unrelated or local-only files — for \
                   example `.claude/settings.local.json`, `.idea/`, editor state, or build output \
                   — before you push or open a pull request.",
    },
];

/// Tracks work that must be confirmed before a run can report success: files
/// edited but not confirmed on disk, and side effects (pull requests, comments,
/// releases, deployments) performed but not checked. A model that tries to
/// finish anyway is asked to verify first.
#[derive(Default)]
struct VerificationState {
    edited: BTreeSet<String>,
    pending: BTreeSet<&'static str>,
    nudged: bool,
    /// A pull request or review was opened, replied to, or updated, so any edit
    /// made afterwards is part of that external work and has to be delivered.
    reviewed: bool,
    /// Files were edited since the last successful `git push`. Reading the
    /// change back proves it is on disk but does not put it in the pull request.
    edited_since_push: bool,
}

impl VerificationState {
    /// Records one tool result. A successful edit adds its path; reading,
    /// diffing or type-checking that path confirms it, and a build/test/lint
    /// command confirms everything at once. Side-effecting shell commands add a
    /// pending check that the matching rule's status command clears.
    fn record(&mut self, name: &str, args: &Value, output: &str) {
        let canonical = crate::tools::canonical_tool_name(name);
        let path = args.get("path").and_then(Value::as_str);
        match canonical {
            "write_file" | "edit" | "patch" => {
                if !output_failed(output) {
                    if let Some(path) = path.filter(|path| !path.is_empty()) {
                        self.edited.insert(path.to_string());
                    }
                    self.edited_since_push = true;
                }
            }
            "read_file" | "diagnostics" => {
                if let Some(path) = path.filter(|path| !path.is_empty()) {
                    self.edited.retain(|edited| !same_path(edited, path));
                }
            }
            "bash" => {
                let raw = args.get("command").and_then(Value::as_str).unwrap_or("");
                // A build/test/lint run confirms every edit at once; a shell
                // inspection that names one file confirms just that file, which
                // is the same evidence the reminder asks for without spending
                // another whole turn on it.
                if looks_like_verification_command(raw) {
                    self.edited.clear();
                } else if !output_failed(output) {
                    self.edited
                        .retain(|edited| !inspected_by_shell(raw, output, edited));
                }
                let command = raw.to_ascii_lowercase();
                let failed = output_failed(output);
                if !failed && runs_git_push(raw) {
                    self.edited_since_push = false;
                }
                for rule in DONE_RULES {
                    if !failed && rule.actions.iter().any(|action| command.contains(action)) {
                        self.pending.insert(rule.key);
                        if matches!(rule.key, "pull request" | "comment") {
                            self.reviewed = true;
                        }
                    }
                    if rule.checks.iter().any(|check| command.contains(check)) {
                        self.pending.remove(rule.key);
                    }
                }
                // The REST reply endpoint is not in the rule's actions because
                // a bare `/replies` substring is also a read; only a POST posts.
                if !failed && posts_review_reply(raw) {
                    self.pending.insert("comment");
                    self.reviewed = true;
                }
            }
            _ => {}
        }
    }

    /// Whether a review reply must be held until the pending code changes are
    /// pushed. Replying that a comment is fixed before the branch has the fix
    /// is misleading, so the reply is refused with an instruction to push first.
    /// `repo_pending` covers edits made outside the tracked tools (a `sed -i`,
    /// a formatter, a script) and commits that were not pushed.
    fn blocks_review_reply(&mut self, command: &str, repo_pending: bool) -> bool {
        if posts_review_reply(command) && (self.edited_since_push || repo_pending) {
            // The reply is part of a review delivery: if the run stops here, the
            // finish reminder still has to ask for the push.
            self.reviewed = true;
            return true;
        }
        false
    }

    /// The reminder to send once when work is unconfirmed, or `None` when there
    /// is nothing to verify or it has already been sent.
    fn reminder(&mut self) -> Option<String> {
        if self.nudged {
            return None;
        }
        let mut parts = Vec::new();
        if !self.edited.is_empty() {
            let files = self.edited.iter().cloned().collect::<Vec<_>>().join(", ");
            parts.push(format!(
                "You edited {files} but have not confirmed the change is on disk. Re-read the \
                 changed region (or `grep` for the new symbol) or run the build/tests."
            ));
        }
        for key in &self.pending {
            if let Some(rule) = DONE_RULES.iter().find(|rule| rule.key == *key) {
                parts.push(rule.reminder.to_string());
            }
        }
        // A review reply that ships a code change is not delivered until the
        // change is pushed: the pull request still shows the old code even
        // though the working tree is correct.
        if self.reviewed && self.edited_since_push {
            parts.push(
                "You addressed a pull request or review with code changes but have not pushed them, \
                 so the branch under review still has the old code. Commit with an explicit path \
                 list and `git push` (or the `glab` equivalent), then confirm the new commit is on \
                 the branch before you call the review addressed."
                    .to_string(),
            );
        }
        if parts.is_empty() {
            return None;
        }
        self.nudged = true;
        Some(format!(
            "# Definition of Done\nBefore you finish: {}",
            parts.join(" ")
        ))
    }
}

/// Whether two tool paths refer to the same file, tolerating one side being
/// absolute and the other relative. Commands and arguments use whichever
/// separator the platform (or the user) chose, so compare on `/` regardless.
fn same_path(a: &str, b: &str) -> bool {
    let a = a.replace('\\', "/");
    let b = b.replace('\\', "/");
    let a = a.trim_end_matches('/');
    let b = b.trim_end_matches('/');
    a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}"))
}

/// Whether a tool result reports failure, either as an `error:` text result or
/// a non-zero shell exit code.
fn output_failed(output: &str) -> bool {
    output.starts_with("error:")
        || output
            .lines()
            .rev()
            .find_map(|line| {
                line.strip_prefix("[exit: ")
                    .or_else(|| line.strip_prefix("[exit code: "))
            })
            .and_then(|rest| rest.strip_suffix(']'))
            .and_then(|code| code.trim().parse::<i32>().ok())
            .is_some_and(|code| code != 0)
}

/// Whether a shell command is the kind that checks work (`cargo test`,
/// `./gradlew build`, `npm run lint`, …) rather than formatting or something
/// unrelated like posting a comment. Tokens are matched exactly (plus
/// camel-case task names like `spotlessCheck`), so the runner (`gradle`, `npm`)
/// alone or a flag word like `latest` does not count.
fn looks_like_verification_command(command: &str) -> bool {
    const EXACT: &[&str] = &[
        "test",
        "tests",
        "check",
        "lint",
        "build",
        "compile",
        "clippy",
        "typecheck",
        "pytest",
        "verify",
        "vet",
        "tsc",
        "spec",
        "specs",
    ];
    const SUFFIXES: &[&str] = &[
        "Check",
        "Test",
        "Tests",
        "Build",
        "Lint",
        "Compile",
        "Verify",
        "Typecheck",
    ];
    command
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| {
            !token.is_empty()
                && (EXACT.iter().any(|word| token.eq_ignore_ascii_case(word))
                    || SUFFIXES.iter().any(|suffix| token.ends_with(suffix)))
        })
}

/// Whether a shell command inspected the edited file itself — a `git diff`,
/// `git show`, or `grep`/`rg` naming that path as the file it reads — so the
/// change is known to be on disk. Only the paths a command names are confirmed,
/// unlike a build or test run, which confirms every edit at once.
fn inspected_by_shell(command: &str, output: &str, path: &str) -> bool {
    const SEARCHERS: &[&str] = &["grep", "rg"];
    const GIT_VIEWERS: &[&str] = &["diff", "show"];
    command.split(['\n', ';', '|', '&']).any(|segment| {
        let mut words = segment.split_whitespace();
        let Some(tool) = words
            .next()
            .map(|word| word.rsplit('/').next().unwrap_or(word))
        else {
            return false;
        };
        let arguments: Vec<&str> = words.map(|word| word.trim_matches(['\'', '"'])).collect();
        if SEARCHERS.contains(&tool) {
            // A searcher's first operand is the pattern, not a file: the path
            // counts only as a later file operand, so `grep -rn "a.rs" .`
            // matching a *mention* of the file does not confirm anything.
            return arguments
                .iter()
                .filter(|argument| !argument.starts_with('-'))
                .skip(1)
                .any(|argument| same_path(path, argument));
        }
        if tool == "git" && GIT_VIEWERS.iter().any(|viewer| arguments.contains(viewer)) {
            // An empty diff exits 0 while showing nothing, and `git show`
            // without a `--` operand reads the committed blob rather than the
            // working file, so require a hunk naming the path.
            let shown = output
                .lines()
                .filter(|line| !line.starts_with("[exit"))
                .collect::<Vec<_>>()
                .join("\n");
            return arguments.iter().any(|argument| same_path(path, argument))
                && (shown.contains("@@") || shown.contains(path));
        }
        false
    })
}

/// Whether a failed model call may end the run quietly. The Definition-of-Done
/// reminder is advisory — it is sent after the model already summarized its
/// work — so a provider that answers it with nothing leaves a complete answer
/// rather than an error for the user to read.
fn nudge_failed_quietly(nudging: bool, err: &anyhow::Error) -> bool {
    nudging && err.downcast_ref::<crate::llm::NoAnswer>().is_some()
}

/// Whether a shell command posts a reply or review comment on a pull/merge
/// request. Replying that a comment is fixed before the fix is on the branch is
/// misleading, so these commands are held until the pending edits are pushed.
/// The bare `/replies` endpoint also reads a thread, so a REST call counts only
/// when it explicitly POSTs.
fn posts_review_reply(command: &str) -> bool {
    let command = command.to_ascii_lowercase();
    if DONE_RULES
        .iter()
        .find(|rule| rule.key == "comment")
        .is_some_and(|rule| rule.actions.iter().any(|action| command.contains(action)))
    {
        return true;
    }
    command.contains("/replies")
        && ["-x post", "-xpost", "--method post", "--method=post"]
            .iter()
            .any(|flag| command.contains(flag))
}

/// Whether a shell command runs `git push` as the invoked program (not, say,
/// `echo git push`) and is not a dry run. Clearing the delivery flag is only an
/// optimization: [`repo_has_pending_delivery`] is the authority.
fn runs_git_push(command: &str) -> bool {
    command.split(['\n', ';', '&', '|']).any(|segment| {
        if segment.contains("--dry-run") {
            return false;
        }
        let mut words = segment.split_whitespace();
        words
            .find(|word| !matches!(*word, "cd" | "env" | "sudo" | "time" | "nohup"))
            .is_some_and(|program| program == "git")
            && segment.contains(" push")
    })
}

/// Whether the repository has work that a review reply would falsely claim is
/// delivered. The tracked tools (`write`/`edit`) cover the common case, but
/// `bash` can edit files through `sed -i`, a formatter, or a script, so the
/// guard also inspects the repository: tracked modifications and staged files,
/// plus commits that are not on a remote-tracking branch. Untracked files are
/// ignored so unrelated new files (notes, reports) do not block a reply. The git
/// calls are async so a slow `git` never blocks the runtime.
async fn repo_has_pending_delivery(cwd: &Path) -> bool {
    if git_stdout(cwd, &["status", "--porcelain", "--untracked-files=no"])
        .await
        .is_some_and(|status| !status.is_empty())
    {
        return true;
    }
    let unpushed = git_stdout(cwd, &["rev-list", "--count", "HEAD", "--not", "--remotes"])
        .await
        .and_then(|count| count.parse::<u64>().ok())
        .is_some_and(|count| count > 0);
    if !unpushed {
        return false;
    }
    // A normal clone gives the current branch a remote-tracking ref, so the
    // non-zero count is trustworthy (the commit really is unpushed). A
    // single-branch clone does not — its fetch refspec maps only the default
    // branch, so `--remotes` reports a pushed feature branch as unpushed
    // forever. Ask the remote for the tip only in that case.
    if let Some((remote, branch)) = tracking_branch(cwd).await {
        let reference = format!("refs/remotes/{remote}/{branch}");
        if git_stdout(cwd, &["rev-parse", "--verify", "--quiet", &reference])
            .await
            .is_some()
        {
            return true;
        }
    }
    !remote_has_head(cwd).await
}

/// The remote and branch the current `HEAD` tracks, when configured. Falls back
/// to `origin` when the branch has no explicit remote.
async fn tracking_branch(cwd: &Path) -> Option<(String, String)> {
    let branch = git_stdout(cwd, &["symbolic-ref", "--short", "HEAD"]).await?;
    let remote = git_stdout(
        cwd,
        &["config", "--get", &format!("branch.{branch}.remote")],
    )
    .await
    .filter(|remote| !remote.is_empty())
    .unwrap_or_else(|| "origin".to_string());
    Some((remote, branch))
}

/// Runs `git` in `cwd` and returns trimmed stdout, or `None` on any failure.
/// `GIT_TERMINAL_PROMPT=0` keeps a credential prompt from hanging the agent.
async fn git_stdout(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Whether the current branch's `HEAD` is already delivered to the same-named
/// branch on its remote. Only consulted when the branch has no remote-tracking
/// ref, so it corrects the single-branch-clone blind spot without adding a
/// network round trip to the common case. A remote that cannot be reached is
/// treated as not delivered, preserving the guard.
async fn remote_has_head(cwd: &Path) -> bool {
    let Some((remote, branch)) = tracking_branch(cwd).await else {
        return false;
    };
    let Some(head) = git_stdout(cwd, &["rev-parse", "HEAD"]).await else {
        return false;
    };
    let Some(remote_sha) = ls_remote(cwd, &remote, &branch).await else {
        return false;
    };
    if remote_sha == head {
        return true;
    }
    // The remote branch may have advanced past this commit; `HEAD` is still
    // delivered if it is an ancestor of the remote tip. A tip that is behind
    // (`HEAD` not an ancestor) stays pending.
    git_stdout(cwd, &["merge-base", "--is-ancestor", &head, &remote_sha])
        .await
        .is_some()
}

/// Runs `git ls-remote --heads <remote> <branch>` and returns the branch tip, or
/// `None` when it is absent, unreachable, or slower than the timeout. The child
/// is spawned explicitly so a slow lookup is killed and reaped instead of being
/// left alive by a dropped `output()` future.
async fn ls_remote(cwd: &Path, remote: &str, branch: &str) -> Option<String> {
    use tokio::io::AsyncReadExt;
    let mut child = tokio::process::Command::new("git")
        .args(["ls-remote", "--heads", remote, branch])
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut text = String::new();
    let read = stdout.read_to_string(&mut text);
    match tokio::time::timeout(std::time::Duration::from_secs(5), read).await {
        Ok(Ok(_)) => {}
        _ => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return None;
        }
    }
    match child.wait().await {
        Ok(status) if status.success() => text.split_whitespace().next().map(str::to_string),
        _ => None,
    }
}

/// The identity of a build/test invocation for repeat detection: the command
/// before its output plumbing, so `./gradlew test | tail -50` and
/// `./gradlew test | wc -l` count as the same run. Returns `None` for a command
/// that is not a verifier at all.
fn verification_key(command: &str) -> Option<String> {
    if !looks_like_verification_command(command) {
        return None;
    }
    let base = match first_unquoted(command, '|') {
        Some(index) => &command[..index],
        None => command,
    };
    let base = base.trim().trim_end_matches("2>&1").trim();
    (!base.is_empty()).then(|| base.to_string())
}

/// The byte index of the first `needle` outside single or double quotes.
fn first_unquoted(command: &str, needle: char) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (index, ch) in command.char_indices() {
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch == needle => return Some(index),
            None => {}
        }
    }
    None
}

/// Records a verifier that actually ran and reports whether it had already run
/// this turn. A denied or disabled call never dispatched, so it must not be
/// recorded: approving and retrying it is a first run, not a repeat.
fn note_verifier(seen: &mut BTreeSet<String>, dispatched: bool, command: &str) -> bool {
    if !dispatched {
        return false;
    }
    match verification_key(command) {
        Some(key) => !seen.insert(key),
        None => false,
    }
}

/// Whether a tool result invalidates a build/test result seen earlier in the
/// run. Only a successful `write`/`edit`/`patch` changes the workspace; a failed
/// one leaves it untouched, so the earlier verifier still applies.
fn mutation_invalidates_verifier(canonical_name: &str, output: &str) -> bool {
    matches!(canonical_name, "write_file" | "edit" | "patch") && !output_failed(output)
}

async fn dispatch(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
    events: &UnboundedSender<AgentEvent>,
    call: &crate::llm::ToolCall,
    depth: usize,
    progress: &tools::Progress,
) -> tools::ToolOutput {
    if config.tool_filter.is_restrictive() && !config.tool_filter.permits(&call.function.name) {
        return tools::ToolOutput::text(format!(
            "error: tool `{}` is disabled",
            call.function.name
        ));
    }
    match call.function.name.as_str() {
        "task" => tools::ToolOutput::text(
            task(
                config,
                cwd,
                runtime,
                events,
                &call.function.arguments,
                depth,
            )
            .await,
        ),
        "skill" => tools::ToolOutput::text(skill(config, &call.function.arguments)),
        "command" => tools::ToolOutput::text(
            command(
                config,
                cwd,
                runtime,
                events,
                &call.function.arguments,
                depth,
            )
            .await,
        ),
        "memory" => tools::ToolOutput::text(memory(config, &call.function.arguments)),
        "diagnostics" => lsp_diagnostics(runtime, cwd, &call.function.arguments).await,
        _ => tools::execute(call, cwd, &runtime.mcp, progress).await,
    }
}

async fn lsp_diagnostics(runtime: &Runtime, cwd: &Path, arguments: &str) -> tools::ToolOutput {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return tools::ToolOutput::text("error: `path` is required".to_string());
    };
    match runtime.lsp.diagnostics(cwd, Path::new(path)).await {
        Some(diagnostics) => tools::ToolOutput::text(diagnostics),
        None => tools::ToolOutput::text(format!("error: no language server available for {path}")),
    }
}

fn record(session: &Option<Arc<SessionLog>>, depth: usize, message: &Message) {
    record_usage(session, depth, message, None);
}

fn record_usage(
    session: &Option<Arc<SessionLog>>,
    depth: usize,
    message: &Message,
    usage: Option<crate::compact::UsageRecord>,
) {
    if depth == 0 {
        if let Some(log) = session {
            let _ = log.append_with_usage(message, usage);
        }
    }
}

async fn task(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
    events: &UnboundedSender<AgentEvent>,
    arguments: &str,
    depth: usize,
) -> String {
    match task_inner(config, cwd, runtime, events, arguments, depth).await {
        Ok(output) => output,
        Err(err) => format!("error: {err:#}"),
    }
}

async fn task_inner(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
    events: &UnboundedSender<AgentEvent>,
    arguments: &str,
    depth: usize,
) -> Result<String> {
    let args: Value = serde_json::from_str(arguments).context("invalid task arguments")?;
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .context("missing `prompt` argument")?;
    let name = args
        .get("subagent_type")
        .and_then(Value::as_str)
        .context("missing `subagent_type` argument")?;

    let agent = config.ecosystem.agent(name).cloned().with_context(|| {
        let available: Vec<&str> = config
            .ecosystem
            .agents
            .iter()
            .filter(|agent| agent.mode != AgentMode::Primary)
            .map(|agent| agent.name.as_str())
            .collect();
        format!(
            "unknown subagent `{name}` (available: {})",
            available.join(", ")
        )
    })?;
    if agent.mode == AgentMode::Primary {
        anyhow::bail!("agent `{name}` is primary and cannot be used as a subagent");
    }

    let agent_name = agent.name.clone();
    let mut sub = config.clone();
    sub.active_agent = Some(agent);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sub_runtime = Runtime {
        session: None,
        follow_ups: Steering::new(),
        ..runtime.clone()
    };
    let handle = tokio::spawn(run_depth(
        sub,
        cwd.to_path_buf(),
        vec![Message::user(prompt)],
        tx,
        sub_runtime,
        depth + 1,
    ));

    let mut output = String::new();
    let mut error = None;
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Text(delta) => output.push_str(&delta),
            AgentEvent::Error(message) => error = Some(message),
            AgentEvent::Finished(_) => break,
            // The subagent's own tool traffic feeds the caller's progress view;
            // only its final report becomes the tool result.
            AgentEvent::ToolCall { name, args } => {
                let _ = events.send(AgentEvent::SubagentActivity {
                    agent: agent_name.clone(),
                    tool: name,
                    args,
                });
            }
            AgentEvent::ToolProgress { .. }
            | AgentEvent::ToolResult { .. }
            | AgentEvent::Usage { .. }
            | AgentEvent::Compaction { .. }
            | AgentEvent::Branch { .. }
            | AgentEvent::SubagentActivity { .. }
            | AgentEvent::ThinkingDelta(_)
            | AgentEvent::Retrying { .. }
            | AgentEvent::Thought { .. }
            | AgentEvent::ThoughtDone { .. } => {}
        }
    }
    let _ = handle.await;

    if output.trim().is_empty() {
        if let Some(error) = error {
            anyhow::bail!("subagent `{name}` failed: {error}");
        }
        return Ok(format!(
            "subagent `{name}` finished without producing a response"
        ));
    }
    Ok(output)
}

fn skill(config: &Config, arguments: &str) -> String {
    match skill_inner(config, arguments) {
        Ok(content) => content,
        Err(err) => format!("error: {err:#}"),
    }
}

fn skill_inner(config: &Config, arguments: &str) -> Result<String> {
    let args: Value = serde_json::from_str(arguments).context("invalid skill arguments")?;
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .context("missing `name` argument")?;
    match config
        .ecosystem
        .skills
        .iter()
        .find(|skill| skill.name == name)
    {
        Some(skill) => Ok(skill.content.clone()),
        None => {
            let available: Vec<&str> = config
                .ecosystem
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect();
            anyhow::bail!(
                "unknown skill `{name}` (available: {})",
                available.join(", ")
            )
        }
    }
}

fn task_spec(config: &Config) -> ToolSpec {
    let subagents: Vec<String> = config
        .ecosystem
        .agents
        .iter()
        .filter(|agent| agent.mode != AgentMode::Primary)
        .map(|agent| match &agent.description {
            Some(description) if !description.is_empty() => {
                format!("{} ({description})", agent.name)
            }
            _ => agent.name.clone(),
        })
        .collect();
    let available = if subagents.is_empty() {
        "none".to_string()
    } else {
        subagents.join(", ")
    };

    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "task".to_string(),
            description: format!(
                "Launch a subagent to handle a focused task in its own context and return its final report. Available subagents: {available}."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "Short 3-5 word description of the task"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The task for the subagent to perform"
                    },
                    "subagent_type": {
                        "type": "string",
                        "description": "Name of the subagent to use"
                    }
                },
                "required": ["description", "prompt", "subagent_type"]
            }),
        },
    }
}

async fn command(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
    events: &UnboundedSender<AgentEvent>,
    arguments: &str,
    depth: usize,
) -> String {
    match command_inner(config, cwd, runtime, events, arguments, depth).await {
        Ok(output) => output,
        Err(err) => format!("error: {err:#}"),
    }
}

async fn command_inner(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
    events: &UnboundedSender<AgentEvent>,
    arguments: &str,
    depth: usize,
) -> Result<String> {
    let args: Value = serde_json::from_str(arguments).context("invalid command arguments")?;
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .context("missing `name` argument")?
        .trim_start_matches('/');
    let extra = args.get("arguments").and_then(Value::as_str).unwrap_or("");
    let input = if extra.trim().is_empty() {
        format!("/{name}")
    } else {
        format!("/{name} {extra}")
    };
    let Some(resolved) = config.ecosystem.resolve_command(&input) else {
        let available: Vec<&str> = config
            .ecosystem
            .commands
            .iter()
            .map(|command| command.name.as_str())
            .chain(
                config
                    .ecosystem
                    .prompt_templates
                    .iter()
                    .filter(|template| config.ecosystem.command(&template.name).is_none())
                    .map(|template| template.name.as_str()),
            )
            .collect();
        anyhow::bail!(
            "unknown command `{name}` (available: {})",
            available.join(", ")
        );
    };

    if resolved.subtask {
        let agent = resolved
            .agent
            .clone()
            .context("subtask command requires an `agent` in its frontmatter")?;
        let task_args = json!({ "prompt": resolved.prompt, "subagent_type": agent }).to_string();
        return task_inner(config, cwd, runtime, events, &task_args, depth).await;
    }
    if let Some(agent_name) = &resolved.agent {
        if let Some(agent) = config.ecosystem.agent(agent_name) {
            return Ok(format!(
                "Operate as the `{agent_name}` agent while carrying out the `/{name}` command.\n\n\
                 {}\n\n{}",
                agent.prompt.trim(),
                resolved.prompt
            ));
        }
    }
    Ok(resolved.prompt)
}

fn command_spec(config: &Config) -> ToolSpec {
    let available: Vec<String> = config
        .ecosystem
        .commands
        .iter()
        .map(|command| command.name.clone())
        .chain(
            config
                .ecosystem
                .prompt_templates
                .iter()
                .filter(|template| config.ecosystem.command(&template.name).is_none())
                .map(|template| template.name.clone()),
        )
        .collect();

    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "command".to_string(),
            description: format!(
                "Invoke a predefined command by name when the user's request matches its purpose. Subtask commands run in an isolated subagent and return their result; other commands return their expanded instructions for you to carry out. Available commands: {}.",
                available.join(", ")
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the command to invoke"
                    },
                    "arguments": {
                        "type": "string",
                        "description": "Arguments expanded into the command's $ARGUMENTS/$1 placeholders"
                    }
                },
                "required": ["name"]
            }),
        },
    }
}

fn skill_spec(config: &Config) -> ToolSpec {
    let available: Vec<&str> = config
        .ecosystem
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();

    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "skill".to_string(),
            description: format!(
                "Load the full instructions for a skill by name before doing work that matches it. Available skills: {}.",
                available.join(", ")
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the skill to load"
                    }
                },
                "required": ["name"]
            }),
        },
    }
}

fn memory(config: &Config, arguments: &str) -> String {
    match memory_inner(config, arguments) {
        Ok(output) => output,
        Err(err) => format!("error: {err:#}"),
    }
}

fn memory_inner(config: &Config, arguments: &str) -> Result<String> {
    let args: Value = serde_json::from_str(arguments).context("invalid memory arguments")?;
    let mode = args
        .get("mode")
        .and_then(Value::as_str)
        .context("missing `mode` argument")?;
    let scope = QueryScope::parse(
        args.get("scope")
            .and_then(Value::as_str)
            .unwrap_or("project"),
    );

    match mode {
        "add" => {
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .context("missing `content` argument")?;
            let tags = args
                .get("tags")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let entry = config.memory.add(content, scope.entry_scope()?, tags)?;
            Ok(format!("stored memory `{}`", entry.id))
        }
        "search" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let results = config.memory.search(query, scope, limit_arg(&args));
            Ok(format_entries(&results))
        }
        "list" => {
            let results = config.memory.list(scope, limit_arg(&args));
            Ok(format_entries(&results))
        }
        "forget" => {
            let id = args
                .get("memoryId")
                .and_then(Value::as_str)
                .context("missing `memoryId` argument")?;
            if config.memory.forget(id)? {
                Ok(format!("forgot memory `{id}`"))
            } else {
                Ok(format!("no memory with id `{id}`"))
            }
        }
        "profile" => match config.memory.profile() {
            Some(entry) => Ok(entry.content),
            None => Ok("no user profile recorded yet".to_string()),
        },
        other => anyhow::bail!("unknown memory mode `{other}`"),
    }
}

fn limit_arg(args: &Value) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(10)
        .clamp(1, 100)
}

fn format_entries(entries: &[crate::memory::MemoryEntry]) -> String {
    if entries.is_empty() {
        return "no matching memories".to_string();
    }
    entries
        .iter()
        .map(|entry| {
            let scope = match entry.scope {
                crate::memory::Scope::Project => "project",
                crate::memory::Scope::User => "user",
            };
            format!("- `{}` [{scope}] {}", entry.id, entry.content)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn memory_spec() -> ToolSpec {
    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "memory".to_string(),
            description: "Persist and retrieve long-term memory across sessions. Modes: `add` stores a note, `search` finds relevant notes, `list` shows recent notes, `forget` deletes a note by id, and `profile` shows the learned user profile. Default scope is the current project; use `user` for cross-project preferences.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["add", "search", "list", "forget", "profile"],
                        "description": "Operation to perform"
                    },
                    "content": {
                        "type": "string",
                        "description": "Memory text to store (for `add`)"
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query (for `search`)"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tags (for `add`)"
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "user", "all-projects"],
                        "description": "Memory scope; defaults to `project`"
                    },
                    "memoryId": {
                        "type": "string",
                        "description": "Entry id (for `forget`)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results (for `search`/`list`)"
                    }
                },
                "required": ["mode"]
            }),
        },
    }
}

fn lsp_spec() -> ToolSpec {
    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "diagnostics".to_string(),
            description: "Fetch language-server diagnostics (errors and warnings) for a file."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path to check, relative to the working directory"
                    }
                },
                "required": ["path"]
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecosystem::{AgentDef, Skill};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn config_with_skill() -> Config {
        let mut config = Config::default();
        config.ecosystem.skills.push(Skill {
            name: "audit".into(),
            description: Some("audits deps".into()),
            content: "Audit the deps.".into(),
        });
        config
    }

    #[test]
    fn skill_tool_returns_content() {
        let config = config_with_skill();
        assert_eq!(skill(&config, r#"{"name":"audit"}"#), "Audit the deps.");
    }

    #[test]
    fn skill_tool_reports_unknown() {
        let config = config_with_skill();
        assert!(skill(&config, r#"{"name":"missing"}"#).starts_with("error: unknown skill"));
    }

    #[test]
    fn task_spec_lists_subagents() {
        let mut config = Config::default();
        config.ecosystem.agents.push(AgentDef {
            name: "reviewer".into(),
            description: Some("reviews code".into()),
            mode: AgentMode::Subagent,
            permission: None,
            prompt: String::new(),
        });
        assert!(task_spec(&config).function.description.contains("reviewer"));
    }

    async fn test_runtime() -> Runtime {
        Runtime {
            mcp: Arc::new(McpRegistry::new(&[])),
            plugins: Arc::new(PluginHost::spawn(&[], &std::env::temp_dir()).await),
            session: None,
            snapshots: None,
            lsp: Arc::new(LspManager::new()),
            approve: Arc::new(|_, _| Box::pin(async { false })),
            steering: Steering::new(),
            follow_ups: Steering::new(),
            cancel: Cancel::new(),
        }
    }

    /// Deadline for a scripted request to arrive, so a test that stops short of
    /// its scripted turns fails instead of waiting out the job timeout.
    const POLL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    /// Serves one SSE response per request, in order, and returns the raw
    /// request texts it saw so a test can assert on what was sent. A request
    /// that never arrives fails the test instead of hanging the runner.
    /// Creates a repository with a committed `catalog.yaml`, so a later edit
    /// produces a real `git diff` hunk.
    /// Runs `git` in `dir` for a test, panicking on failure.
    fn run_git(dir: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git is available for the test");
        assert!(status.success(), "git {args:?} failed");
    }

    fn git_text(dir: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .stderr(std::process::Stdio::null())
            .output()
            .expect("git is available for the test");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Creates a repository with a committed `catalog.yaml`, so a later edit
    /// produces a real `git diff` hunk.
    fn init_git_repo(dir: &std::path::Path) {
        run_git(dir, &["init", "-q"]);
        run_git(dir, &["config", "user.email", "oxide@example.com"]);
        run_git(dir, &["config", "user.name", "Oxide Test"]);
        // A global `commit.gpgsign` would otherwise make commits fail.
        run_git(dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("catalog.yaml"), "old\n").unwrap();
        run_git(dir, &["add", "catalog.yaml"]);
        run_git(dir, &["commit", "-q", "-m", "init"]);
    }

    /// Adds a bare `origin` remote and pushes the current commit, so
    /// [`repo_has_pending_delivery`] starts clean.
    fn add_git_remote(dir: &std::path::Path) {
        let origin = std::path::PathBuf::from(format!("{}.origin.git", dir.display()));
        std::fs::remove_dir_all(&origin).ok();
        let status = std::process::Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&origin)
            .status()
            .expect("git is available for the test");
        assert!(status.success(), "git init --bare failed");
        run_git(dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
        run_git(dir, &["push", "-u", "origin", "HEAD"]);
    }

    async fn sse_server(
        bodies: Vec<String>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for body in bodies {
                let (mut socket, _) = tokio::time::timeout(POLL_TIMEOUT, listener.accept())
                    .await
                    .expect("the agent stopped short of every scripted request")
                    .unwrap();
                seen.push(read_request(&mut socket).await);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
            seen
        });
        (addr, handle)
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        use tokio::io::AsyncReadExt;

        let mut raw = Vec::new();
        let mut expected: Option<usize> = None;
        loop {
            let mut chunk = [0u8; 8192];
            let read = match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            raw.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&raw);
            let Some(head_end) = text.find("\r\n\r\n") else {
                continue;
            };
            if expected.is_none() {
                expected = text[..head_end].lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
            }
            if expected.is_some_and(|len| raw.len() - (head_end + 4) >= len) {
                break;
            }
        }
        String::from_utf8_lossy(&raw).into_owned()
    }

    fn openai_sse(events: &[serde_json::Value]) -> String {
        let mut body = String::new();
        for event in events {
            body.push_str("data: ");
            body.push_str(&event.to_string());
            body.push_str("\n\n");
        }
        body.push_str("data: [DONE]\n\n");
        body
    }

    fn write_call_body(path: &str) -> String {
        // The arguments are a JSON *string*, so the path must be escaped: a
        // Windows temp path (backslashes) would otherwise be unparseable and
        // the write would fail instead of recording an edit.
        let arguments = json!({"path": path, "content": "x"}).to_string();
        openai_sse(&[
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_0", "function": {"name": "write", "arguments": arguments}}]}, "finish_reason": null}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ])
    }

    fn bash_call_body(command: &str) -> String {
        let arguments = json!({"command": command}).to_string();
        openai_sse(&[
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_1", "function": {"name": "bash", "arguments": arguments}}]}, "finish_reason": null}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ])
    }

    fn answer_body(text: &str) -> String {
        openai_sse(&[
            serde_json::json!({"choices": [{"delta": {"content": text}, "finish_reason": null}]}),
            serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ])
    }

    fn empty_body() -> String {
        openai_sse(&[serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]})])
    }

    /// The regression from a real session: the model edited a file and
    /// summarized its work, oxide sent the hidden Definition-of-Done reminder,
    /// and the provider answered the reminder with an empty turn. The summary
    /// must survive as the result instead of the user reading
    /// `the model returned an empty response` after a complete answer.
    #[tokio::test]
    async fn an_empty_answer_to_the_verification_nudge_ends_the_run_quietly() {
        let dir = std::env::temp_dir().join(format!("oxide_nudge_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let edited = dir.join("catalog.yaml");
        let edited = edited.to_str().unwrap().to_string();

        let (addr, server) = sse_server(vec![
            write_call_body(&edited),
            answer_body("Upgrade complete."),
            empty_body(),
            empty_body(),
            empty_body(),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            auto_approve: true,
            ..Config::default()
        };
        let runtime = test_runtime().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            dir.clone(),
            vec![Message::user("upgrade the package")],
            tx,
            runtime,
        )
        .await;

        let mut errors = Vec::new();
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::Error(message) => errors.push(message),
                AgentEvent::Finished(messages) => finished = Some(messages),
                _ => {}
            }
        }
        assert!(errors.is_empty(), "{errors:?}");
        let finished = finished.expect("the run finished");
        assert_eq!(
            finished.last().and_then(|message| message.display()),
            Some("Upgrade complete.".to_string())
        );

        // The reminder really was the request that produced the empty turn.
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 5);
        assert!(requests[2].contains("Before you finish"), "{}", requests[2]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_diff_of_the_edited_file_skips_the_verification_nudge() {
        let dir = std::env::temp_dir().join(format!("oxide_diff_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        // The reported session edited a catalog file and confirmed it with
        // `git diff <file>`. A real repository makes the inspection succeed and
        // show a hunk, which is the evidence the reminder asks for.
        init_git_repo(&dir);
        let edited = dir.join("catalog.yaml");
        let edited = edited.to_str().unwrap().to_string();

        // Write, inspect the result from the shell, summarize. The inspection is
        // the evidence the reminder asks for, so no reminder is sent and the
        // run needs one model call fewer than the version that nudged.
        let (addr, server) = sse_server(vec![
            write_call_body(&edited),
            bash_call_body("git diff catalog.yaml"),
            answer_body("Upgrade complete."),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            auto_approve: true,
            ..Config::default()
        };
        let runtime = test_runtime().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            dir.clone(),
            vec![Message::user("upgrade the package")],
            tx,
            runtime,
        )
        .await;

        let mut errors = Vec::new();
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::Error(message) => errors.push(message),
                AgentEvent::Finished(messages) => finished = Some(messages),
                _ => {}
            }
        }
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            finished
                .expect("the run finished")
                .last()
                .and_then(|m| m.display()),
            Some("Upgrade complete.".to_string())
        );

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            !requests[2].contains("Before you finish"),
            "the shell inspection should have confirmed the edit: {}",
            requests[2]
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn command_tool_expands_and_reports_unknown() {
        let mut config = Config::default();
        config
            .ecosystem
            .commands
            .push(crate::ecosystem::CommandDef {
                name: "lint".into(),
                description: Some("lints the crate".into()),
                template: "Lint $ARGUMENTS now".into(),
                agent: None,
                subtask: false,
            });
        let runtime = test_runtime().await;
        let (events, _rx) = tokio::sync::mpsc::unbounded_channel();

        let output = command(
            &config,
            Path::new("."),
            &runtime,
            &events,
            r#"{"name":"/lint","arguments":"src"}"#,
            0,
        )
        .await;
        assert_eq!(output, "Lint src now");

        let output = command(
            &config,
            Path::new("."),
            &runtime,
            &events,
            r#"{"name":"missing"}"#,
            0,
        )
        .await;
        assert!(output.starts_with("error: unknown command"), "{output}");
    }

    #[test]
    fn command_spec_lists_commands_and_templates() {
        let mut config = Config::default();
        config
            .ecosystem
            .commands
            .push(crate::ecosystem::CommandDef {
                name: "lint".into(),
                description: Some("lints the crate".into()),
                template: "Lint $ARGUMENTS".into(),
                agent: None,
                subtask: false,
            });
        config
            .ecosystem
            .prompt_templates
            .push(crate::ecosystem::PromptTemplate {
                name: "component".into(),
                description: Some("creates a component".into()),
                argument_hint: None,
                body: "Create $1".into(),
            });
        let spec = command_spec(&config);
        assert_eq!(spec.function.name, "command");
        assert!(spec.function.description.contains("lint"));
        assert!(spec.function.description.contains("component"));
    }

    #[test]
    fn only_read_only_tools_are_concurrency_safe() {
        for name in [
            "read",
            "ls",
            "find",
            "grep",
            "webfetch",
            "memory",
            "skill",
            "diagnostics",
        ] {
            assert!(concurrency_safe(name), "{name} should be concurrency-safe");
        }
        for name in [
            "write",
            "edit",
            "bash",
            "task",
            "command",
            "mcp__server__tool",
        ] {
            assert!(!concurrency_safe(name), "{name} must stay sequential");
        }
    }

    fn tool_call(name: &str) -> ToolCall {
        ToolCall {
            id: format!("call_{name}"),
            kind: "function".into(),
            function: crate::llm::FunctionCall {
                name: name.into(),
                arguments: "{}".into(),
            },
        }
    }

    #[test]
    fn batch_runs_group_reads_and_isolate_state_changes() {
        let runs = batch_runs(vec![
            tool_call("read"),
            tool_call("grep"),
            tool_call("write"),
            tool_call("read"),
            tool_call("edit"),
            tool_call("find"),
            tool_call("ls"),
        ]);
        let shape: Vec<Vec<&str>> = runs
            .iter()
            .map(|run| run.iter().map(|call| call.function.name.as_str()).collect())
            .collect();
        assert_eq!(
            shape,
            vec![
                vec!["read", "grep"],
                vec!["write"],
                vec!["read"],
                vec!["edit"],
                vec!["find", "ls"],
            ]
        );
    }

    #[test]
    fn batch_runs_keep_a_lone_read_sequential() {
        let runs = batch_runs(vec![tool_call("read")]);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len(), 1);
    }

    #[test]
    fn snapshot_classification_covers_unknown_side_effects() {
        for name in ["write", "edit", "patch", "bash", "task", "server__tool"] {
            assert!(tool_may_mutate_workspace(name), "{name} may mutate");
        }
        for name in ["read", "ls", "find", "grep", "webfetch", "diagnostics"] {
            assert!(!tool_may_mutate_workspace(name), "{name} is read-only");
        }
    }

    #[test]
    fn batch_terminates_only_when_all_request_it() {
        assert!(!batch_terminates(&[]));
        assert!(!batch_terminates(&[true, false]));
        assert!(batch_terminates(&[true, true]));
    }

    #[test]
    fn cancel_flag_is_shared_and_sticky() {
        let cancel = Cancel::new();
        assert!(!cancel.is_cancelled());
        let shared = cancel.clone();
        shared.cancel();
        assert!(cancel.is_cancelled());
        assert!(shared.is_cancelled());
    }

    #[tokio::test]
    async fn denied_permission_requests_approval() {
        let calls = Arc::new(AtomicUsize::new(0));
        let approve: Approver = Arc::new({
            let calls = Arc::clone(&calls);
            move |tool, subject| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { tool == "bash" && subject == "ls" })
            }
        });

        assert!(permission_granted(Action::Deny, false, &approve, "bash", "ls").await);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn allowed_and_auto_approved_permissions_skip_prompt() {
        let calls = Arc::new(AtomicUsize::new(0));
        let approve: Approver = Arc::new({
            let calls = Arc::clone(&calls);
            move |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { false })
            }
        });

        assert!(permission_granted(Action::Allow, false, &approve, "read", "a.rs").await);
        assert!(permission_granted(Action::Deny, true, &approve, "bash", "ls").await);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn edits_stay_unverified_until_read_back_or_built() {
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/main.rs"}), "ok");
        state.record("write", &json!({"path": "src/lib.rs"}), "ok");
        assert_eq!(state.edited.len(), 2);
        assert!(state.reminder().is_some());
        // The reminder is sent only once.
        assert!(state.reminder().is_none());

        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/main.rs"}), "ok");
        state.record("read", &json!({"path": "src/main.rs"}), "ok");
        assert!(state.edited.is_empty());
        assert!(state.reminder().is_none());

        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/main.rs"}), "ok");
        state.record("bash", &json!({"command": "./gradlew test"}), "[exit: 0]");
        assert!(state.edited.is_empty());
        assert!(state.reminder().is_none());

        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/main.rs"}), "ok");
        state.record("read", &json!({"path": "/repo/src/main.rs"}), "ok");
        assert!(state.edited.is_empty());
        assert!(state.reminder().is_none());

        // A failed edit is not something to verify.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/main.rs"}), "error: no match");
        assert!(state.edited.is_empty());
    }

    #[test]
    fn a_shell_inspection_of_the_edited_file_confirms_it() {
        let diff = "diff --git a/pnpm-workspace.yaml b/pnpm-workspace.yaml\n\
                    @@ -55,7 +55,7 @@\n[exit: 0]";
        // `git diff <path>` shows the change is on disk, so it needs no nudge.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "/repo/pnpm-workspace.yaml"}), "ok");
        state.record(
            "bash",
            &json!({"command": "cd /repo && git diff pnpm-workspace.yaml"}),
            diff,
        );
        assert!(state.edited.is_empty());
        assert!(state.reminder().is_none());

        // So does grepping the file for the new content.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "/repo/src/main.rs"}), "ok");
        state.record(
            "bash",
            &json!({"command": "grep -n \"fn main\" src/main.rs"}),
            "12:fn main() {\n[exit: 0]",
        );
        assert!(state.edited.is_empty());

        // Only the named file is confirmed, and only by a successful command.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "/repo/a.rs"}), "ok");
        state.record("edit", &json!({"path": "/repo/b.rs"}), "ok");
        state.record("bash", &json!({"command": "git diff a.rs"}), diff);
        assert_eq!(state.edited.iter().collect::<Vec<_>>(), vec!["/repo/b.rs"]);

        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "/repo/a.rs"}), "ok");
        state.record("bash", &json!({"command": "git diff a.rs"}), "[exit: 128]");
        assert_eq!(state.edited.len(), 1);

        // A diff of the whole tree does not show whether the hunk was included
        // in the truncated output, and a plain read-only listing is not a look
        // at the content, so neither counts.
        assert!(!inspected_by_shell("git diff", diff, "/repo/a.rs"));
        assert!(!inspected_by_shell(
            "ls -la a.rs",
            "/repo/a.rs\n[exit: 0]",
            "/repo/a.rs"
        ));
        assert!(!inspected_by_shell("git diff a.rs", diff, "/repo/b.rs"));
        assert!(inspected_by_shell("git diff a.rs", diff, "/repo/a.rs"));
        assert!(inspected_by_shell(
            "git show HEAD -- a.rs",
            diff,
            "/repo/a.rs"
        ));

        // A named path is not evidence when the command only mentions it — as
        // the *pattern* a searcher matches, a glob it filters names by, or a
        // diff that shows nothing — because none of those read the change.
        let empty = "[exit: 0]";
        assert!(!inspected_by_shell(
            "grep -rn \"a.rs\" .",
            empty,
            "/repo/a.rs"
        ));
        assert!(!inspected_by_shell(
            "rg --files -g a.rs",
            empty,
            "/repo/a.rs"
        ));
        assert!(!inspected_by_shell(
            "git diff --staged a.rs",
            empty,
            "/repo/a.rs"
        ));
        // `git show HEAD:a.rs` reads the committed blob, not the working file,
        // so it says nothing about an uncommitted edit.
        assert!(!inspected_by_shell(
            "git show HEAD:a.rs",
            diff,
            "/repo/a.rs"
        ));
    }

    #[test]
    fn a_nudge_that_produces_no_answer_ends_the_run_quietly() {
        // The model already summarized its work, so a provider that answers the
        // reminder with nothing must not fail the turn with an error the user
        // reads after a complete answer (see the empty-response regression).
        let empty = anyhow::Error::new(crate::llm::NoAnswer::empty());
        assert!(nudge_failed_quietly(true, &empty));
        // A real failure is still reported, and an ordinary turn never gets
        // this leniency.
        assert!(!nudge_failed_quietly(false, &empty));
        assert!(!nudge_failed_quietly(
            true,
            &anyhow::anyhow!("provider returned 401")
        ));
    }

    #[test]
    fn side_effects_are_confirmed_before_done() {
        // A pull request is unconfirmed until its checks or state are read.
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh pr create --fill"}),
            "[exit: 0]",
        );
        assert!(state.pending.contains("pull request"));
        let reminder = state.reminder().unwrap();
        assert!(reminder.contains("gh pr checks"), "{reminder}");
        assert!(state.reminder().is_none());

        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh pr create --fill"}),
            "[exit: 0]",
        );
        state.record(
            "bash",
            &json!({"command": "gh pr view 42 --json state"}),
            "OPEN",
        );
        assert!(state.pending.is_empty());
        assert!(state.reminder().is_none());

        // A failed command is not an outcome that needs checking.
        let mut state = VerificationState::default();
        state.record("bash", &json!({"command": "gh pr create"}), "[exit: 1]");
        assert!(state.pending.is_empty());
        assert!(state.reminder().is_none());

        assert!(output_failed("error: nope"));
        assert!(output_failed("[exit: 1]"));
        assert!(!output_failed("[exit: 0]"));
    }

    #[test]
    fn done_rules_cover_more_than_pull_requests() {
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "kubectl apply -f k8s.yaml"}),
            "[exit: 0]",
        );
        assert!(state.pending.contains("deployment"));
        state.record(
            "bash",
            &json!({"command": "kubectl rollout status deploy/api"}),
            "ok",
        );
        assert!(state.pending.is_empty());

        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh pr comment 5 --body hi"}),
            "[exit: 0]",
        );
        assert!(state.pending.contains("comment"));
        let reminder = state.reminder().unwrap();
        assert!(reminder.contains("Read the thread back"), "{reminder}");

        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh release create v1.0"}),
            "[exit: 0]",
        );
        assert!(state.pending.contains("release"));

        // Ordinary git commands are not side effects needing a separate check.
        let mut state = VerificationState::default();
        state.record("bash", &json!({"command": "git commit -m x"}), "[exit: 0]");
        assert!(state.pending.is_empty());
    }

    #[test]
    fn blanket_staging_requires_a_scope_review() {
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "git add -A && git commit -m x"}),
            "[exit: 0]",
        );
        assert!(state.pending.contains("scope"));
        let reminder = state.reminder().unwrap();
        assert!(reminder.contains("blanket flag"), "{reminder}");

        let mut state = VerificationState::default();
        state.record("bash", &json!({"command": "git add -A"}), "[exit: 0]");
        state.record("bash", &json!({"command": "git diff --staged"}), "...");
        assert!(state.pending.is_empty());

        // Staging explicit paths is scoped and needs no review.
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "git add src/main.rs"}),
            "[exit: 0]",
        );
        assert!(state.pending.is_empty());
    }

    #[test]
    fn review_edits_are_not_delivered_until_pushed() {
        // A review reply after an edit must be followed by a push, or the
        // branch under review still shows the old code.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/a.rs"}), "ok");
        state.record(
            "bash",
            &json!({"command": "gh pr comment 5 --body 'fixed'"}),
            "[exit: 0]",
        );
        let reminder = state.reminder().unwrap();
        assert!(reminder.contains("git push"), "{reminder}");
        assert!(state.reminder().is_none());

        // A push before the reply already delivered the edit, so no nudge to
        // push again.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/a.rs"}), "ok");
        state.record("bash", &json!({"command": "git push"}), "[exit: 0]");
        state.record(
            "bash",
            &json!({"command": "gh pr comment 5 --body hi"}),
            "[exit: 0]",
        );
        let reminder = state.reminder().unwrap_or_default();
        assert!(!reminder.contains("git push"), "{reminder}");

        // An edit made after the reply needs its own push.
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh pr review 5 --comment"}),
            "[exit: 0]",
        );
        state.record("edit", &json!({"path": "src/a.rs"}), "ok");
        assert!(state.reminder().unwrap().contains("git push"));

        // A review with no code change has nothing to push.
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh pr comment 5 --body hi"}),
            "[exit: 0]",
        );
        let reminder = state.reminder().unwrap_or_default();
        assert!(!reminder.contains("git push"), "{reminder}");
    }

    #[test]
    fn review_replies_are_held_until_the_code_is_pushed() {
        assert!(posts_review_reply("gh pr comment 5 --body fixed"));
        assert!(posts_review_reply(
            "gh api -X POST repos/o/r/pulls/5/comments/1/replies -f body=x"
        ));
        // Reading a thread is a GET, not a posted reply.
        assert!(!posts_review_reply(
            "gh api repos/o/r/pulls/5/comments/1/replies"
        ));
        assert!(!posts_review_reply("gh pr view 5 --comments"));
        assert!(!posts_review_reply("git push"));

        // A reply while a tracked edit is unpushed is held.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/a.rs"}), "ok");
        assert!(state.blocks_review_reply("gh pr comment 5 --body fixed", false));

        // Repository state holds it even when no tracked edit was seen.
        let mut state = VerificationState::default();
        assert!(state.blocks_review_reply("gh pr comment 5 --body fixed", true));

        // A real push unblocks the tracked edit.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/a.rs"}), "ok");
        state.record("bash", &json!({"command": "git push"}), "[exit: 0]");
        assert!(!state.blocks_review_reply("gh pr comment 5 --body fixed", false));

        // `echo git push` is not a push, and does not clear the guard.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/a.rs"}), "ok");
        state.record("bash", &json!({"command": "echo git push"}), "[exit: 0]");
        assert!(state.blocks_review_reply("gh pr comment 5 --body fixed", false));

        // A reply with no pending code change is allowed.
        let mut state = VerificationState::default();
        assert!(!state.blocks_review_reply("gh pr comment 5 --body hi", false));
    }

    #[tokio::test]
    async fn repo_delivery_pending_tracks_uncommitted_and_unpushed_work() {
        let dir = std::env::temp_dir().join(format!("oxide_pending_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        add_git_remote(&dir);
        assert!(!repo_has_pending_delivery(&dir).await, "clean and pushed");

        std::fs::write(dir.join("catalog.yaml"), "changed\n").unwrap();
        assert!(repo_has_pending_delivery(&dir).await, "uncommitted change");

        run_git(&dir, &["add", "catalog.yaml"]);
        run_git(&dir, &["commit", "-q", "-m", "fix"]);
        assert!(repo_has_pending_delivery(&dir).await, "unpushed commit");

        run_git(&dir, &["push"]);
        assert!(!repo_has_pending_delivery(&dir).await, "pushed");

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(format!("{}.origin.git", dir.display())).ok();
    }

    #[tokio::test]
    async fn repo_delivery_ignores_a_pushed_branch_the_remote_tracking_ref_misses() {
        let dir = std::env::temp_dir().join(format!("oxide_pending_sb_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        add_git_remote(&dir);

        // Simulate a single-branch clone: the fetch refspec maps only the
        // default branch, so pushing `feature` never creates `origin/feature`.
        let default = git_text(&dir, &["symbolic-ref", "--short", "HEAD"]);
        run_git(
            &dir,
            &[
                "config",
                "remote.origin.fetch",
                &format!("+refs/heads/{default}:refs/remotes/origin/{default}"),
            ],
        );
        run_git(&dir, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(dir.join("catalog.yaml"), "feature\n").unwrap();
        run_git(&dir, &["add", "catalog.yaml"]);
        run_git(&dir, &["commit", "-q", "-m", "feature"]);
        assert!(
            repo_has_pending_delivery(&dir).await,
            "an unpushed feature commit is held"
        );

        run_git(&dir, &["push", "-q", "origin", "feature"]);
        // The pushed branch is not visible locally...
        assert!(
            std::process::Command::new("git")
                .args(["rev-parse", "--verify", "refs/remotes/origin/feature"])
                .current_dir(&dir)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("git runs")
                .code()
                .is_none_or(|code| code != 0),
            "no remote-tracking ref for the feature branch"
        );
        // ...but asking the remote clears the guard.
        assert!(
            !repo_has_pending_delivery(&dir).await,
            "a commit already on the remote is delivered"
        );

        // A second commit lands on the remote branch, then the local branch is
        // rewound behind it: the older local commit is still delivered because
        // it is an ancestor of the remote tip.
        std::fs::write(dir.join("catalog.yaml"), "later\n").unwrap();
        run_git(&dir, &["add", "catalog.yaml"]);
        run_git(&dir, &["commit", "-q", "-m", "later"]);
        run_git(&dir, &["push", "-q", "origin", "feature"]);
        run_git(&dir, &["reset", "-q", "--hard", "HEAD~1"]);
        assert!(
            !repo_has_pending_delivery(&dir).await,
            "a commit behind the remote tip is still delivered"
        );

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(format!("{}.origin.git", dir.display())).ok();
    }

    #[tokio::test]
    async fn a_review_reply_before_the_push_is_held() {
        let dir = std::env::temp_dir().join(format!("oxide_reply_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        add_git_remote(&dir);
        let edited = dir.join("catalog.yaml");
        let edited = edited.to_str().unwrap().to_string();

        let (addr, server) = sse_server(vec![
            write_call_body(&edited),
            // The reply is refused while the edit is uncommitted.
            bash_call_body("gh pr comment 5 --body 'fixed'"),
            bash_call_body("git diff catalog.yaml"),
            // Committing is not enough: the commit is not pushed yet.
            bash_call_body("git add catalog.yaml && git commit -m 'fix'"),
            // ...so the reply is still refused.
            bash_call_body("gh pr comment 5 --body 'fixed'"),
            // Pushing the commit delivers it.
            bash_call_body("git push"),
            // Now the reply is allowed.
            bash_call_body("gh pr comment 5 --body 'fixed'"),
            bash_call_body("gh pr view 5 --comments"),
            answer_body("Done."),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            auto_approve: true,
            ..Config::default()
        };
        let runtime = test_runtime().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            dir.clone(),
            vec![Message::user("address the review")],
            tx,
            runtime,
        )
        .await;

        let mut errors = Vec::new();
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::Error(message) => errors.push(message),
                AgentEvent::Finished(messages) => finished = Some(messages),
                _ => {}
            }
        }
        assert!(errors.is_empty(), "{errors:?}");
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 9);
        assert!(
            requests[2].contains("commit and push the code changes before replying"),
            "{}",
            requests[2]
        );
        assert!(
            requests[5].contains("commit and push the code changes before replying"),
            "{}",
            requests[5]
        );
        assert!(
            !requests[8].contains("Before you finish"),
            "{}",
            requests[8]
        );
        assert_eq!(
            finished
                .expect("the run finished")
                .last()
                .and_then(|m| m.display()),
            Some("Done.".to_string())
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn same_path_ignores_the_separator_style() {
        assert!(same_path(r"C:\repo\src\main.rs", "src/main.rs"));
        assert!(same_path(r"C:\repo\src\main.rs", r"src\main.rs"));
        assert!(same_path("/repo/a.rs", "/repo/a.rs"));
        assert!(!same_path(r"C:\repo\a.rs", "b.rs"));
    }

    #[test]
    fn repeated_verifiers_share_a_key_across_output_plumbing() {
        let base = verification_key("./gradlew test --tests '*Boutique*'").unwrap();
        assert_eq!(
            verification_key("./gradlew test --tests '*Boutique*' 2>&1 | tail -50").unwrap(),
            base
        );
        assert_eq!(
            verification_key("./gradlew test --tests '*Boutique*' | grep PASSED | wc -l").unwrap(),
            base
        );
        // A different verifier is a different key, and non-verifiers have none.
        assert_ne!(
            verification_key("./gradlew test --tests '*Villa*'").unwrap(),
            base
        );
        assert!(verification_key("git status").is_none());
        assert!(verification_key("echo hello").is_none());

        // A pipe inside a quoted selector is part of the key, not plumbing.
        let quoted = verification_key("cargo test -- --exact 'foo|bar'").unwrap();
        assert_eq!(quoted, "cargo test -- --exact 'foo|bar'");
        assert_ne!(
            verification_key("cargo test -- --exact 'foo'").unwrap(),
            quoted
        );
    }

    #[test]
    fn a_denied_verifier_is_not_recorded_as_run() {
        let mut seen = BTreeSet::new();
        // A denied call never dispatched, so it must not count as a run.
        assert!(!note_verifier(&mut seen, false, "cargo test"));
        // The first real run is new; repeating it is flagged.
        assert!(!note_verifier(&mut seen, true, "cargo test"));
        assert!(note_verifier(&mut seen, true, "cargo test 2>&1 | tail -5"));
        assert!(!note_verifier(&mut seen, true, "cargo build"));
    }

    #[test]
    fn only_a_successful_mutation_invalidates_a_verifier() {
        // A successful edit makes the prior build/test result stale, so the next
        // run must be treated as fresh.
        assert!(mutation_invalidates_verifier(
            "edit",
            "applied 1 replacement"
        ));
        assert!(mutation_invalidates_verifier("write_file", "wrote a.rs"));
        assert!(mutation_invalidates_verifier("patch", "patched a.rs"));
        // A failed mutation left the workspace unchanged and must not.
        assert!(!mutation_invalidates_verifier(
            "edit",
            "error: oldText not found"
        ));
        assert!(!mutation_invalidates_verifier(
            "write_file",
            "error: denied"
        ));
        // Non-mutating tools never invalidate.
        assert!(!mutation_invalidates_verifier("bash", "ok"));
        assert!(!mutation_invalidates_verifier("read_file", "1|line"));
    }

    #[test]
    fn commands_that_verify_are_recognized() {
        assert!(looks_like_verification_command("./gradlew test"));
        assert!(looks_like_verification_command("cargo build"));
        assert!(looks_like_verification_command("npm run lint"));
        assert!(looks_like_verification_command("mvn verify"));
        assert!(looks_like_verification_command("./gradlew spotlessCheck"));
        assert!(looks_like_verification_command(
            "./gradlew testDebugUnitTest"
        ));
        assert!(!looks_like_verification_command("./gradlew spotlessApply"));
        assert!(!looks_like_verification_command(
            "gh api repos/o/r/pulls/1/comments"
        ));
        assert!(!looks_like_verification_command("git status"));
        assert!(!looks_like_verification_command("git log --grep latest"));
    }

    #[test]
    fn steering_queue_drains_in_order() {
        let steering = Steering::new();
        steering.push(Message::user("first"));
        steering.push(Message::user("second"));
        let drained = steering.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].display().as_deref(), Some("first"));
        assert_eq!(drained[1].display().as_deref(), Some("second"));
        assert!(steering.drain().is_empty());
    }

    #[test]
    fn follow_ups_are_separate_from_steering() {
        let steering = Steering::new();
        let follow_ups = Steering::new();
        steering.push(Message::user("steer"));
        follow_ups.push(Message::user("later"));
        assert_eq!(steering.drain().len(), 1);
        assert_eq!(follow_ups.drain().len(), 1);
        assert!(steering.drain().is_empty());
        assert!(follow_ups.drain().is_empty());
    }

    #[test]
    fn memory_tool_add_search_forget() {
        let root = std::env::temp_dir().join(format!("oxide_agent_mem_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            memory: crate::memory::MemoryStore::open(root.clone(), &root),
            ..Default::default()
        };

        let added = memory(
            &config,
            r#"{"mode":"add","content":"prefer anyhow for errors"}"#,
        );
        assert!(added.starts_with("stored memory"), "{added}");

        let found = memory(&config, r#"{"mode":"search","query":"anyhow errors"}"#);
        assert!(found.contains("prefer anyhow for errors"), "{found}");

        let listed = memory(&config, r#"{"mode":"list"}"#);
        assert!(listed.contains("prefer anyhow for errors"));

        std::fs::remove_dir_all(&root).ok();
    }
}
