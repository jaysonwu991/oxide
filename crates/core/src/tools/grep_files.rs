use super::{command_path, resolve, strict_int_arg, NO_MATCHES};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

pub(super) async fn search(cwd: &Path, args: &Value) -> Result<String> {
    if let Some(mode) = args.get("output_mode") {
        anyhow::ensure!(
            mode == "files_with_matches",
            "unknown grep output_mode: {mode}"
        );
    }
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .context("grep requires a `pattern` string")?
        .trim();
    anyhow::ensure!(!pattern.is_empty(), "pattern must not be empty");
    let limit = strict_int_arg(args, "limit")?.unwrap_or(100);
    anyhow::ensure!(limit > 0, "limit must be greater than zero");
    let limit = limit.min(2000);
    let root = args
        .get("path")
        .and_then(Value::as_str)
        .map(|path| resolve(cwd, path))
        .unwrap_or_else(|| cwd.to_path_buf());
    std::fs::metadata(&root).with_context(|| format!("unable to access `{}`", root.display()))?;
    let rg =
        command_path("rg").context("failed to launch rg: install ripgrep and add it to PATH")?;
    let mut command = Command::new(rg);
    command.current_dir(cwd).kill_on_drop(true).args([
        "--files-with-matches",
        "--sortr=modified",
        "--regexp",
        pattern,
        "--no-messages",
    ]);
    if let Some(include) = args
        .get("include")
        .or_else(|| args.get("glob"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        command.arg("--glob").arg(include);
    }
    if super::bool_arg(args, "ignoreCase", "ignore_case") {
        command.arg("--ignore-case");
    }
    if super::opt_bool_arg(args, "literal") == Some(true) {
        command.arg("--fixed-strings");
    }
    command.arg("--").arg(root);
    run_search(command, limit, Duration::from_secs(30)).await
}

async fn run_search(mut command: Command, limit: usize, duration: Duration) -> Result<String> {
    let output = timeout(duration, command.output())
        .await
        .context("rg timed out after 30 seconds")?
        .context("failed to launch rg; ensure ripgrep is installed and on PATH")?;
    match output.status.code() {
        Some(0) => {
            let results = output
                .stdout
                .split(|byte| *byte == b'\n')
                .filter_map(|line| std::str::from_utf8(line).ok())
                .filter(|line| !line.is_empty())
                .take(limit)
                .collect::<Vec<_>>();
            Ok(if results.is_empty() {
                NO_MATCHES.to_string()
            } else {
                results.join("\n")
            })
        }
        Some(1) => Ok(NO_MATCHES.to_string()),
        _ => anyhow::bail!("rg failed: {}", String::from_utf8_lossy(&output.stderr)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{FunctionCall, ToolCall};
    use crate::mcp::McpRegistry;
    use crate::tools::{execute, Progress};
    use serde_json::json;
    use std::fs::{File, FileTimes};
    use std::time::{Duration as StdDuration, UNIX_EPOCH};

    fn call(args: Value) -> ToolCall {
        ToolCall {
            id: "grep".into(),
            kind: "function".into(),
            signature: None,
            function: FunctionCall {
                name: "grep".into(),
                arguments: args.to_string(),
            },
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn search_reports_failures_and_timeouts_without_partial_results() {
        let mut failure = Command::new("sh");
        failure.args([
            "-c",
            "printf 'partial.txt\\n'; printf 'bad search' >&2; exit 2",
        ]);
        let error = run_search(failure, 100, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "rg failed: bad search");

        let mut slow = Command::new("sh");
        slow.kill_on_drop(true).args(["-c", "exec sleep 30"]);
        let start = std::time::Instant::now();
        let error = run_search(slow, 100, Duration::from_millis(20))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn filename_search_matches_codex_order_filters_and_limits() {
        if command_path("rg").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("oxide_codex_grep_{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored.txt\n").unwrap();
        for (name, age) in [
            ("old.rs", 1),
            ("nested/new.rs", 3),
            ("other.txt", 2),
            (".hidden.rs", 0),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, "needle\nneedle again\n").unwrap();
            File::open(path)
                .unwrap()
                .set_times(FileTimes::new().set_modified(UNIX_EPOCH + StdDuration::from_secs(age)))
                .unwrap();
        }
        std::fs::write(dir.join("ignored.txt"), "needle\n").unwrap();
        std::fs::write(dir.join("binary.rs"), b"\0needle\n").unwrap();
        let result = search(&dir, &json!({"pattern": " needle ", "include": " *.rs "}))
            .await
            .unwrap();
        assert_eq!(
            result.lines().collect::<Vec<_>>(),
            [
                dir.join("nested/new.rs").to_str().unwrap(),
                dir.join("old.rs").to_str().unwrap(),
                dir.join(".hidden.rs").to_str().unwrap(),
            ]
        );
        assert_eq!(
            search(&dir, &json!({"pattern": "needle", "limit": 1}))
                .await
                .unwrap(),
            dir.join("nested/new.rs").to_str().unwrap()
        );
        let out = execute(
            &call(json!({"pattern": "needle", "limit": 1})),
            &dir,
            &McpRegistry::default(),
            &Progress::default(),
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(out.text, dir.join("nested/new.rs").to_str().unwrap());
        assert_eq!(
            search(&dir, &json!({"pattern": "needle", "path": "old.rs"}))
                .await
                .unwrap(),
            dir.join("old.rs").to_str().unwrap()
        );
        assert_eq!(
            search(&dir, &json!({"pattern": "absent"})).await.unwrap(),
            NO_MATCHES
        );
        assert!(search(&dir, &json!({"pattern": "["}))
            .await
            .unwrap_err()
            .to_string()
            .contains("rg failed"));
        assert!(
            search(&dir, &json!({"pattern": "needle", "path": "missing"}))
                .await
                .is_err()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn filename_search_defaults_to_100_and_caps_at_2000() {
        if command_path("rg").is_none() {
            return;
        }
        let dir =
            std::env::temp_dir().join(format!("oxide_codex_grep_limits_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for index in 0..2001 {
            std::fs::write(dir.join(format!("{index}.txt")), "needle\n").unwrap();
        }
        for (limit, expected) in [(None, 100), (Some(5000), 2000)] {
            let mut args = json!({"pattern": "needle"});
            if let Some(limit) = limit {
                args["limit"] = json!(limit);
            }
            assert_eq!(search(&dir, &args).await.unwrap().lines().count(), expected);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn dispatch_reports_invalid_filename_search_arguments() {
        for args in [
            json!({"pattern": "   "}),
            json!({"pattern": "needle", "limit": 0}),
            json!({"pattern": "needle", "limit": -1}),
            json!({"pattern": "needle", "limit": "invalid"}),
            json!({"pattern": "needle", "output_mode": "invalid"}),
        ] {
            let out = execute(
                &call(args.clone()),
                Path::new("."),
                &McpRegistry::default(),
                &Progress::default(),
            )
            .await;
            assert!(out.is_error, "{args}: {}", out.text);
        }
    }
}
