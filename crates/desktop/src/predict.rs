//! The next message a reader is likely to send, offered as a draft.
//!
//! This is one small model call over the tail of a conversation, not a turn: no
//! tools, no session, a short cap and a prompt that asks for the message alone.
//! What comes back is a suggestion — nothing is sent until the reader takes it
//! and presses send — so a wrong one costs a glance and a right one saves typing.
//!
//! The prompt is built from the conversation itself rather than from a transcript
//! the window hands over, so the call is the same one the terminal could make and
//! the window holds no copy of the thread to keep in step.

use anyhow::{Context, Result};
use oxide_core::config::Config;
use oxide_core::llm::{LlmClient, Message};

/// How much of the conversation the suggestion is read from, and how long the
/// suggestion itself may be: enough to be useful, short enough that the call is
/// cheap and cannot become a second reply.
const TAIL: usize = 12;
const MAX_CHARS: usize = 280;

const SYSTEM: &str = "You suggest the next message a developer would type in a coding \
conversation. Reply with that message only — one sentence or two, in the language the \
conversation is in, with no quotes, no greeting, no explanation and no formatting. It must \
be something the reader would say to the agent: a follow-up, a correction, a request for the \
next step. If nothing sensible follows, reply with a single full stop.";

/// The messages one suggestion is asked with: the conversation's tail, oldest
/// first, and the instruction that asks for the reader's own next line.
pub fn prompt(messages: &[Message]) -> Vec<Message> {
    let start = messages.len().saturating_sub(TAIL);
    let mut lines = Vec::new();
    for message in &messages[start..] {
        let Some(text) = message.display() else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let who = match message.role.as_str() {
            "assistant" => "Oxide",
            "user" => "The reader",
            other => other,
        };
        lines.push(format!("{who}: {text}"));
    }
    let conversation = if lines.is_empty() {
        "(the conversation has not started yet)".to_string()
    } else {
        lines.join("\n\n")
    };
    vec![
        Message::system(SYSTEM),
        Message::user(format!(
            "The conversation so far:\n\n{conversation}\n\nWrite the reader's next message."
        )),
    ]
}

/// What the model said, made into something a composer can hold: the first line,
/// unquoted, without a label it may have put in front of it, and short enough to
/// be a draft rather than a paragraph.
pub fn clean(raw: &str) -> String {
    let mut text = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string();
    for prefix in [
        "Next message:",
        "next message:",
        "Suggestion:",
        "suggestion:",
        "Message:",
        "message:",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim().to_string();
        }
    }
    text = text
        .trim_matches(|character: char| {
            matches!(character, '"' | '\'' | '`' | '“' | '”' | '‘' | '’' | '*')
        })
        .trim()
        .to_string();
    if text.chars().count() > MAX_CHARS {
        text = text.chars().take(MAX_CHARS).collect::<String>();
        text = text.trim_end().to_string();
    }
    // A full stop is the model's way of saying nothing follows, and a suggestion
    // that says nothing is no suggestion at all.
    if text == "." || text == "…" {
        return String::new();
    }
    text
}

/// Asks the model for the next message. A configuration with no credential, a
/// provider that is down or an answer that says nothing all end in an empty
/// suggestion: this is an offer, not a requirement.
pub async fn suggest(config: &Config, messages: &[Message]) -> Result<String> {
    let mut config = config.clone();
    // The suggestion is a line, not a reply: the cap is this call's decision.
    config.max_tokens = 120;
    config.max_tokens_explicit = true;
    let messages = prompt(messages);
    let client = LlmClient::new(config);
    // Two of the hooks write to the answer, so it is held where both can reach it
    // rather than borrowed by one of them; the mutex is what lets the stream sit
    // on whatever thread the provider runs it.
    let answer = std::sync::Mutex::new(String::new());
    let mut ignore = |_: String| {};
    let mut reset = |_: oxide_core::llm::Retry| {
        answer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear()
    };
    let mut keep = |delta: String| {
        answer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_str(&delta)
    };
    let mut hooks = oxide_core::llm::StreamHooks {
        text: &mut keep,
        thinking: &mut ignore,
        retry: &mut reset,
    };
    client
        .stream_chat(&messages, &[], &mut hooks)
        .await
        .context("asking for a suggestion")?;
    let answer = answer
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok(clean(&answer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::llm::MessageContent;

    fn said(role: &str, text: &str) -> Message {
        Message {
            role: role.into(),
            content: Some(MessageContent::Text(text.into())),
            tool_calls: None,
            tool_call_id: None,
            thinking: None,
        }
    }

    #[test]
    fn the_prompt_carries_the_conversation_and_asks_for_the_readers_line() {
        let messages = prompt(&[
            said("user", "the tests fail"),
            said("assistant", "which one?"),
        ]);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        let body = messages[1].display().unwrap_or_default();
        assert!(body.contains("The reader: the tests fail"), "{body}");
        assert!(body.contains("Oxide: which one?"), "{body}");
        assert!(body.contains("Write the reader's next message."), "{body}");
    }

    #[test]
    fn only_the_tail_of_a_long_conversation_is_asked_about() {
        let mut messages = Vec::new();
        for index in 0..40 {
            messages.push(said("user", &format!("message {index}")));
        }
        let body = prompt(&messages)[1].display().unwrap_or_default();
        assert!(!body.contains("message 0\n"), "{body}");
        assert!(!body.contains("message 27"), "{body}");
        assert!(body.contains("message 39"), "{body}");
    }

    #[test]
    fn a_conversation_with_nothing_in_it_still_asks_a_question() {
        let body = prompt(&[])[1].display().unwrap_or_default();
        assert!(body.contains("has not started yet"), "{body}");
    }

    #[test]
    fn the_suggestion_is_the_first_line_and_nothing_else() {
        assert_eq!(
            clean("Also update the changelog.\nThen push."),
            "Also update the changelog."
        );
        assert_eq!(
            clean("\n\n  Run the tests again  \n"),
            "Run the tests again"
        );
    }

    #[test]
    fn quotes_and_labels_the_model_added_are_taken_off() {
        assert_eq!(clean("\"Run the tests again\""), "Run the tests again");
        assert_eq!(clean("Next message: run it"), "run it");
        assert_eq!(clean("Suggestion: “check the logs”"), "check the logs");
        assert_eq!(
            clean("**Try the other provider**"),
            "Try the other provider"
        );
    }

    #[test]
    fn a_suggestion_that_says_nothing_is_no_suggestion() {
        assert_eq!(clean(""), "");
        assert_eq!(clean("   "), "");
        assert_eq!(clean("."), "");
        assert_eq!(clean("…"), "");
    }

    #[test]
    fn a_long_suggestion_is_cut_to_a_draft() {
        let long = "word ".repeat(200);
        let cleaned = clean(&long);
        assert!(
            cleaned.chars().count() <= MAX_CHARS,
            "{}",
            cleaned.chars().count()
        );
        assert!(!cleaned.ends_with(' '), "{cleaned:?}");
    }
}
