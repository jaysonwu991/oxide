//! The repository's pull requests, as the review pane shows one: what the forge
//! says about the request, its checks, its comments and the files it touches.
//!
//! The forge's own CLI answers (`gh` for GitHub, `glab` for GitLab), and its JSON
//! is parsed here rather than in the window: the shapes differ per forge and per
//! version, so the narrowing lives beside the command that ran. A CLI that is
//! missing, logged out or offline is one answer the caller can show — the pane
//! says which — rather than a failed command with nothing in it.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long the forge's CLI gets before it is treated as not answering: a
/// listing the pane waits on must not hang on a network that is not there.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The fields the pane paints, and the JSON names they come from. Keeping the
/// list here means a forge that renames one is a parsing failure the pane reports
/// rather than a field that quietly reads empty.
const LIST_FIELDS: &str =
    "number,title,state,isDraft,url,headRefName,reviewDecision,updatedAt,author";
const VIEW_FIELDS: &str = "number,title,body,state,isDraft,url,headRefName,baseRefName,reviewDecision,mergeable,additions,deletions,changedFiles,author,comments,reviews,files,statusCheckRollup";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub draft: bool,
    pub url: String,
    pub head: String,
    pub base: String,
    /// The forge's own review decision: `APPROVED`, `CHANGES_REQUESTED`, or empty.
    pub review: String,
    pub author: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    pub name: String,
    /// `pass`, `fail` or `pending` — what the pane draws, not the forge's words.
    pub state: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comment {
    pub author: String,
    pub body: String,
    pub created_at: String,
    /// Whether the comment is a review's verdict rather than a remark.
    pub review: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChangedFile {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PullDetail {
    #[serde(flatten)]
    pub pull: PullRequest,
    pub body: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub mergeable: String,
    pub checks: Vec<Check>,
    pub comments: Vec<Comment>,
    pub files: Vec<ChangedFile>,
}

/// The forge and the command each answer needs. Both CLIs take the same JSON
/// request in their own spelling, and both answer a request's number the same
/// way, which is what keeps one pane over two forges honest.
fn forge(project: &Path) -> (&'static str, &'static str) {
    if project.join(".gitlab-ci.yml").exists() || project.join(".gitlab").exists() {
        ("glab", "mr")
    } else {
        ("gh", "pr")
    }
}

fn run(cli: &str, args: &[&str], cwd: &Path) -> Result<String> {
    let mut child = Command::new(cli)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running `{cli}` — is the CLI installed?"))?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut answer = String::new();
                let mut error = String::new();
                if let Some(mut stdout) = child.stdout.take() {
                    stdout.read_to_string(&mut answer)?;
                }
                if let Some(mut stderr) = child.stderr.take() {
                    stderr.read_to_string(&mut error)?;
                }
                if !status.success() {
                    let reason = error.trim();
                    bail!(
                        "{}",
                        if reason.is_empty() {
                            format!("`{cli}` exited with {status}")
                        } else {
                            reason.to_string()
                        }
                    );
                }
                return Ok(answer);
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("`{cli}` did not answer within {}s", TIMEOUT.as_secs());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(error) => return Err(error).context("waiting for the forge's CLI"),
        }
    }
}

/// The repository's open requests, newest first as the forge lists them.
pub fn list(project: &Path, limit: usize) -> Result<Vec<PullRequest>> {
    let (cli, noun) = forge(project);
    let limit = limit.to_string();
    let args = if cli == "gh" {
        vec![noun, "list", "--limit", &limit, "--json", LIST_FIELDS]
    } else {
        vec![noun, "list", "--per-page", &limit, "--output", "json"]
    };
    let answer = run(cli, &args, project)?;
    let value: Value = serde_json::from_str(&answer).context("reading the forge's listing")?;
    let rows = value
        .as_array()
        .context("the forge's listing was not a list")?;
    Ok(rows.iter().filter_map(|row| pull(row, cli).ok()).collect())
}

/// One request, with what a reviewer reads before approving it.
pub fn view(project: &Path, number: u64) -> Result<PullDetail> {
    let (cli, noun) = forge(project);
    let number_text = number.to_string();
    let args = if cli == "gh" {
        vec![noun, "view", &number_text, "--json", VIEW_FIELDS]
    } else {
        vec![noun, "view", &number_text, "--output", "json"]
    };
    let answer = run(cli, &args, project)?;
    let value: Value = serde_json::from_str(&answer).context("reading the request")?;
    detail(&value, cli)
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn count(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn pull(value: &Value, cli: &str) -> Result<PullRequest> {
    let number = value
        .get("number")
        .or_else(|| value.get("iid"))
        .and_then(Value::as_u64)
        .context("a request without a number")?;
    // `gh` names the branch `headRefName`; `glab` nests it under `source_branch`.
    let head = if cli == "gh" {
        text(value, "headRefName")
    } else {
        value
            .get("source_branch")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default()
    };
    let review = text(value, "reviewDecision");
    Ok(PullRequest {
        number,
        title: text(value, "title"),
        state: text(value, "state").to_lowercase(),
        draft: value
            .get("isDraft")
            .or_else(|| value.get("draft"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        url: {
            let url = text(value, "url");
            if url.is_empty() {
                text(value, "web_url")
            } else {
                url
            }
        },
        head,
        base: text(value, "baseRefName"),
        review,
        author: value
            .get("author")
            .and_then(|author| author.get("login").or_else(|| author.get("username")))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

fn check_state(entry: &Value) -> Check {
    // A check run says `conclusion`; a status context says `state`.
    let conclusion = text(entry, "conclusion");
    let context_state = text(entry, "state");
    let state = if matches!(conclusion.as_str(), "SUCCESS" | "NEUTRAL" | "SKIPPED") {
        "pass"
    } else if matches!(
        conclusion.as_str(),
        "FAILURE" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED"
    ) {
        "fail"
    } else if matches!(context_state.as_str(), "SUCCESS") {
        "pass"
    } else if matches!(context_state.as_str(), "FAILURE" | "ERROR") {
        "fail"
    } else {
        // Anything the forge has not finished with is pending: a check that is
        // queued, running, expected, or of a kind this build does not know is
        // neither a pass nor a failure.
        "pending"
    };
    // A check run names itself `name`; a status context `context`, and the one it
    // does not have reads empty.
    let name = {
        let name = text(entry, "name");
        if name.is_empty() {
            text(entry, "context")
        } else {
            name
        }
    };
    let url = {
        let url = text(entry, "detailsUrl");
        if url.is_empty() {
            text(entry, "targetUrl")
        } else {
            url
        }
    };
    Check {
        name,
        state: state.to_string(),
        url,
    }
}

fn detail(value: &Value, cli: &str) -> Result<PullDetail> {
    let pull = pull(value, cli)?;
    let checks = value
        .get("statusCheckRollup")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().map(check_state).collect())
        .unwrap_or_default();
    let comments = value
        .get("comments")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| Comment {
                    author: row
                        .get("author")
                        .and_then(|author| author.get("login"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    body: text(row, "body"),
                    created_at: text(row, "createdAt"),
                    review: String::new(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut comments = comments;
    // A review is the verdict a comment cannot carry, so it is listed with the
    // comments and says which verdict it was.
    if let Some(reviews) = value.get("reviews").and_then(Value::as_array) {
        for row in reviews {
            let body = text(row, "body");
            if body.trim().is_empty() {
                continue;
            }
            comments.push(Comment {
                author: row
                    .get("author")
                    .and_then(|author| author.get("login"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                body,
                created_at: text(row, "createdAt"),
                review: text(row, "state").to_lowercase(),
            });
        }
    }
    let files = value
        .get("files")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| ChangedFile {
                    path: text(row, "path"),
                    additions: count(row, "additions"),
                    deletions: count(row, "deletions"),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(PullDetail {
        pull,
        body: text(value, "body"),
        additions: count(value, "additions"),
        deletions: count(value, "deletions"),
        changed_files: count(value, "changedFiles"),
        mergeable: text(value, "mergeable").to_lowercase(),
        checks,
        comments,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn gh_list_row() -> Value {
        json!({
            "number": 12,
            "title": "Add a terminal",
            "state": "OPEN",
            "isDraft": false,
            "url": "https://github.com/o/r/pull/12",
            "headRefName": "feat/terminal",
            "reviewDecision": "APPROVED",
            "updatedAt": "2026-01-01T00:00:00Z",
            "author": { "login": "jayson" }
        })
    }

    #[test]
    fn a_listing_row_reads_the_fields_the_pane_draws() {
        let request = pull(&gh_list_row(), "gh").unwrap();
        assert_eq!(request.number, 12);
        assert_eq!(request.title, "Add a terminal");
        assert_eq!(request.state, "open");
        assert!(!request.draft);
        assert_eq!(request.head, "feat/terminal");
        assert_eq!(request.review, "APPROVED");
        assert_eq!(request.author, "jayson");
    }

    #[test]
    fn a_forge_that_spells_it_differently_still_reads() {
        // `glab`'s spelling of the same request.
        let row = json!({
            "iid": 7,
            "title": "Fix the login redirect",
            "state": "opened",
            "web_url": "https://gitlab.com/o/r/-/merge_requests/7",
            "source_branch": "fix/login",
            "author": { "username": "jayson" }
        });
        let request = pull(&row, "glab").unwrap();
        assert_eq!(request.number, 7);
        assert_eq!(request.head, "fix/login");
        assert_eq!(request.author, "jayson");
        assert_eq!(request.url, "https://gitlab.com/o/r/-/merge_requests/7");
        assert_eq!(request.state, "opened");
    }

    #[test]
    fn the_checks_are_reduced_to_three_states() {
        let rollup = json!([
            { "name": "test", "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": "u1" },
            { "name": "lint", "status": "COMPLETED", "conclusion": "FAILURE", "detailsUrl": "u2" },
            { "name": "build", "status": "IN_PROGRESS", "conclusion": null },
            { "context": "ci/legacy", "state": "SUCCESS", "targetUrl": "u3" }
        ]);
        let checks: Vec<Check> = rollup.as_array().unwrap().iter().map(check_state).collect();
        assert_eq!(
            checks
                .iter()
                .map(|check| check.state.as_str())
                .collect::<Vec<_>>(),
            vec!["pass", "fail", "pending", "pass"]
        );
        assert_eq!(checks[3].name, "ci/legacy");
        assert_eq!(checks[0].url, "u1");
    }

    #[test]
    fn the_detail_carries_the_body_reviews_comments_and_files() {
        let mut value = gh_list_row();
        value["body"] = json!("What this does");
        value["additions"] = json!(120);
        value["deletions"] = json!(4);
        value["changedFiles"] = json!(3);
        value["mergeable"] = json!("MERGEABLE");
        value["comments"] = json!([
            { "author": { "login": "reviewer" }, "body": "Looks good", "createdAt": "2026-01-02T00:00:00Z" }
        ]);
        value["reviews"] = json!([
            { "author": { "login": "reviewer" }, "body": "", "state": "APPROVED" },
            { "author": { "login": "other" }, "body": "Please rename this", "state": "CHANGES_REQUESTED" }
        ]);
        value["files"] = json!([
            { "path": "crates/desktop/src/terminal.rs", "additions": 100, "deletions": 2 }
        ]);
        value["statusCheckRollup"] = json!([
            { "name": "desktop", "status": "COMPLETED", "conclusion": "SUCCESS" }
        ]);
        let detail = detail(&value, "gh").unwrap();
        assert_eq!(detail.body, "What this does");
        assert_eq!(detail.additions, 120);
        assert_eq!(detail.deletions, 4);
        assert_eq!(detail.changed_files, 3);
        assert_eq!(detail.mergeable, "mergeable");
        // A review with no body is a verdict, not a comment: it is not listed as
        // one, since the review decision already says it.
        assert_eq!(detail.comments.len(), 2);
        assert_eq!(detail.comments[1].review, "changes_requested");
        assert_eq!(detail.files[0].path, "crates/desktop/src/terminal.rs");
        assert_eq!(detail.checks[0].state, "pass");
        assert_eq!(detail.pull.review, "APPROVED");
    }

    #[test]
    fn a_request_with_no_number_is_not_a_request() {
        assert!(pull(&json!({ "title": "no number" }), "gh").is_err());
    }

    #[test]
    fn the_forge_is_read_off_the_repository() {
        let dir = std::env::temp_dir().join(format!("oxide_review_forge_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(forge(&dir).0, "gh");
        std::fs::write(dir.join(".gitlab-ci.yml"), "stages: []\n").unwrap();
        assert_eq!(forge(&dir).0, "glab");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
