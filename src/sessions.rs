//! Non-interactive saved-session management: list, delete, compact, and merge.

use crate::compact;
use crate::config::Config;
use crate::session::{SessionLog, SessionSummary};
use anyhow::{Context, Result};
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn list(cwd: &Path, all: bool, older_than_days: Option<u64>) -> Result<()> {
    let mut sessions = if all {
        SessionLog::list_all()?
    } else {
        SessionLog::list(cwd)?
    };
    if let Some(days) = older_than_days {
        let cutoff = now_secs().saturating_sub(days.saturating_mul(86_400));
        sessions.retain(|summary| summary.modified_at < cutoff);
    }
    if sessions.is_empty() {
        println!("no sessions{}", if all { "" } else { " for this project" });
        return Ok(());
    }

    let now = now_secs();
    for summary in &sessions {
        let label = summary
            .name
            .clone()
            .unwrap_or_else(|| summary.preview.clone());
        if all {
            println!(
                "{}  {:<10}  {:>4} msg  {}  {}",
                summary.id,
                relative_age(now, summary.modified_at),
                summary.message_count,
                summary.cwd,
                label
            );
        } else {
            println!(
                "{}  {:<10}  {:>4} msg  {}",
                summary.id,
                relative_age(now, summary.modified_at),
                summary.message_count,
                label
            );
        }
    }
    Ok(())
}

pub fn delete(
    cwd: &Path,
    id: Option<String>,
    all: bool,
    older_than_days: Option<u64>,
    force: bool,
) -> Result<()> {
    let targets = delete_targets(cwd, id, all, older_than_days)?;
    if targets.is_empty() {
        println!("no sessions to delete");
        return Ok(());
    }

    if !force {
        println!("delete {} session(s)?", targets.len());
        let now = now_secs();
        for summary in &targets {
            let label = summary
                .name
                .clone()
                .unwrap_or_else(|| summary.preview.clone());
            println!(
                "  {}  {}  {}",
                summary.id,
                relative_age(now, summary.modified_at),
                label
            );
        }
        print!("[y/N] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("aborted");
            return Ok(());
        }
    }

    for summary in &targets {
        SessionLog::delete(cwd, &summary.id)?;
        println!("deleted {}", summary.id);
    }
    Ok(())
}

pub async fn compact_sessions(
    cwd: &Path,
    config: &Config,
    id: Option<String>,
    all: bool,
) -> Result<()> {
    let ids = if all {
        SessionLog::list(cwd)?
            .into_iter()
            .map(|summary| summary.id)
            .collect::<Vec<_>>()
    } else if let Some(id) = id {
        vec![id]
    } else {
        anyhow::bail!("specify a session id or --all");
    };
    if ids.is_empty() {
        println!("no sessions for this project");
        return Ok(());
    }

    let mut compacted = 0;
    for id in &ids {
        match compact_one(cwd, config, id).await {
            Ok(true) => {
                println!("compacted {id}");
                compacted += 1;
            }
            Ok(false) => println!("skipped {id} (already compact)"),
            Err(err) => println!("error compacting {id}: {err:#}"),
        }
    }
    if compacted == 0 {
        println!("no sessions needed compaction");
    }
    Ok(())
}

pub async fn merge(
    cwd: &Path,
    config: Option<&Config>,
    a: &str,
    b: &str,
    summarize: bool,
) -> Result<()> {
    let log_a = SessionLog::open_ref(cwd, a)?;
    let log_b = SessionLog::open_ref(cwd, b)?;
    let a_messages = log_a.messages()?;
    let b_messages = log_b.messages()?;

    let mut merged = if summarize {
        let config = config.context("summarization requires a configured provider")?;
        compact::compact(config, b_messages).await?
    } else {
        b_messages
    };
    merged.extend(a_messages);

    let log = SessionLog::fork(cwd, &merged)?;
    println!("merged {a} + {b} into {}", log.id());
    Ok(())
}

async fn compact_one(cwd: &Path, config: &Config, id: &str) -> Result<bool> {
    let log = SessionLog::open_id(cwd, id)?;
    let messages = log.messages()?;
    let before = messages.len();
    let compacted = compact::compact(config, messages).await?;
    if compacted.len() == before {
        return Ok(false);
    }
    log.rewrite(&compacted)?;
    Ok(true)
}

fn delete_targets(
    cwd: &Path,
    id: Option<String>,
    all: bool,
    older_than_days: Option<u64>,
) -> Result<Vec<SessionSummary>> {
    if let Some(id) = id {
        let log = SessionLog::open_id(cwd, &id)?;
        return Ok(vec![log.summary()?]);
    }
    if !all && older_than_days.is_none() {
        anyhow::bail!("specify a session id, --all, or --older-than <days>");
    }
    let mut sessions = SessionLog::list(cwd)?;
    if let Some(days) = older_than_days {
        let cutoff = now_secs().saturating_sub(days.saturating_mul(86_400));
        sessions.retain(|summary| summary.modified_at < cutoff);
    }
    Ok(sessions)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn relative_age(now: u64, then: u64) -> String {
    let secs = now.saturating_sub(then);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}
