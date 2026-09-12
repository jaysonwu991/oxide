use crate::config::Config;
use crate::llm::{LlmClient, Message};
use anyhow::Result;

pub const KEEP_RECENT: usize = 6;
const MAX_CHARS: usize = 48_000;

pub fn needs_compaction(messages: &[Message]) -> bool {
    messages.iter().map(estimated_chars).sum::<usize>() > MAX_CHARS
}

fn estimated_chars(message: &Message) -> usize {
    message.display().map(|text| text.len()).unwrap_or(0) + 200
}

pub async fn compact(config: &Config, messages: Vec<Message>) -> Result<Vec<Message>> {
    if messages.len() <= KEEP_RECENT {
        return Ok(messages);
    }
    let split = messages.len() - KEEP_RECENT;
    let (older, recent) = messages.split_at(split);
    let transcript = older
        .iter()
        .filter_map(Message::display)
        .collect::<Vec<_>>()
        .join("\n\n");
    let summary = summarize(config, &transcript).await?;
    let mut compacted = Vec::with_capacity(recent.len() + 1);
    compacted.push(Message::user(format!("[conversation summary]\n{summary}")));
    compacted.extend_from_slice(recent);
    Ok(compacted)
}

async fn summarize(config: &Config, transcript: &str) -> Result<String> {
    let client = LlmClient::new(config.clone());
    let messages = vec![
        Message::system(
            "You compress conversation history for another AI coding agent. Preserve goals, \
             decisions, file paths, code changes and unresolved tasks. Be concise but complete.",
        ),
        Message::user(format!("Summarize this conversation:\n\n{transcript}")),
    ];
    let mut summary = String::new();
    let _ = client
        .stream_chat(&messages, &[], |delta| summary.push_str(&delta))
        .await?;
    Ok(summary.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_compaction_only_for_long_history() {
        let short = vec![Message::user("hi")];
        assert!(!needs_compaction(&short));

        let long = vec![Message::user("x".repeat(MAX_CHARS + 1))];
        assert!(needs_compaction(&long));
    }

    #[tokio::test]
    async fn compact_is_noop_for_short_history() {
        let messages = vec![Message::user("hello")];
        let result = compact(&Config::default(), messages.clone()).await.unwrap();
        assert_eq!(result.len(), messages.len());
    }
}
