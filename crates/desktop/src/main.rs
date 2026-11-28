//! Oxide Desktop: the native Tauri front-end.
//!
//! The webview loads the page in `ui/`. The host injects the packet bridge that
//! page speaks and dispatches every engine request into the in-process
//! `oxide-desktop` library, so the window reads and writes the same projects,
//! sessions, approvals, preferences and configuration as the other front-ends.

mod engine;
mod menu;

use engine::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::oneshot;

const PAGE_EVENT: &str = "oxide:to-page";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments
        .iter()
        .any(|argument| argument == "--version" || argument == "-V")
    {
        println!("oxide-desktop {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if arguments.iter().any(|argument| argument == "--check") {
        std::process::exit(check());
    }
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!(
            "Oxide Desktop — the native Tauri front-end.\n\n\
             Usage: oxide-desktop [--version] [--check]\n\n\
             Runs the shared desktop engine in this process and hosts the desktop page."
        );
        return;
    }

    if let Err(error) = run() {
        eprintln!("[oxide] {error:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![oxide_message])
        .setup(|app| {
            let (engine, packets) = Engine::start()?;
            let delivery = Arc::new(Delivery::default());
            let host = HostState {
                engine,
                delivery: Arc::clone(&delivery),
                menu: Mutex::new(menu::NativeMenu::default()),
                awake: Mutex::new(None),
            };
            app.manage(host);

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                forward_packets(handle, delivery, packets).await;
            });

            let state = app.state::<HostState>();
            menu::install(app.handle(), &state.menu.lock().unwrap())?;
            Ok(())
        })
        .on_menu_event(|app, event| menu::handle(app, &event))
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                window.app_handle().exit(0);
            }
        })
        .run(tauri::generate_context!())
        .map_err(|error| anyhow::anyhow!("{error}"))
}

struct HostState {
    engine: Engine,
    delivery: Arc<Delivery>,
    menu: Mutex<menu::NativeMenu>,
    awake: Mutex<Option<keepawake::KeepAwake>>,
}

#[derive(Default)]
struct Delivery {
    inner: Mutex<DeliveryInner>,
}

#[derive(Default)]
struct DeliveryInner {
    ready: bool,
    pending: Vec<String>,
}

impl Delivery {
    fn send(&self, app: &AppHandle, packet: impl Into<String>) {
        let packet = packet.into();
        let waits_for_page = waits_for_page(&packet);
        let send_now = {
            let mut inner = self.inner.lock().unwrap();
            if inner.ready || !waits_for_page {
                true
            } else {
                inner.pending.push(packet.clone());
                false
            }
        };
        if send_now {
            let _ = app.emit(PAGE_EVENT, packet);
        }
    }

    fn flush(&self) -> Vec<String> {
        let mut inner = self.inner.lock().unwrap();
        inner.ready = true;
        std::mem::take(&mut inner.pending)
    }
}

fn waits_for_page(packet: &str) -> bool {
    serde_json::from_str::<Value>(packet)
        .map(|packet| packet["type"] == "message")
        .unwrap_or(false)
}

async fn forward_packets(
    app: AppHandle,
    delivery: Arc<Delivery>,
    mut packets: tokio::sync::mpsc::UnboundedReceiver<Value>,
) {
    while let Some(packet) = packets.recv().await {
        if packet["type"] == "message" && packet["id"] == "restart" {
            restart_app(&app);
        }
        delivery.send(&app, packet.to_string());
    }
}

/// Relaunches the app after an update without briefly running two copies.
///
/// Tauri's macOS restart uses `open -n`, which explicitly asks LaunchServices
/// for a second instance while the old process is still alive. That gives the
/// Dock two Oxide tiles during an update and breaks the expectation that an
/// installed release replaces the running one. Wait until this process has
/// exited, then open the bundle normally and bring it forward so LaunchServices
/// reuses Oxide's one application identity.
fn restart_app(app: &AppHandle) -> ! {
    #[cfg(target_os = "macos")]
    {
        if let Ok(executable) = std::env::current_exe() {
            if let Some(bundle) = bundle_of(&executable) {
                match schedule_macos_restart(&bundle) {
                    Ok(()) => {
                        app.cleanup_before_exit();
                        std::process::exit(0);
                    }
                    Err(error) => {
                        eprintln!("[oxide] scheduling restart through LaunchServices: {error}");
                    }
                }
            }
        }
    }
    app.restart()
}

#[cfg(target_os = "macos")]
fn bundle_of(executable: &Path) -> Option<PathBuf> {
    executable
        .ancestors()
        .find(|ancestor| {
            ancestor.extension() == Some(std::ffi::OsStr::new("app"))
                && ancestor.join("Contents").join("Info.plist").is_file()
        })
        .map(Path::to_path_buf)
}

#[cfg(target_os = "macos")]
fn schedule_macos_restart(bundle: &Path) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;

    let mut command = macos_restart_command(bundle, std::process::id());
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn macos_restart_command(bundle: &Path, pid: u32) -> Command {
    const WAIT_THEN_OPEN: &str = r#"
while kill -0 "$1" 2>/dev/null; do
  state="$(/bin/ps -p "$1" -o stat= 2>/dev/null)"
  case "$state" in Z*) break ;; esac
  /bin/sleep 0.1
done
/bin/sleep 0.2
attempt=0
until /usr/bin/open "$2"; do
  attempt=$((attempt + 1))
  [ "$attempt" -ge 20 ] && exit 1
  /bin/sleep 0.25
done
"#;
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(WAIT_THEN_OPEN)
        .arg("oxide-restart")
        .arg(pid.to_string())
        .arg(bundle);
    command
}

fn native_ready(app: &AppHandle, state: &HostState) -> Result<(), String> {
    for packet in state.delivery.flush() {
        app.emit(PAGE_EVENT, packet)
            .map_err(|error| error.to_string())?;
    }
    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn oxide_message(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, HostState>,
    message: String,
) -> Result<(), String> {
    let Some(request) = Request::parse(&message) else {
        return Ok(());
    };

    match request.command.as_str() {
        "native_ready" => {
            native_ready(&app, &state)?;
            respond_ok(&state.delivery, &app, request.id, Value::Null);
        }
        "set_native_language" => {
            let strings = request
                .args
                .get("strings")
                .and_then(Value::as_object)
                .map(|strings| {
                    strings
                        .iter()
                        .filter_map(|(key, value)| {
                            value.as_str().map(|value| (key.clone(), value.to_string()))
                        })
                        .collect::<HashMap<_, _>>()
                })
                .unwrap_or_default();
            *state.menu.lock().unwrap() = menu::NativeMenu::with_strings(strings);
            menu::install(&app, &state.menu.lock().unwrap())?;
            respond_ok(&state.delivery, &app, request.id, Value::Null);
        }
        "set_native_shortcuts" => {
            let bindings = request
                .args
                .get("bindings")
                .and_then(Value::as_object)
                .map(|bindings| {
                    bindings
                        .iter()
                        .filter_map(|(key, value)| {
                            value.as_str().map(|value| (key.clone(), value.to_string()))
                        })
                        .collect::<HashMap<_, _>>()
                })
                .unwrap_or_default();
            state.menu.lock().unwrap().bindings = bindings;
            menu::install(&app, &state.menu.lock().unwrap())?;
            respond_ok(&state.delivery, &app, request.id, Value::Null);
        }
        "pick_folder" => {
            let title = state
                .menu
                .lock()
                .unwrap()
                .text("ui.chooseAProjectFolder", "Choose a project folder");
            let (sender, receiver) = oneshot::channel();
            app.dialog()
                .file()
                .set_title(title)
                .set_can_create_directories(true)
                .pick_folder(move |folder| {
                    let _ = sender.send(folder);
                });
            let folder = receiver.await.ok().flatten();
            let path = folder
                .and_then(|folder| folder.into_path().ok())
                .and_then(|path| path.to_str().map(str::to_string));
            respond_ok(
                &state.delivery,
                &app,
                request.id,
                path.map(Value::String).unwrap_or(Value::Null),
            );
        }
        "open_url" => {
            let url = request
                .args
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if !is_openable_url(url) {
                respond_err(
                    &state.delivery,
                    &app,
                    request.id,
                    "Only http(s) links can be opened",
                );
            } else {
                match app.opener().open_url(url, None::<&str>) {
                    Ok(()) => respond_ok(&state.delivery, &app, request.id, Value::Null),
                    Err(error) => respond_err(&state.delivery, &app, request.id, error.to_string()),
                }
            }
        }
        "set_zoom" => {
            let scale = request
                .args
                .get("scale")
                .and_then(Value::as_f64)
                .filter(|scale| scale.is_finite())
                .map(|scale| scale.clamp(0.7, 1.6));
            if let Some(scale) = scale {
                window.set_zoom(scale).map_err(|error| error.to_string())?;
            }
            respond_ok(&state.delivery, &app, request.id, Value::Null);
        }
        "notify" => {
            let title = request
                .args
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let body = request
                .args
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !title.is_empty() {
                let _ = app.notification().builder().title(title).body(body).show();
            }
            respond_ok(&state.delivery, &app, request.id, Value::Null);
        }
        "set_badge" => {
            let count = request
                .args
                .get("count")
                .and_then(Value::as_f64)
                .filter(|count| count.is_finite())
                .map(|count| count.max(0.0).round() as i64)
                .unwrap_or(0);
            let _ = window.set_badge_count((count > 0).then_some(count));
            respond_ok(&state.delivery, &app, request.id, Value::Null);
        }
        "set_awake" => {
            let wanted = request
                .args
                .get("on")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut awake = state.awake.lock().unwrap();
            if wanted && awake.is_none() {
                let mut builder = keepawake::Builder::default();
                builder
                    .idle(true)
                    .display(false)
                    .reason("Oxide is running a turn")
                    .app_name("Oxide")
                    .app_reverse_domain("dev.oxide.desktop");
                match builder.create() {
                    Ok(created) => *awake = Some(created),
                    Err(error) => {
                        respond_err(&state.delivery, &app, request.id, error.to_string());
                        return Ok(());
                    }
                }
            } else if !wanted {
                *awake = None;
            }
            respond_ok(&state.delivery, &app, request.id, Value::Null);
        }
        _ => {
            let (command, args) = route_update_command(&request);
            let answer = state
                .engine
                .call(&command, args)
                .await
                .unwrap_or_else(|_| Err("the engine stopped answering".to_string()));
            match answer {
                Ok(payload) => respond_ok(&state.delivery, &app, request.id, payload),
                Err(error) => respond_err(&state.delivery, &app, request.id, error),
            }
        }
    }
    Ok(())
}

fn route_update_command(request: &Request) -> (String, Value) {
    let current = env!("CARGO_PKG_VERSION");
    let mut args = request.args.clone();
    match request.command.as_str() {
        "check_updates" | "install_update" | "download_update" => {
            args["current"] = json!(current);
            (request.command.clone(), args)
        }
        command => (command.to_string(), args),
    }
}

fn respond_ok(delivery: &Delivery, app: &AppHandle, id: u64, payload: Value) {
    delivery.send(
        app,
        json!({
            "type": "response",
            "id": id,
            "success": true,
            "payload": payload,
        })
        .to_string(),
    );
}

fn respond_err(delivery: &Delivery, app: &AppHandle, id: u64, error: impl Into<String>) {
    delivery.send(
        app,
        json!({
            "type": "response",
            "id": id,
            "success": false,
            "error": error.into(),
        })
        .to_string(),
    );
}

fn is_openable_url(url: &str) -> bool {
    let url = url.trim_start().to_ascii_lowercase();
    url.starts_with("http://") || url.starts_with("https://")
}

struct Request {
    id: u64,
    command: String,
    args: Value,
}

impl Request {
    fn parse(raw: &str) -> Option<Self> {
        let packet: Value = serde_json::from_str(raw).ok()?;
        if packet["type"] != "request" {
            return None;
        }
        Some(Self {
            id: packet["id"].as_u64()?,
            command: packet["params"]["command"].as_str()?.to_string(),
            args: packet["params"]["args"].clone(),
        })
    }
}

/// Answers the engine's command contract without opening a window, so a build
/// fails when the packaged host cannot reach the shared stores.
fn check() -> i32 {
    let (engine, _packets) = match Engine::start() {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("[oxide] could not start the desktop engine: {error:#}");
            return 1;
        }
    };
    let commands: Vec<(&str, Value)> = vec![
        ("list_projects", json!({})),
        ("all_sessions", json!({})),
        ("read_prefs", json!({})),
        ("language", json!({ "project": Value::Null })),
        ("list_themes", json!({ "project": "." })),
        ("list_approvals", json!({ "project": "." })),
        ("list_providers", json!({})),
        ("plugin_catalog", json!({})),
        ("launch_update", json!({})),
    ];
    let mut failed = false;
    let mut theme = "dark".to_string();
    for (command, args) in commands {
        match engine.call_blocking(command, args) {
            Ok(value) => {
                if command == "list_themes" {
                    if let Some(current) = value["current"].as_str() {
                        theme = current.to_string();
                    }
                }
                println!("ok   {command}");
            }
            Err(error) => {
                failed = true;
                println!("fail {command}: {error}");
            }
        }
    }
    match engine.call_blocking("theme_colors", json!({ "project": ".", "name": theme })) {
        Ok(_) => println!("ok   theme_colors"),
        Err(error) => {
            failed = true;
            println!("fail theme_colors: {error}");
        }
    }
    if failed {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_the_page_request_shape() {
        let request = Request::parse(
            &json!({
                "type": "request",
                "id": 7,
                "method": "oxide_invoke",
                "params": { "command": "list_projects", "args": {} },
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(request.id, 7);
        assert_eq!(request.command, "list_projects");
        assert_eq!(request.args, json!({}));
        assert!(Request::parse(r#"{"type":"message","id":"agent-end"}"#).is_none());
        assert!(Request::parse("not json").is_none());
    }

    #[test]
    fn accepts_only_web_links_for_the_platform_handler() {
        assert!(is_openable_url(" https://example.com/a"));
        assert!(is_openable_url("HTTP://example.com"));
        assert!(!is_openable_url("file:///etc/passwd"));
        assert!(!is_openable_url("javascript:alert(1)"));
        assert!(!is_openable_url(""));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_restart_waits_before_opening_one_application_instance() {
        let bundle = Path::new("/Applications/Oxide.app");
        let command = macos_restart_command(bundle, 42);
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(args[0], "-c");
        assert!(args[1].contains("kill -0 \"$1\""));
        assert!(args[1].contains("case \"$state\" in Z*) break ;; esac"));
        assert!(args[1].contains("until /usr/bin/open \"$2\"; do"));
        assert!(args[1].contains("attempt=$((attempt + 1))"));
        assert!(!args.iter().any(|arg| arg == "-n"));
        assert!(!args.iter().any(|arg| arg == "-g"));
        assert_eq!(args[2], "oxide-restart");
        assert_eq!(args[3], "42");
        assert_eq!(args[4], bundle.to_string_lossy());
    }

    #[test]
    fn gives_desktop_update_requests_this_builds_version() {
        let request = Request {
            id: 1,
            command: "check_updates".to_string(),
            args: json!({}),
        };
        assert_eq!(
            route_update_command(&request),
            (
                "check_updates".to_string(),
                json!({ "current": env!("CARGO_PKG_VERSION") }),
            )
        );

        let request = Request {
            id: 2,
            command: "install_update".to_string(),
            args: json!({ "token": "abc" }),
        };
        assert_eq!(
            route_update_command(&request),
            (
                "install_update".to_string(),
                json!({ "token": "abc", "current": env!("CARGO_PKG_VERSION") }),
            )
        );
    }

    #[test]
    fn buffers_events_until_the_page_has_installed_its_listener() {
        let delivery = Delivery::default();
        {
            let mut inner = delivery.inner.lock().unwrap();
            assert!(!inner.ready);
            inner.pending.push("early".to_string());
        }
        let pending = {
            let mut inner = delivery.inner.lock().unwrap();
            inner.ready = true;
            std::mem::take(&mut inner.pending)
        };
        assert_eq!(pending, vec!["early"]);
        assert!(delivery.inner.lock().unwrap().ready);
    }

    #[test]
    fn command_responses_do_not_wait_for_the_page_to_report_ready() {
        let response = json!({
            "type": "response",
            "id": 1,
            "success": true,
            "payload": {},
        });
        assert!(!waits_for_page(&response.to_string()));
        assert!(waits_for_page(
            &json!({ "type": "message", "id": "agent-end", "payload": {} }).to_string()
        ));
    }
}
