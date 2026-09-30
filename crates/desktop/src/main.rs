//! Rust host for the Electron desktop app.
//!
//! Electron owns the window and native dialogs. This process owns the same
//! project/session/agent state the former in-process shell did and exchanges
//! newline-delimited JSON frames over stdin/stdout.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod approval;
mod ask;
mod commands;

use commands::{dispatch, DesktopState};
use oxide_desktop::bridge::EventSink;
use oxide_desktop::manager::DesktopManager;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::mpsc;

#[derive(Deserialize)]
struct Request {
    id: u64,
    command: String,
    #[serde(default)]
    args: Value,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (output, mut frames) = mpsc::unbounded_channel::<Value>();
    let events = EventSink::new(output.clone());
    let state = Arc::new(DesktopState::new(
        DesktopManager::load_lossy(),
        events.clone(),
    ));

    let writer = tokio::spawn(async move {
        let mut stdout = BufWriter::new(tokio::io::stdout());
        while let Some(frame) = frames.recv().await {
            let encoded = serde_json::to_vec(&frame)?;
            stdout.write_all(&encoded).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
        Ok::<(), anyhow::Error>(())
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                let _ = events.emit(
                    "host-error",
                    json!({ "message": format!("invalid desktop request: {error}") }),
                );
                continue;
            }
        };
        let state = Arc::clone(&state);
        let events = events.clone();
        tokio::spawn(async move {
            let result = dispatch(state, &request.command, request.args).await;
            events.response(request.id, result);
        });
    }

    writer.abort();
    Ok(())
}
