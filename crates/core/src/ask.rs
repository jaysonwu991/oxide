//! Interactive questions for every front-end.
//!
//! When the model needs something only the user can decide — typically while
//! following a skill, whose instructions say to confirm a choice — it calls the
//! `ask` tool. The agent awaits this broker, which emits
//! [`AgentEvent::QuestionRequest`] carrying a request id and the questions, and
//! the front-end answers with [`AskBroker::answer`]. The request travels the
//! run's own event stream, so it arrives after the `ToolCall` it belongs to; an
//! unanswered request times out and the tool reports that nobody answered, so a
//! turn cannot hang forever.
//!
//! The request id is broker-wide, so a subagent's question is answered by the
//! broker the front-end already holds, exactly as an approval is.

use anyhow::Result;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::agent::AgentEvent;

/// How long a question waits for an answer before the tool reports that nobody
/// answered. A front-end holds the turn until it arrives, so this only ends a
/// request whose view is gone (a closed window, a client that does not answer
/// questions).
pub const ASK_TIMEOUT: Duration = Duration::from_secs(300);

/// How many questions one call may ask, and how many options one question may
/// offer. A dialog a person reads, not a form the model fills in.
pub const MAX_QUESTIONS: usize = 4;
pub const MAX_OPTIONS: usize = 8;

/// One question: free text, or a choice among [`Choice`]s — several of them when
/// `multiSelect` is set, which is also how the model spells it in the tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub question: String,
    /// A short label for the question, used as the dialog's heading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(
        default,
        deserialize_with = "choices",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub options: Vec<Choice>,
    #[serde(
        default,
        rename = "multiSelect",
        alias = "multi_select",
        deserialize_with = "boolean"
    )]
    pub multi_select: bool,
}

/// One offered answer. The label is what the user picks and what the model reads
/// back, so it stays short; the description says what choosing it means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// An answer to one question: the labels that were picked, or the text that was
/// typed. `question` echoes the question it answers, so a front-end sends what
/// the user answered without the answers depending on their order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub question: String,
    #[serde(default, deserialize_with = "strings")]
    pub values: Vec<String>,
}

/// What a front-end answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// The answers the user gave, in the order the questions were asked.
    Answers(Vec<Answer>),
    /// The user closed the question without answering it.
    Dismissed,
}

/// Asks the user the questions. `None` means no answer came back: nothing was
/// listening for a question in this run, or the request timed out.
pub type Asker =
    Arc<dyn Fn(Vec<Question>) -> Pin<Box<dyn Future<Output = Option<Reply>> + Send>> + Send + Sync>;

struct Pending {
    sender: oneshot::Sender<Reply>,
}

/// Matches a question with the answer it is waiting for, the way
/// [`ApprovalBroker`](crate::approval::ApprovalBroker) matches an approval. One
/// broker serves a whole run, so a question asked by a subagent is answered the
/// same way.
pub struct AskBroker {
    pending: Mutex<HashMap<u64, Pending>>,
    next: AtomicU64,
    timeout: Duration,
}

impl Default for AskBroker {
    fn default() -> Self {
        Self::from_timeout(ASK_TIMEOUT)
    }
}

impl AskBroker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A broker with a timeout of its own, so a test does not wait out the real
    /// one.
    pub fn from_timeout(timeout: Duration) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            timeout,
        }
    }

    /// The [`Asker`] an agent runtime is given. `events` is the run's own
    /// stream, so the request reaches whichever front-end is drawing it.
    pub fn asker(self: &Arc<Self>, events: UnboundedSender<AgentEvent>) -> Asker {
        let broker = Arc::clone(self);
        Arc::new(move |questions| {
            let broker = Arc::clone(&broker);
            let events = events.clone();
            Box::pin(async move { broker.request(questions, events).await })
        })
    }

    async fn request(
        &self,
        questions: Vec<Question>,
        events: UnboundedSender<AgentEvent>,
    ) -> Option<Reply> {
        if questions.is_empty() {
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.lock().insert(id, Pending { sender });
        if events
            .send(AgentEvent::QuestionRequest { id, questions })
            .is_err()
        {
            self.lock().remove(&id);
            return None;
        }
        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(reply)) => Some(reply),
            // The request is gone either way: a timeout tells the front-end to
            // stop offering an answer nobody is waiting for, while a dropped
            // sender is a broker that went away with its run — the front-end
            // that dropped it already knows, so only the timeout is announced.
            Ok(Err(_)) => {
                self.lock().remove(&id);
                None
            }
            Err(_) => {
                self.lock().remove(&id);
                let _ = events.send(AgentEvent::QuestionClosed { id });
                None
            }
        }
    }

    /// Resolves a pending question from a front-end that already has the
    /// [`Reply`]. `false` means the id is unknown: the request already timed out
    /// or was answered somewhere else.
    pub fn answer(&self, id: u64, reply: Reply) -> bool {
        let Some(pending) = self.lock().remove(&id) else {
            return false;
        };
        pending.sender.send(reply).is_ok()
    }

    /// Resolves a pending question from a front-end that has wire values: an
    /// empty answer list is a dismissal, since the user closed the question
    /// without answering it.
    pub fn resolve(&self, id: u64, answers: Vec<Answer>) -> bool {
        let reply = if answers.is_empty() {
            Reply::Dismissed
        } else {
            Reply::Answers(answers)
        };
        self.answer(id, reply)
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<u64, Pending>> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Parses the `ask` tool's arguments into the questions a front-end paints,
/// capping them at [`MAX_QUESTIONS`] and [`MAX_OPTIONS`] so one call cannot
/// build a dialog nobody can read.
///
/// A call that names its questions under `questions` is the documented shape;
/// one that sends a single question object is accepted too, because a model that
/// asks about one thing drops the array.
pub fn parse_questions(arguments: &str) -> Result<Vec<Question>> {
    let args: Value = serde_json::from_str(arguments).map_err(|_| {
        anyhow::anyhow!(
            "ask received invalid JSON. Expected an object like \
             {{\"questions\":[{{\"question\":\"Which option?\",\"options\":[{{\"label\":\"A\"}}]}}]}}"
        )
    })?;
    let values: Vec<Value> = match args.get("questions") {
        Some(Value::Array(items)) => items.clone(),
        Some(other) => vec![other.clone()],
        None => vec![args],
    };
    let mut questions = Vec::new();
    for (index, value) in values.into_iter().enumerate() {
        let mut question: Question = serde_json::from_value(value).map_err(|_| {
            anyhow::anyhow!(
                "ask question {} needs a `question` string; `header`, `options`, and \
                 `multiSelect` are optional",
                index + 1
            )
        })?;
        question.question = question.question.trim().to_string();
        if question.question.is_empty() {
            continue;
        }
        question.header = question.header.filter(|header| !header.trim().is_empty());
        for choice in &mut question.options {
            choice.label = choice.label.trim().to_string();
        }
        let mut labels: Vec<String> = Vec::new();
        question.options.retain(|choice| {
            if choice.label.is_empty() || labels.iter().any(|label| label == &choice.label) {
                return false;
            }
            labels.push(choice.label.clone());
            true
        });
        question.options.truncate(MAX_OPTIONS);
        questions.push(question);
        if questions.len() == MAX_QUESTIONS {
            break;
        }
    }
    if questions.is_empty() {
        anyhow::bail!(
            "no questions were asked: give `questions` at least one entry with a `question`"
        );
    }
    Ok(questions)
}

/// What the model reads back as the tool result: the answers, or why it got
/// none. A question the user left blank is named as unanswered rather than
/// dropped, so the model does not report an answer nobody gave.
pub fn format_reply(questions: &[Question], reply: Option<Reply>) -> String {
    match reply {
        Some(Reply::Answers(answers)) => {
            let mut lines = vec!["The user answered:".to_string()];
            let mut answered = 0usize;
            for answer in &answers {
                let values: Vec<&str> = answer
                    .values
                    .iter()
                    .map(|value| value.trim())
                    .filter(|value| !value.is_empty())
                    .collect();
                if values.is_empty() {
                    lines.push(format!("- {} = (left blank)", answer.question));
                    continue;
                }
                answered += 1;
                lines.push(format!("- {} = {}", answer.question, values.join(", ")));
            }
            for question in questions {
                if !answers
                    .iter()
                    .any(|answer| answer.question == question.question)
                {
                    lines.push(format!("- {} = (left blank)", question.question));
                }
            }
            if answered == 0 {
                lines.push("Every question was left blank.".to_string());
            }
            lines.push("Continue with these answers.".to_string());
            lines.join("\n")
        }
        Some(Reply::Dismissed) => concat!(
            "The user dismissed the question without answering it. Do not ask it again: continue ",
            "with the most sensible default and say plainly what you assumed."
        )
        .to_string(),
        None => concat!(
            "Nobody answered: no front-end is waiting for a question in this run. Do not ask it ",
            "again: continue with the most sensible default and say plainly what you assumed."
        )
        .to_string(),
    }
}

/// Options are objects with a `label`; a model that sends its choices as plain
/// strings is read the same way rather than failing the whole call.
fn choices<'de, D>(deserializer: D) -> std::result::Result<Vec<Choice>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<Value>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .filter_map(|value| match value {
            Value::String(label) => Some(Choice {
                label,
                description: None,
            }),
            Value::Object(map) => match map.get("label") {
                Some(Value::String(label)) => Some(Choice {
                    label: label.clone(),
                    description: map
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                }),
                _ => None,
            },
            _ => None,
        })
        .collect())
}

/// `multiSelect` is a bool; a model that spelled it out — `"true"`, `"yes"`,
/// `1` — is read the same way rather than failing the whole call.
fn boolean<'de, D>(deserializer: D) -> std::result::Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match &value {
        Value::Bool(flag) => Ok(*flag),
        Value::Null => Ok(false),
        Value::Number(number) => number
            .as_i64()
            .and_then(|number| match number {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            })
            .ok_or_else(|| D::Error::custom(format!("expected a boolean, got {value}"))),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => Ok(true),
            "false" | "no" | "0" | "" => Ok(false),
            _ => Err(D::Error::custom(format!("expected a boolean, got {value}"))),
        },
        _ => Err(D::Error::custom(format!("expected a boolean, got {value}"))),
    }
}

/// A typed answer arrives as a list, but a front-end that collected one field
/// may send the value alone.
fn strings<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => Ok(vec![text]),
        Value::Array(items) => Ok(items
            .into_iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text),
                Value::Number(number) => Some(number.to_string()),
                other => other.as_str().map(str::to_string),
            })
            .collect()),
        other => Err(D::Error::custom(format!(
            "expected a string or a list of strings, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn parses_the_documented_shape() {
        let questions = parse_questions(
            r#"{"questions":[{"question":"Which database?","header":"Database","options":[{"label":"Postgres","description":"Relational"},{"label":"SQLite"}],"multiSelect":true}]}"#,
        )
        .expect("questions");
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].question, "Which database?");
        assert_eq!(questions[0].header.as_deref(), Some("Database"));
        assert!(questions[0].multi_select);
        assert_eq!(questions[0].options.len(), 2);
        assert_eq!(questions[0].options[0].label, "Postgres");
        assert_eq!(
            questions[0].options[0].description.as_deref(),
            Some("Relational")
        );
        assert_eq!(questions[0].options[1].description, None);
    }

    #[test]
    fn accepts_a_single_question_and_plain_string_options() {
        let questions = parse_questions(r#"{"question":"Name it?","options":["one","two"]}"#)
            .expect("questions");
        assert_eq!(questions.len(), 1);
        assert_eq!(
            questions[0]
                .options
                .iter()
                .map(|choice| choice.label.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
    }

    #[test]
    fn caps_questions_and_options_and_drops_duplicates() {
        let questions = parse_questions(
            r#"{"questions":[
                {"question":"a","options":[{"label":"x"},{"label":"x"},{"label":""},{"label":"y"}]},
                {"question":"b"},{"question":"c"},{"question":"d"},{"question":"e"}
            ]}"#,
        )
        .expect("questions");
        assert_eq!(questions.len(), MAX_QUESTIONS);
        assert_eq!(
            questions[0]
                .options
                .iter()
                .map(|choice| choice.label.as_str())
                .collect::<Vec<_>>(),
            ["x", "y"]
        );
    }

    #[test]
    fn rejects_a_call_with_no_question_text() {
        assert!(parse_questions(r#"{"questions":[{"question":"   "}]}"#).is_err());
        let missing = parse_questions("{}").unwrap_err().to_string();
        assert!(missing.contains("needs a `question` string"), "{missing}");
        assert!(!missing.contains("missing field"), "{missing}");

        let malformed = parse_questions("not json").unwrap_err().to_string();
        assert!(malformed.contains("received invalid JSON"), "{malformed}");
        assert!(!malformed.contains("line 1 column"), "{malformed}");
    }

    #[test]
    fn accepts_a_spelled_out_multi_select() {
        // A model that writes the flag out is read the same way as one that
        // sends a bool, since the call is not worth failing over spelling.
        for (value, expected) in [
            ("true", true),
            ("\"true\"", true),
            ("\"Yes\"", true),
            ("1", true),
            ("false", false),
            ("\"false\"", false),
            ("0", false),
            ("null", false),
        ] {
            let questions = parse_questions(&format!(
                r#"{{"question":"Which extras?","multiSelect":{value}}}"#
            ))
            .unwrap_or_else(|err| panic!("multiSelect {value}: {err:#}"));
            assert_eq!(questions[0].multi_select, expected, "multiSelect {value}");
        }
        // Anything else is a question that cannot be painted as asked.
        assert!(parse_questions(r#"{"question":"Which extras?","multiSelect":"maybe"}"#).is_err());
    }

    #[test]
    fn formats_answers_and_absences() {
        let questions = parse_questions(
            r#"{"questions":[{"question":"Which database?","options":[{"label":"Postgres"}]},{"question":"Anything else?"}]}"#,
        )
        .expect("questions");
        let text = format_reply(
            &questions,
            Some(Reply::Answers(vec![Answer {
                question: "Which database?".into(),
                values: vec!["Postgres".into()],
            }])),
        );
        assert!(text.contains("- Which database? = Postgres"), "{text}");
        assert!(text.contains("- Anything else? = (left blank)"), "{text}");

        let text = format_reply(&questions, Some(Reply::Dismissed));
        assert!(text.contains("dismissed"), "{text}");
        let text = format_reply(&questions, None);
        assert!(text.contains("Nobody answered"), "{text}");
    }

    #[test]
    fn parses_typed_and_blank_answers() {
        let answer: Answer =
            serde_json::from_value(serde_json::json!({"question":"Name?","values":"acme"}))
                .unwrap();
        assert_eq!(answer.values, ["acme"]);
        let answer: Answer =
            serde_json::from_value(serde_json::json!({"question":"Name?","values":[]})).unwrap();
        assert!(answer.values.is_empty());
        let answer: Answer =
            serde_json::from_value(serde_json::json!({"question":"Name?"})).unwrap();
        assert!(answer.values.is_empty());
    }

    #[tokio::test]
    async fn a_question_reaches_the_front_end_and_the_answer_comes_back() {
        let broker = AskBroker::new();
        let (tx, mut rx) = unbounded_channel();
        let asker = broker.asker(tx);
        let asking = tokio::spawn({
            let asker = Arc::clone(&asker);
            async move {
                asker(vec![Question {
                    question: "Which database?".into(),
                    header: None,
                    options: Vec::new(),
                    multi_select: false,
                }])
                .await
            }
        });
        let event = rx.recv().await.expect("a question request");
        let AgentEvent::QuestionRequest { id, questions } = event else {
            panic!("expected a question request");
        };
        assert_eq!(questions[0].question, "Which database?");
        assert!(broker.resolve(
            id,
            vec![Answer {
                question: questions[0].question.clone(),
                values: vec!["SQLite".into()],
            }]
        ));
        let reply = asking.await.unwrap().expect("an answer");
        assert_eq!(
            reply,
            Reply::Answers(vec![Answer {
                question: "Which database?".into(),
                values: vec!["SQLite".into()],
            }])
        );
        // The id is spent: a second answer is not applied to anything.
        assert!(!broker.resolve(id, Vec::new()));
    }

    #[tokio::test]
    async fn an_empty_answer_is_a_dismissal() {
        let broker = AskBroker::new();
        let (tx, mut rx) = unbounded_channel();
        let asker = broker.asker(tx);
        let asking = tokio::spawn({
            let asker = Arc::clone(&asker);
            async move {
                asker(vec![Question {
                    question: "Which database?".into(),
                    header: None,
                    options: Vec::new(),
                    multi_select: false,
                }])
                .await
            }
        });
        let event = rx.recv().await.expect("a question request");
        let AgentEvent::QuestionRequest { id, .. } = event else {
            panic!("expected a question request");
        };
        assert!(broker.resolve(id, Vec::new()));
        assert_eq!(asking.await.unwrap(), Some(Reply::Dismissed));
    }

    #[tokio::test]
    async fn an_unanswered_question_times_out() {
        let broker = Arc::new(AskBroker::from_timeout(Duration::from_millis(20)));
        let (tx, mut rx) = unbounded_channel();
        let asker = broker.asker(tx);
        let reply = asker(vec![Question {
            question: "Which database?".into(),
            header: None,
            options: Vec::new(),
            multi_select: false,
        }])
        .await;
        assert_eq!(reply, None);
        // The front-end is told the request is dead, so a dialog it painted
        // stops offering an answer nobody is waiting for any more.
        let event = rx.recv().await.expect("a request");
        let AgentEvent::QuestionRequest { id, .. } = event else {
            panic!("expected a question request");
        };
        let event = rx.recv().await.expect("a closed request");
        let AgentEvent::QuestionClosed { id: closed } = event else {
            panic!("expected a closed question");
        };
        assert_eq!(closed, id);
        assert!(!broker.resolve(closed, Vec::new()));
    }

    #[tokio::test]
    async fn a_question_with_no_audience_is_not_asked() {
        let broker = Arc::new(AskBroker::from_timeout(Duration::from_millis(20)));
        let (tx, rx) = unbounded_channel();
        drop(rx);
        let asker = broker.asker(tx);
        let reply = asker(vec![Question {
            question: "Which database?".into(),
            header: None,
            options: Vec::new(),
            multi_select: false,
        }])
        .await;
        assert_eq!(reply, None);
        assert!(asker(Vec::new()).await.is_none());
    }
}
