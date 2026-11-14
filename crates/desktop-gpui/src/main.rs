//! Oxide for Desktop, Beta: the GPUI front-end.
//!
//! One native process draws the whole window, and the desktop engine
//! (`oxide-desktop`'s library: projects, sessions, turns, approvals, updates)
//! runs inside it. The engine, the configuration, the session store and the
//! approval store are the ones the Electron window and the terminal use, so
//! this is the same app with a different renderer — a beta front-end beside the
//! stable one.

mod app;
mod editor;
mod engine;
mod markdown;
mod model;
mod overlays;
mod theme;
mod views;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments
        .iter()
        .any(|argument| argument == "--version" || argument == "-V")
    {
        println!("oxide-desktop-gpui {}", env!("CARGO_PKG_VERSION"));
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
            "Oxide for Desktop (Beta) — the GPUI front-end.\n\n\
             Usage: oxide-desktop-gpui [--version] [--check]\n\n\
             Runs the same desktop engine as the Electron app, in this process, and\n\
             draws its window with GPUI."
        );
        return;
    }

    let (engine, packets) = match engine::Engine::start() {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("[oxide] could not start the desktop engine: {error:#}");
            std::process::exit(1);
        }
    };

    gpui::Application::new().run(move |cx| {
        app::Root::bind_keys(cx);
        app::Root::install_menus(cx);
        app::open(engine, packets, cx);
        cx.activate(true);
    });
}

/// Answers a handful of the engine's commands without opening a window, so a
/// build can prove the front-end is wired to the same engine the Electron app
/// runs. Exit status 0 means every command answered.
fn check() -> i32 {
    use serde_json::json;

    let (engine, _packets) = match engine::Engine::start() {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("[oxide] could not start the desktop engine: {error:#}");
            return 1;
        }
    };

    let commands: Vec<(&str, serde_json::Value)> = vec![
        ("list_projects", json!({})),
        ("all_sessions", json!({})),
        ("read_prefs", json!({})),
        ("language", json!({ "project": serde_json::Value::Null })),
        ("list_themes", json!({ "project": "." })),
        ("list_approvals", json!({ "project": "." })),
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
    use crate::markdown;
    use crate::model::{describe_call, Chat, Entry};
    use serde_json::json;

    #[test]
    fn a_streamed_answer_becomes_one_card() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.apply(&json!({
            "type": "message_update",
            "assistantMessageEvent": { "type": "text_delta", "delta": "Hello " },
        }));
        chat.apply(&json!({
            "type": "message_update",
            "assistantMessageEvent": { "type": "text_delta", "delta": "there" },
        }));
        assert_eq!(chat.entries.len(), 1);
        match &chat.entries[0] {
            Entry::Assistant { text, blocks, .. } => {
                assert_eq!(text, "Hello there");
                assert!(!blocks.is_empty());
            }
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    #[test]
    fn a_tool_card_keeps_its_call_and_result_together() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.apply(&json!({
            "type": "tool_call",
            "toolName": "bash",
            "arguments": { "command": "cargo test" },
        }));
        chat.apply(&json!({
            "type": "tool_execution_end",
            "toolName": "bash",
            "result": "ok",
            "isError": false,
        }));
        match &chat.entries[0] {
            Entry::Tool(card) => {
                assert_eq!(card.name, "bash");
                assert_eq!(card.detail, "cargo test");
                assert_eq!(card.output, "ok");
            }
            other => panic!("expected a tool card, got {other:?}"),
        }
    }

    #[test]
    fn a_call_is_summarised_by_the_argument_it_acts_on() {
        assert_eq!(
            describe_call("read", &json!({ "path": "src/main.rs" })),
            "src/main.rs"
        );
        assert_eq!(
            describe_call("bash", &json!({ "command": "ls\npwd" })),
            "ls"
        );
        assert_eq!(describe_call("find", &json!({})), "");
    }

    #[test]
    fn markdown_survives_a_half_streamed_fence() {
        let blocks = markdown::parse("```rust\nfn main(");
        assert_eq!(blocks.len(), 1);
    }
}
