//! Opt-in live gateway check: cargo run -p oxide-core --example model_check -- auto
use anyhow::{bail, ensure, Result};
use futures::{stream, StreamExt};
use oxide_core::config::{Config, Reasoning};
use oxide_core::llm::{FunctionSpec, LlmClient, Message, StreamHooks, ToolSpec};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let effort = args.next().unwrap_or_else(|| "auto".into());
    let all_efforts = effort == "all";
    let base = Config::load(
        &std::env::current_dir()?,
        None,
        None,
        None,
        Some(if all_efforts { "auto".into() } else { effort }),
    )?;
    ensure!(
        base.is_portkey(),
        "this model matrix requires a configured Portkey provider"
    );
    let selected: Vec<String> = args.collect();
    let models = if selected.is_empty() {
        LlmClient::new(base.clone()).refresh_models().await?
    } else {
        selected
    };
    let mut cases = Vec::new();
    for model in models {
        let mut config = base.clone();
        config.model = model.clone();
        config.max_tokens = 4096;
        config.refresh_reasoning_supported();
        let levels = if all_efforts {
            std::iter::once(Reasoning::Auto)
                .chain(config.reasoning_levels())
                .collect()
        } else {
            vec![config.reasoning]
        };
        for reasoning in levels {
            let mut config = config.clone();
            config.reasoning = reasoning;
            cases.push((model.clone(), config));
        }
    }
    let mut checks = stream::iter(cases.into_iter().map(|(model, config)| async move {
        let started = Instant::now();
        let effort = config.effective_reasoning().label();
        let result = tokio::time::timeout(Duration::from_secs(90), check(config)).await;
        let mut report = match result {
            Ok(Ok(report)) => report,
            Ok(Err(error)) => json!({"ok":false,"error":error.to_string()}),
            Err(_) => json!({"ok":false,"error":"90s timeout"}),
        };
        report["model"] = json!(model);
        report["effort"] = json!(effort);
        report["seconds"] = json!(started.elapsed().as_secs_f64());
        report
    }))
    .buffer_unordered(3);
    let mut failed = false;
    while let Some(report) = checks.next().await {
        failed |= report["ok"] != true;
        println!("{report}");
    }
    ensure!(
        !failed,
        "some models failed; inspect the JSON results above"
    );
    Ok(())
}

async fn check(config: Config) -> Result<Value> {
    let effort = config.effective_reasoning().label();
    let client = LlmClient::new(config);
    let tools = vec![ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "lookup".into(),
            description: "Read the requested value. The result cannot be inferred; call this tool."
                .into(),
            parameters: json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false}),
        },
    }];
    let mut messages = vec![
        Message::system("Use tools for lookups. Reply concisely with only the returned value."),
        Message::user("Look up the key benchmark using lookup, then reply with its value only."),
    ];
    let (mut input, mut output, mut cached, mut reasoning) = (0, 0, 0, 0);
    let mut calls = 0;
    let mut retries = 0;
    let mut response_models = Vec::new();
    let mut identity_verified = true;
    for step in 1..=3 {
        let mut hooks = StreamHooks {
            text: &mut |_| {},
            thinking: &mut |_| {},
            retry: &mut |_| {
                retries += 1;
            },
        };
        let turn = client.stream_chat(&messages, &tools, &mut hooks).await?;
        if let Some(model) = turn.response_model.as_ref() {
            if !response_models.contains(model) {
                response_models.push(model.clone());
            }
        } else {
            identity_verified = false;
        }
        input += turn.usage.input;
        output += turn.usage.output;
        cached += turn.usage.cache_read;
        reasoning += turn.usage.reasoning;
        if turn.tool_calls.is_empty() {
            ensure!(calls == 1, "expected one lookup, got {calls}");
            ensure!(
                turn.content.trim() == "oxide-check-42",
                "wrong or missing final result"
            );
            return Ok(
                json!({"ok":true,"identity_verified":identity_verified,"effort":effort,"requests":step + retries,"steps":step,"input":input,"output":output,"cached":cached,"reasoning":reasoning,"response_models":response_models}),
            );
        }
        let assistant =
            Message::assistant(turn.content, turn.tool_calls.clone()).with_thinking(turn.thinking);
        messages.push(assistant);
        for call in turn.tool_calls {
            calls += 1;
            ensure!(
                calls == 1 && call.function.name == "lookup",
                "unexpected or repeated tool call"
            );
            let args: Value = serde_json::from_str(&call.function.arguments)?;
            ensure!(args == json!({"key":"benchmark"}), "wrong lookup arguments");
            messages.push(Message::tool(call.id, "oxide-check-42"));
        }
    }
    bail!("model did not finish within three requests")
}
