//! The `--mode rpc` contracts a client drives oxide through.
//!
//! The VS Code extension runs `oxide --mode rpc` and depends on both of these:
//! the CLI emits an `approval_request` mid-turn and reads the answer off the same
//! stdin channel, and it writes a finished turn's change listing behind that
//! turn's own last event. This test runs the real binary against a scripted
//! model, so a change to the framing (the event fields, the answer shape, the
//! flag that turns prompting on, the order the frames leave in) fails here
//! instead of in the editor.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

/// Every wait is bounded: a protocol break should fail the test, not hang the
/// runner until the job times out.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Serves one scripted SSE response per request on a background thread, and
/// returns the request bodies it read so a test can assert what the run sent.
fn serve(responses: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let mut bodies = Vec::new();
        for body in responses {
            let Ok((mut socket, _)) = listener.accept() else {
                break;
            };
            socket
                .set_read_timeout(Some(TIMEOUT))
                .expect("a read timeout");
            bodies.push(read_request(&mut socket));
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes());
            let _ = socket.flush();
        }
        bodies
    });
    (format!("http://{addr}"), handle)
}

/// Consumes one HTTP request so the client sees a complete exchange, and returns
/// its body.
fn read_request(socket: &mut std::net::TcpStream) -> String {
    use std::io::Read;

    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut expected: Option<usize> = None;
    loop {
        let read = match socket.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
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
    let text = String::from_utf8_lossy(&raw).to_string();
    match text.find("\r\n\r\n") {
        Some(head_end) => text[head_end + 4..].to_string(),
        None => String::new(),
    }
}

/// A 1x1 PNG, small enough that `media::optimize_image` passes it through
/// untouched (and so has no dependency on a platform image tool).
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

fn openai_sse(events: &[Value]) -> String {
    let mut body = String::new();
    for event in events {
        body.push_str(&format!("data: {event}\n\n"));
    }
    body
}

/// A model step that asks to `write` a file, so the turn needs approval.
fn write_call_body(path: &str) -> String {
    let arguments = json!({"path": path, "content": "approved\n"}).to_string();
    openai_sse(&[
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_0", "function": {"name": "write", "arguments": arguments}}]}, "finish_reason": null}]}),
        json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
    ])
}

/// A model step that runs a shell command.
fn bash_call_body(command: &str) -> String {
    let arguments = json!({ "command": command }).to_string();
    openai_sse(&[
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_0", "function": {"name": "bash", "arguments": arguments}}]}, "finish_reason": null}]}),
        json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
    ])
}

fn answer_body(text: &str) -> String {
    openai_sse(&[
        json!({"choices": [{"delta": {"content": text}, "finish_reason": null}]}),
        json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
    ])
}

/// A tool call cut off exactly at the default 8,192-token response budget.
/// The RPC client must see the retry, never this incomplete call.
fn output_limited_tool_call_body() -> String {
    openai_sse(&[
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_0", "function": {"name": "write", "arguments": "{\"path\":\"unfinished"}}]}, "finish_reason": null}]}),
        json!({"choices": [{"delta": {}, "finish_reason": "length"}], "usage": {"prompt_tokens": 10, "completion_tokens": 8192}}),
    ])
}

struct Rpc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Rpc {
    fn send(&mut self, request: Value) {
        writeln!(self.stdin, "{request}").expect("the run accepts the request");
        self.stdin.flush().expect("the request is flushed");
    }

    /// Reads one JSONL event.
    fn event(&mut self) -> Value {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("an event line from the run");
        assert!(!line.is_empty(), "the run closed its event stream early");
        serde_json::from_str(&line).unwrap_or_else(|err| panic!("bad event line {line:?}: {err}"))
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "{}", json!({"type": "quit"}));
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A temp directory that is removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("oxide_rpc_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a writable temp dir");
        Self(dir)
    }

    fn path(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn start(config_home: &PathBuf, cwd: &PathBuf, base_url: &str, ask: bool) -> Rpc {
    start_as(
        config_home,
        cwd,
        base_url,
        ask,
        "deepseek",
        "deepseek-flash",
    )
}

fn start_as(
    config_home: &PathBuf,
    cwd: &PathBuf,
    base_url: &str,
    ask: bool,
    provider: &str,
    model: &str,
) -> Rpc {
    // A private config directory keeps the test away from the developer's own
    // credentials, settings and MCP servers, whatever platform this runs on.
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxide"));
    command
        .arg("--mode")
        .arg("rpc")
        .arg("--no-session")
        .arg(if ask {
            "--ask-approvals"
        } else {
            "--no-ask-approvals"
        })
        .current_dir(cwd)
        .env("HOME", config_home)
        .env("XDG_CONFIG_HOME", config_home)
        .env("APPDATA", config_home)
        .env("LOCALAPPDATA", config_home)
        .env("OXIDE_PROVIDER", provider)
        .env("OXIDE_MODEL", model)
        .env("OXIDE_BASE_URL", base_url)
        .env("OXIDE_API_KEY", "sk-test")
        .env_remove("OXIDE_SETTINGS_FILE")
        .env_remove("OXIDE_ASK_APPROVALS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("the oxide binary runs");
    let stdin = child.stdin.take().expect("stdin is piped");
    let stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    Rpc {
        child,
        stdin,
        stdout,
    }
}

/// The prompt is driven to completion, and every event is read until the turn
/// ends, so a missing `agent_end` fails the test rather than hanging it.
fn run_turn(rpc: &mut Rpc, prompt: &str, approvals: &mut Vec<Value>) -> Vec<Value> {
    rpc.send(json!({"type": "prompt", "message": prompt}));
    drain_turn(rpc, approvals)
}

/// Reads a turn to its end. The request that started it is the caller's to send,
/// so a test can put images on it.
fn drain_turn(rpc: &mut Rpc, approvals: &mut Vec<Value>) -> Vec<Value> {
    let mut events = Vec::new();
    loop {
        let event = rpc.event();
        match event["type"].as_str() {
            Some("approval_request") => {
                approvals.push(event.clone());
                let id = event["id"].as_u64().expect("a request id");
                rpc.send(json!({"type": "approval", "id": id, "decision": "once"}));
            }
            Some("agent_end") => {
                events.push(event);
                return events;
            }
            _ => events.push(event),
        }
    }
}

#[test]
fn an_rpc_client_answers_a_tool_approval() {
    let project = TempDir::new("project");
    let config_home = TempDir::new("config");
    let target = project.path().join("approved.txt").display().to_string();
    let (base_url, server) = serve(vec![write_call_body(&target), answer_body("Done.")]);

    let mut rpc = start(config_home.path(), project.path(), &base_url, true);
    let mut approvals = Vec::new();
    let events = run_turn(&mut rpc, "write the file", &mut approvals);

    assert_eq!(approvals.len(), 1, "one tool was gated: {approvals:?}");
    assert_eq!(approvals[0]["toolName"], "write");
    assert_eq!(approvals[0]["detail"], target);

    // The answer released the tool, so the file it wrote exists and the turn
    // ended with the model's summary.
    assert!(
        project.path().join("approved.txt").is_file(),
        "the approved tool ran"
    );
    let end = events.last().expect("an end event");
    assert_eq!(end["type"], "agent_end");
    assert_eq!(
        server.join().unwrap().len(),
        2,
        "both model steps were served"
    );
}

#[test]
fn without_the_approval_flag_no_prompt_is_emitted() {
    let project = TempDir::new("project_plain");
    let config_home = TempDir::new("config_plain");
    let target = project.path().join("unasked.txt").display().to_string();
    // A client that does not ask gets the configured auto-approval instead of a
    // prompt it would not understand: the tool runs and the turn finishes.
    let (base_url, server) = serve(vec![write_call_body(&target), answer_body("Done.")]);

    let mut rpc = start(config_home.path(), project.path(), &base_url, false);
    let mut approvals = Vec::new();
    let events = run_turn(&mut rpc, "write the file", &mut approvals);

    assert!(approvals.is_empty(), "nothing is asked without the flag");
    assert!(project.path().join("unasked.txt").is_file(), "the tool ran");
    assert_eq!(events.last().expect("an end event")["type"], "agent_end");
    assert_eq!(
        server.join().unwrap().len(),
        2,
        "both model steps were served"
    );
}

#[test]
fn an_rpc_edit_skips_unchanged_blocks_without_retrying_the_batch() {
    let project = TempDir::new("project_noop_edit");
    let config_home = TempDir::new("config_noop_edit");
    let target = project.path().join("example.txt");
    std::fs::write(&target, "header\nbefore\n").unwrap();
    let arguments = json!({"path": target, "edits": [
        {"oldText": "header", "newText": "header"},
        {"oldText": "header\nbefore", "newText": "header\nafter"}
    ]})
    .to_string();
    let edit = openai_sse(&[
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_0", "function": {"name": "edit", "arguments": arguments}}]}, "finish_reason": null}]}),
        json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
    ]);
    let (base_url, server) = serve(vec![edit, answer_body("Done.")]);
    let mut rpc = start(config_home.path(), project.path(), &base_url, false);
    let events = run_turn(&mut rpc, "edit the file", &mut Vec::new());
    let result = events
        .iter()
        .find(|event| event["type"] == "tool_execution_end")
        .unwrap();
    assert_eq!(result["toolName"], "edit");
    assert_eq!(result["isError"], false);
    assert!(result["result"].as_str().unwrap().contains("1 block(s)"));
    assert_eq!(std::fs::read_to_string(target).unwrap(), "header\nafter\n");
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2, "the batch needed no repair round trip");
    assert!(requests[1].contains("Successfully replaced 1 block(s)"));
}

/// VS Code consumes this exact RPC stream. An output-limited tool call is
/// retried inside the shared core, and only the replacement response may cross
/// the process boundary into the extension.
#[test]
fn an_rpc_client_never_receives_an_output_limited_tool_call() {
    let project = TempDir::new("project_output_limit");
    let config_home = TempDir::new("config_output_limit");
    let (base_url, server) = serve(vec![
        output_limited_tool_call_body(),
        answer_body("Recovered."),
    ]);

    let mut rpc = start(config_home.path(), project.path(), &base_url, false);
    let events = run_turn(&mut rpc, "finish safely", &mut Vec::new());

    assert!(
        events
            .iter()
            .any(|event| event["type"] == "auto_retry_start"),
        "the extension is told to discard the failed attempt: {events:?}"
    );
    assert!(
        events.iter().all(|event| event["type"] != "tool_call"),
        "the truncated tool call never reaches the extension: {events:?}"
    );
    let answer = events
        .iter()
        .filter(|event| event["type"] == "message_update")
        .filter_map(|event| event["assistantMessageEvent"]["delta"].as_str())
        .collect::<String>();
    assert_eq!(answer, "Recovered.");

    let bodies = server.join().unwrap();
    assert_eq!(bodies.len(), 2, "the core retried the model response");
    let budgets = bodies
        .iter()
        .map(|body| serde_json::from_str::<Value>(body).unwrap()["max_tokens"].as_u64())
        .collect::<Vec<_>>();
    assert_eq!(budgets, vec![Some(8192), Some(16384)]);
}

#[test]
fn a_denial_message_reaches_the_model() {
    let project = TempDir::new("project_deny");
    let config_home = TempDir::new("config_deny");
    let target = project.path().join("denied.txt").display().to_string();
    let (base_url, server) = serve(vec![write_call_body(&target), answer_body("Understood.")]);

    let mut rpc = start(config_home.path(), project.path(), &base_url, true);
    rpc.send(json!({"type": "prompt", "message": "write the file"}));
    loop {
        let event = rpc.event();
        match event["type"].as_str() {
            Some("approval_request") => {
                let id = event["id"].as_u64().expect("a request id");
                rpc.send(json!({
                    "type": "approval",
                    "id": id,
                    "decision": "deny",
                    "message": "use ls instead",
                }));
            }
            Some("agent_end") => break,
            _ => {}
        }
    }
    assert!(
        !project.path().join("denied.txt").exists(),
        "the denied tool did not run"
    );

    let bodies = server.join().unwrap();
    assert_eq!(
        bodies.len(),
        2,
        "the model was asked again after the denial"
    );
    let second: Value = serde_json::from_str(&bodies[1]).expect("the request is JSON");
    let guided = second["messages"]
        .as_array()
        .expect("the request carries messages")
        .iter()
        .any(|message| {
            message["role"] == "user" && message["content"].as_str() == Some("use ls instead")
        });
    assert!(
        guided,
        "the denial's guidance is in the next request: {}",
        bodies[1]
    );
}

/// A finished turn's change listing travels on the same channel as the run's own
/// events, just ahead of `agent_end`: a client draws a card for the turn it
/// belongs to, so the listing arrives while that run is still the one on screen
/// and the turn's last frame is the end event.
#[test]
fn a_turn_change_listing_follows_the_run_that_produced_it() {
    let project = TempDir::new("project_changes");
    let config_home = TempDir::new("config_changes");
    let target = project.path().join("changed.txt").display().to_string();
    let (base_url, server) = serve(vec![write_call_body(&target), answer_body("Done.")]);

    let mut rpc = start(config_home.path(), project.path(), &base_url, false);
    rpc.send(json!({"type": "prompt", "message": "write the file"}));
    let mut types = Vec::new();
    loop {
        let event = rpc.event();
        types.push(event["type"].as_str().unwrap_or("?").to_string());
        if event["type"] == "turn_changes" {
            // The listing names the file the turn wrote, against a baseline a
            // client can read that file's left side from.
            assert!(
                event["changes"]["files"]
                    .as_array()
                    .is_some_and(|files| files.iter().any(|file| file["path"] == "changed.txt")),
                "the listing names the file the turn wrote: {event}"
            );
            assert!(
                event["baseline"]
                    .as_str()
                    .is_some_and(|base| !base.is_empty()),
                "the listing carries the baseline: {event}"
            );
            // The folder the run was in travels with the listing: a client that
            // has moved to another root since reads the file and the snapshot from it.
            let named = std::fs::canonicalize(event["project"].as_str().unwrap_or_default())
                .expect("the listing names the project");
            assert_eq!(
                named,
                std::fs::canonicalize(project.path()).unwrap(),
                "the listing names the run's own project: {event}"
            );
        }
        if event["type"] == "agent_end" {
            break;
        }
    }
    assert_eq!(
        types.as_slice()[types.len().saturating_sub(2)..],
        ["turn_changes", "agent_end"],
        "the listing came ahead of the turn's end event: {types:?}"
    );
    assert_eq!(
        server.join().unwrap().len(),
        2,
        "both model steps were served"
    );
}

/// `abort` stops the running turn without ending the rpc process, so the next
/// prompt is a new turn in the same session. The VS Code panel keeps one
/// process per thread and relies on exactly this for its Stop button.
#[test]
fn an_abort_stops_the_turn_and_the_session_keeps_running() {
    let project = TempDir::new("project_abort");
    let config_home = TempDir::new("config_abort");
    let (base_url, server) = serve(vec![bash_call_body("sleep 2"), answer_body("Again.")]);

    let mut rpc = start(config_home.path(), project.path(), &base_url, false);
    rpc.send(json!({"type": "prompt", "message": "run it"}));
    loop {
        let event = rpc.event();
        match event["type"].as_str() {
            Some("tool_call") => break,
            Some("agent_end") => panic!("the turn ended before the tool ran: {event}"),
            _ => {}
        }
    }
    rpc.send(json!({"type": "abort"}));
    let ended = drain_turn(&mut rpc, &mut Vec::new());
    assert_eq!(ended.last().expect("an end event")["type"], "agent_end");
    assert!(
        rpc.child.try_wait().expect("the process state").is_none(),
        "the process stays open for the next prompt"
    );

    rpc.send(json!({"type": "prompt", "message": "say it again"}));
    let next = drain_turn(&mut rpc, &mut Vec::new());
    assert!(
        next.iter().any(|event| event["type"] == "message_update"
            && event["assistantMessageEvent"]["delta"] == "Again."),
        "the second prompt was served by the same process: {next:?}"
    );
    assert_eq!(next.last().expect("an end event")["type"], "agent_end");
    assert_eq!(server.join().unwrap().len(), 2, "both prompts were served");
}

/// The extension sends the prompt as a request frame rather than through `-p`,
/// so the images a message carries have to travel with it: the run has to turn
/// the paths into media parts of that one message.
#[test]
fn an_rpc_prompt_carries_its_images() {
    let project = TempDir::new("project_image");
    let config_home = TempDir::new("config_image");
    let shot = project.path().join("shot.png");
    std::fs::write(&shot, PNG_1X1).expect("a writable attachment");
    let (base_url, server) = serve(vec![answer_body("Seen it.")]);

    // A vision-capable provider keeps the image native; the text-only
    // DeepSeek conversion is covered by the core's own tests.
    let mut rpc = start_as(
        config_home.path(),
        project.path(),
        &base_url,
        true,
        "openai",
        "gpt-4o-mini",
    );
    rpc.send(json!({
        "type": "prompt",
        "message": "what is in this screenshot?",
        "images": [shot.display().to_string()],
    }));
    let events = drain_turn(&mut rpc, &mut Vec::new());
    assert_eq!(events.last().expect("an end event")["type"], "agent_end");

    let bodies = server.join().unwrap();
    assert_eq!(bodies.len(), 1, "one model step was served");
    let body: Value = serde_json::from_str(&bodies[0]).expect("the request is JSON");
    // The user message is the one carrying the text; the image has to ride on
    // that same message, not on one of its own.
    let prompt = body["messages"]
        .as_array()
        .expect("the request carries messages")
        .iter()
        .find(|message| {
            message["content"].as_array().is_some_and(|parts| {
                parts
                    .iter()
                    .any(|part| part["text"] == "what is in this screenshot?")
            })
        })
        .expect("the prompt is a message");
    let parts = prompt["content"].as_array().expect("the prompt has parts");
    assert!(
        parts.iter().any(|part| part["type"] == "image_url"
            && part["image_url"]["url"]
                .as_str()
                .is_some_and(|url| url.starts_with("data:image/png;base64,")),),
        "the image rides on the prompt: {parts:?}"
    );
}
