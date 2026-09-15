use crate::config::Config;
use crate::dcp::{self, DcpState};
use crate::ecosystem::AgentMode;
use crate::llm::{FunctionSpec, LlmClient, Message, ToolCall, ToolSpec};
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
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

const MAX_STEPS: usize = 25;
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
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Text(String),
    Thought {
        millis: u64,
    },
    ToolCall {
        name: String,
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
    },
    /// Token usage reported by the provider for the turn just completed.
    Usage {
        input: u64,
        output: u64,
    },
    Error(String),
    Finished(Vec<Message>),
}

/// Drive one conversation turn to completion: stream assistant output, execute
/// any requested tools, feed the results back, and repeat until the model stops
/// asking for tools (or the step budget is exhausted).
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
                AgentEvent::Thought { millis } => {
                    let _ = tx.send(AgentEvent::Thought { millis });
                }
                AgentEvent::ToolCall { name, args } => {
                    let _ = tx.send(AgentEvent::ToolCall { name, args });
                }
                AgentEvent::ToolProgress { name, chunk } => {
                    let _ = tx.send(AgentEvent::ToolProgress { name, chunk });
                }
                AgentEvent::ToolResult {
                    name,
                    args,
                    output,
                    diff,
                } => {
                    let _ = tx.send(AgentEvent::ToolResult {
                        name,
                        args,
                        output,
                        diff,
                    });
                }
                AgentEvent::Usage { input, output } => {
                    let _ = tx.send(AgentEvent::Usage { input, output });
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
    let client = LlmClient::new(config.clone());
    let permissions = Permissions::from_config(&config);
    let dcp_enabled = depth == 0 && config.dcp.enabled;
    let mut dcp_state = if dcp_enabled {
        runtime
            .session
            .as_ref()
            .map(|log| log.dcp_state())
            .unwrap_or_default()
    } else {
        DcpState::default()
    };
    let mut tool_specs = tools::specs(&runtime.mcp);
    if depth < MAX_TASK_DEPTH
        && config
            .ecosystem
            .agents
            .iter()
            .any(|agent| agent.mode != AgentMode::Primary)
    {
        tool_specs.push(task_spec(&config));
    }
    if !config.ecosystem.skills.is_empty() {
        tool_specs.push(skill_spec(&config));
    }
    tool_specs.push(memory_spec());
    tool_specs.push(lsp_spec());
    if dcp_enabled {
        if let Some(spec) = dcp::compress_spec(&config.dcp) {
            tool_specs.push(spec);
        }
    }
    if config.tool_filter.is_restrictive() {
        tool_specs.retain(|spec| config.tool_filter.permits(&spec.function.name));
    }
    let mut messages = history;

    for _ in 0..MAX_STEPS {
        for steered in runtime.steering.drain() {
            record(&runtime.session, depth, &steered);
            messages.push(steered);
        }
        if depth == 0 && !config.dcp.enabled && crate::compact::needs_compaction(&messages) {
            if let Ok(compacted) = crate::compact::compact(&config, messages.clone()).await {
                messages = compacted;
            }
        }
        let mut request = Vec::with_capacity(messages.len() + 2);
        request.push(Message::system(config.compose_system_prompt()));
        if dcp_enabled {
            request.extend(dcp::prune(&messages, &dcp_state, &config.dcp));
            let iterations = messages
                .iter()
                .rev()
                .take_while(|message| message.role != "user")
                .filter(|message| message.role == "assistant")
                .count();
            if let Some(nudge) = dcp::nudge(&messages, &dcp_state, &config.dcp, iterations) {
                request.push(Message::system(nudge));
            }
        } else {
            request.extend(messages.iter().cloned());
        }

        let started = std::time::Instant::now();
        let mut thought_sent = false;
        let turn = match client
            .stream_chat(&request, &tool_specs, |delta| {
                if !thought_sent {
                    thought_sent = true;
                    let _ = tx.send(AgentEvent::Thought {
                        millis: started.elapsed().as_millis() as u64,
                    });
                }
                let _ = tx.send(AgentEvent::Text(delta));
            })
            .await
        {
            Ok(turn) => turn,
            Err(err) => {
                let _ = tx.send(AgentEvent::Error(format!("{err:#}")));
                let _ = tx.send(AgentEvent::Finished(messages));
                return;
            }
        };
        if !thought_sent {
            let _ = tx.send(AgentEvent::Thought {
                millis: started.elapsed().as_millis() as u64,
            });
        }

        let tool_calls = turn.tool_calls.clone();
        let assistant = Message::assistant(turn.content, tool_calls.clone())
            .with_thinking(turn.thinking.clone());
        record(&runtime.session, depth, &assistant);
        messages.push(assistant);
        if turn.usage.total() > 0 {
            let _ = tx.send(AgentEvent::Usage {
                input: turn.usage.input,
                output: turn.usage.output,
            });
        }

        if tool_calls.is_empty() {
            let steered = runtime.steering.drain();
            if steered.is_empty() {
                // Follow-up messages are delivered only once all work is done,
                // so they are drained here, just before finishing.
                let follow_ups = runtime.follow_ups.drain();
                if follow_ups.is_empty() {
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

        let mut terminated: Vec<bool> = Vec::with_capacity(tool_calls.len());
        let mut snapshot_needed = depth == 0 && runtime.plugins.is_active();
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
                let args =
                    serde_json::from_str::<Value>(&call.function.arguments).unwrap_or(Value::Null);
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
                        Prepared::Immediate(output) => output,
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
                            let mut output =
                                dispatch(&config, &cwd, &runtime, &call, depth, &progress).await;
                            if let Some(result) =
                                runtime.plugins.tool_after(&name, &args, &output.text).await
                            {
                                output.text = result.output;
                                output.terminate |= result.terminate;
                            }
                            output
                        }
                    }
                }));
            }

            for (handle, original) in handles.into_iter().zip(&tool_calls) {
                let output = match handle.await {
                    Ok(output) => output,
                    Err(err) => tools::ToolOutput::text(format!("error: tool task failed: {err}")),
                };
                terminated.push(output.terminate);
                let _ = tx.send(AgentEvent::ToolResult {
                    name: original.function.name.clone(),
                    args: original.function.arguments.clone(),
                    output: output.text.clone(),
                    diff: output.diff.clone(),
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
                let args =
                    serde_json::from_str::<Value>(&call.function.arguments).unwrap_or(Value::Null);
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
                let mut output = if name == "compress" && dcp_enabled {
                    match dcp::apply_compress(
                        &mut dcp_state,
                        &messages,
                        &config.dcp,
                        &effective_args,
                    ) {
                        Ok(text) => {
                            if let Some(log) = &runtime.session {
                                if let Some(compression) = dcp_state.compressions.last() {
                                    let _ = log.append_dcp(compression);
                                }
                            }
                            tools::ToolOutput::text(text)
                        }
                        Err(err) => tools::ToolOutput::text(format!("error: {err:#}")),
                    }
                } else {
                    if permission_granted(
                        permissions.decide(&name, &subject),
                        config.auto_approve,
                        &runtime.approve,
                        &name,
                        &subject,
                    )
                    .await
                    {
                        snapshot_needed |= tool_may_mutate_workspace(&name);
                        dispatch(&config, &cwd, &runtime, &call, depth, &progress).await
                    } else {
                        tools::ToolOutput::text(format!("error: permission denied for `{name}`"))
                    }
                };
                let canonical_name = crate::tools::canonical_tool_name(&name);
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
                terminated.push(output.terminate);
                let text = output.text.clone();

                let _ = tx.send(AgentEvent::ToolResult {
                    name,
                    args: serde_json::to_string(&effective_args).unwrap_or_default(),
                    output: text.clone(),
                    diff: output.diff.clone(),
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

        if snapshot_needed {
            if let Some(snapshots) = &runtime.snapshots {
                let _ = snapshots.commit("turn");
            }
        }

        if batch_terminates(&terminated) {
            let _ = tx.send(AgentEvent::Finished(messages));
            return;
        }
    }

    let _ = tx.send(AgentEvent::Error(format!(
        "reached the maximum of {MAX_STEPS} steps without finishing"
    )));
    let _ = tx.send(AgentEvent::Finished(messages));
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

async fn dispatch(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
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
            task(config, cwd, runtime, &call.function.arguments, depth).await,
        ),
        "skill" => tools::ToolOutput::text(skill(config, &call.function.arguments)),
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
    if depth == 0 {
        if let Some(log) = session {
            let _ = log.append(message);
        }
    }
}

async fn task(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
    arguments: &str,
    depth: usize,
) -> String {
    match task_inner(config, cwd, runtime, arguments, depth).await {
        Ok(output) => output,
        Err(err) => format!("error: {err:#}"),
    }
}

async fn task_inner(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
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
            AgentEvent::ToolCall { .. }
            | AgentEvent::ToolProgress { .. }
            | AgentEvent::ToolResult { .. }
            | AgentEvent::Usage { .. }
            | AgentEvent::Thought { .. } => {}
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
            "compress",
            "mcp__server__tool",
        ] {
            assert!(!concurrency_safe(name), "{name} must stay sequential");
        }
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
