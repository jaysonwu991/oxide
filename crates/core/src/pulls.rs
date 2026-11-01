//! Pull and merge request links a conversation reports, and the state each one
//! is in.
//!
//! A turn that opens a request prints the URL its forge CLI answered with, so a
//! thread's log is where the link is written. Reading the link back out — and
//! reading the state a forge CLI reports for it — is shared here rather than
//! re-derived by each front-end that annotates a thread with it.

use serde_json::Value;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a forge CLI is given to answer before the read is abandoned: a
/// listing annotates a row from this, and a forge that is slow or offline must
/// not hold the sidebar.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The state a pull or merge request was last seen in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullState {
    Open,
    Merged,
    Closed,
}

impl PullState {
    pub fn as_str(self) -> &'static str {
        match self {
            PullState::Open => "open",
            PullState::Merged => "merged",
            PullState::Closed => "closed",
        }
    }

    /// The vocabulary a forge CLI uses, which differs between them: `gh` says
    /// `OPEN`, `glab` says `opened`.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "open" | "opened" => Some(PullState::Open),
            "merged" => Some(PullState::Merged),
            "closed" => Some(PullState::Closed),
            _ => None,
        }
    }
}

/// One pull or merge request named in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pull {
    /// The canonical URL, which is what a click opens and what a state is
    /// keyed by.
    pub url: String,
    /// How the request is named in the forge's own shorthand — `#42` for a pull
    /// request, `!42` for a merge request.
    pub label: String,
    /// The forge's official CLI, which is what reads the request's state.
    pub cli: &'static str,
}

/// Resolves a forge work-item URL to the pull or merge request it names.
pub fn link(raw: &str) -> Option<Pull> {
    let trimmed = raw.trim().trim_end_matches([
        '.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\'', '`',
    ]);
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))?;
    let mut segments = rest.split('/');
    let host = segments.next()?.trim_start_matches("www.");
    let owner = segments.next().filter(|part| !part.is_empty())?;
    let repo = segments.next().filter(|part| !part.is_empty())?;
    match host {
        "github.com" => {
            let kind = segments.next()?;
            let number = segments.next()?;
            if kind != "pull" {
                return None;
            }
            let number = numeric(number)?;
            Some(Pull {
                url: format!("https://github.com/{owner}/{repo}/pull/{number}"),
                label: format!("#{number}"),
                cli: "gh",
            })
        }
        "gitlab.com" => {
            let mut next = segments.next()?;
            if next == "-" {
                next = segments.next()?;
            }
            if next != "merge_requests" {
                return None;
            }
            let number = numeric(segments.next()?)?;
            Some(Pull {
                url: format!("https://gitlab.com/{owner}/{repo}/-/merge_requests/{number}"),
                label: format!("!{number}"),
                cli: "glab",
            })
        }
        _ => None,
    }
}

fn numeric(raw: &str) -> Option<&str> {
    let trimmed = raw.trim_start_matches('#').trim_start_matches('!');
    (!trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit())).then_some(trimmed)
}

/// Every pull or merge request named in a text, in the order it first appears
/// and once each.
pub fn links(text: &str) -> Vec<Pull> {
    let mut found: Vec<Pull> = Vec::new();
    for word in text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '"' | '\'' | '`' | ','
            )
    }) {
        let Some(pull) = link(word) else { continue };
        if !found.iter().any(|known| known.url == pull.url) {
            found.push(pull);
        }
    }
    found
}

/// Reads the request's state through its forge CLI. `None` is a forge that did
/// not answer — a missing CLI, no network, or a request it cannot see — which a
/// caller shows as a state not yet known rather than as a failure.
pub fn read_state(pull: &Pull) -> Option<PullState> {
    let args: &[&str] = match pull.cli {
        "gh" => &["pr", "view", &pull.url, "--json", "state"],
        _ => &["mr", "view", &pull.url, "--output", "json"],
    };
    let answer = run(pull.cli, args)?;
    let value: Value = serde_json::from_str(&answer).ok()?;
    PullState::parse(value.get("state")?.as_str()?)
}

fn run(cli: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(cli)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut answer = String::new();
                child.stdout.take()?.read_to_string(&mut answer).ok()?;
                return Some(answer);
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_request_url_is_read_as_the_request_it_names() {
        let pull = link("https://github.com/jaysonwu991/oxide/pull/176").unwrap();
        assert_eq!(pull.url, "https://github.com/jaysonwu991/oxide/pull/176");
        assert_eq!(pull.label, "#176");
        assert_eq!(pull.cli, "gh");
    }

    #[test]
    fn a_merge_request_url_is_read_too() {
        let pull = link("https://gitlab.com/group/app/-/merge_requests/12").unwrap();
        assert_eq!(pull.url, "https://gitlab.com/group/app/-/merge_requests/12");
        assert_eq!(pull.label, "!12");
        assert_eq!(pull.cli, "glab");
        assert_eq!(
            link("https://gitlab.com/group/app/merge_requests/12")
                .unwrap()
                .url,
            "https://gitlab.com/group/app/-/merge_requests/12"
        );
    }

    #[test]
    fn the_url_a_turn_printed_is_trimmed_of_its_sentence() {
        let text = "done: (https://github.com/owner/repo/pull/42).\n";
        assert_eq!(
            links(text),
            vec![Pull {
                url: "https://github.com/owner/repo/pull/42".to_string(),
                label: "#42".to_string(),
                cli: "gh",
            }]
        );
    }

    #[test]
    fn a_url_that_is_not_a_request_is_not_one() {
        assert!(link("https://github.com/owner/repo").is_none());
        assert!(link("https://github.com/owner/repo/pull/").is_none());
        assert!(link("https://github.com/owner/repo/pull/abc").is_none());
        assert!(link("https://github.com/owner/repo/issues/7").is_none());
        assert!(link("https://example.com/owner/repo/pull/7").is_none());
        assert!(link("github.com/owner/repo/pull/7").is_none());
    }

    #[test]
    fn one_request_is_named_once_however_often_it_is_printed() {
        let text = "https://github.com/owner/repo/pull/7 and again \
                    https://github.com/owner/repo/pull/7, then \
                    https://github.com/owner/repo/pull/8.";
        let found = links(text);
        assert_eq!(
            found
                .iter()
                .map(|pull| pull.label.as_str())
                .collect::<Vec<_>>(),
            vec!["#7", "#8"]
        );
    }

    #[test]
    fn both_forges_vocabularies_read_as_states() {
        assert_eq!(PullState::parse("OPEN"), Some(PullState::Open));
        assert_eq!(PullState::parse("opened"), Some(PullState::Open));
        assert_eq!(PullState::parse("MERGED"), Some(PullState::Merged));
        assert_eq!(PullState::parse("closed"), Some(PullState::Closed));
        assert_eq!(PullState::parse("draft"), None);
        assert_eq!(PullState::Open.as_str(), "open");
        assert_eq!(PullState::Merged.as_str(), "merged");
    }
}
