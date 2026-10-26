//! Opt-in live gateway check: cargo run -p oxide-core --example model_check -- medium
use anyhow::{bail, ensure, Result};
use futures::{stream, StreamExt};
use oxide_core::config::Config;
use oxide_core::llm::{FunctionSpec, LlmClient, Message, StreamHooks, ToolSpec};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let effort = args.next().unwrap_or_else(|| "medium".into());
    let base = Config::load(&std::env::current_dir()?, None, None, None, Some(effort))?;
    ensure!(
        base.is_portkey(),
        "this model matrix requires a configured Portkey provider"
    );
    let selected: Vec<String> = args.collect();
    let models = if selected.is_empty() {
        oxide_core::config::PORTKEY_FALLBACK_MODELS
            .iter()
            .map(|model| model.to_string())
            .collect()
    } else {
        selected
    };
    let mut checks = stream::iter(models.into_iter().map(|model| {
        let mut config = base.clone();
        config.model = model.clone();
        config.max_tokens = 4096;
        config.refresh_reasoning_supported();
        async move {
            let started = Instant::now();
            let result = tokio::time::timeout(Duration::from_secs(90), check(config)).await;
            let mut report = match result {
                Ok(Ok(report)) => report,
                Ok(Err(error)) => json!({"ok":false,"error":error.to_string()}),
                Err(_) => json!({"ok":false,"error":"90s timeout"}),
            };
            report["model"] = json!(model);
            report["seconds"] = json!(started.elapsed().as_secs_f64());
            report
        }
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
    for step in 1..=3 {
        let mut hooks = StreamHooks {
            text: &mut |_| {},
            thinking: &mut |_| {},
            retry: &mut |_| {
                retries += 1;
            },
        };
        let turn = client.stream_chat(&messages, &tools, &mut hooks).await?;
        let model = turn.response_model.as_ref().ok_or_else(|| {
            anyhow::anyhow!("provider omitted its model ID; identity cannot be verified")
        })?;
        if !response_models.contains(model) {
            response_models.push(model.clone());
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
                json!({"ok":true,"effort":effort,"requests":step + retries,"steps":step,"input":input,"output":output,"cached":cached,"reasoning":reasoning,"response_models":response_models}),
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
