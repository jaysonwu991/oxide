use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

const HARNESS: &str = r#"
import { pathToFileURL } from "node:url"
import { createInterface } from "node:readline"
import { spawn } from "node:child_process"

const files = process.argv.slice(2)

function shell() {
  return function $(strings, ...values) {
    let command = strings[0]
    for (let i = 0; i < values.length; i++) command += String(values[i]) + strings[i + 1]
    const options = { cwd: process.cwd(), quiet: false, nothrow: false }
    const run = () =>
      new Promise((resolve, reject) => {
        const child = spawn(command, { shell: true, cwd: options.cwd })
        let stdout = ""
        let stderr = ""
        child.stdout?.on("data", (chunk) => {
          stdout += chunk
          if (!options.quiet) process.stderr.write(chunk)
        })
        child.stderr?.on("data", (chunk) => {
          stderr += chunk
          if (!options.quiet) process.stderr.write(chunk)
        })
        child.on("error", reject)
        child.on("close", (code) => {
          if (code !== 0 && !options.nothrow) {
            reject(new Error(`command failed: ${command}\n${stderr}`))
          } else {
            resolve({ stdout, stderr, exitCode: code })
          }
        })
      })
    const chain = {
      cwd(dir) { options.cwd = dir; return chain },
      quiet() { options.quiet = true; return chain },
      nothrow() { options.nothrow = true; return chain },
      then(resolve, reject) { return run().then(resolve, reject) },
      catch(reject) { return run().catch(reject) },
    }
    return chain
  }
}

const input = {
  client: {},
  project: {},
  directory: process.cwd(),
  worktree: process.cwd(),
  $: shell(),
}

const registered = []
for (const file of files) {
  try {
    const module = await import(pathToFileURL(file).href)
    const exported = module.default ?? module.plugin
    if (typeof exported === "function") {
      const result = await exported(input)
      if (result && typeof result === "object") registered.push(result)
    } else if (exported && typeof exported === "object") {
      registered.push(exported)
    } else {
      for (const value of Object.values(module)) {
        if (value && typeof value === "object") {
          registered.push(value)
        } else if (typeof value === "function") {
          const result = await value(input)
          if (result && typeof result === "object") registered.push(result)
        }
      }
    }
  } catch (err) {
    process.stderr.write(`[plugin] failed to load ${file}: ${err}\n`)
  }
}

async function dispatch(name, hookInput, hookOutput) {
  for (const plugin of registered) {
    const handler = plugin[name]
    if (typeof handler === "function") await handler(hookInput, hookOutput)
  }
  return hookOutput
}

const rl = createInterface({ input: process.stdin })
rl.on("line", async (line) => {
  const text = line.trim()
  if (!text) return
  let message
  try {
    message = JSON.parse(text)
  } catch {
    return
  }
  try {
    const output = await dispatch(message.hook, message.input ?? {}, message.output ?? {})
    process.stdout.write(JSON.stringify({ id: message.id, output: output ?? {} }) + "\n")
  } catch (err) {
    process.stdout.write(JSON.stringify({ id: message.id, error: String(err) }) + "\n")
  }
})
"#;

pub struct PluginHost {
    inner: Option<Mutex<Host>>,
    count: usize,
    harness: Option<PathBuf>,
}

struct Host {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _child: Child,
    next_id: u64,
}

impl PluginHost {
    pub async fn spawn(plugins: &[PathBuf], cwd: &Path) -> Self {
        if plugins.is_empty() {
            return Self::inactive();
        }
        let Some(runtime) = detect_runtime() else {
            eprintln!("[plugin] no bun or node runtime found; plugins disabled");
            return Self::inactive();
        };
        let harness = match write_harness() {
            Ok(path) => path,
            Err(err) => {
                eprintln!("[plugin] failed to prepare host: {err:#}");
                return Self::inactive();
            }
        };

        let mut command = tokio::process::Command::new(runtime);
        command.arg(&harness);
        for plugin in plugins {
            command.arg(plugin);
        }
        command
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        match command.spawn() {
            Ok(mut child) => {
                let stdin = child.stdin.take().expect("piped stdin");
                let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
                Self {
                    inner: Some(Mutex::new(Host {
                        stdin,
                        stdout,
                        _child: child,
                        next_id: 0,
                    })),
                    count: plugins.len(),
                    harness: Some(harness),
                }
            }
            Err(err) => {
                std::fs::remove_file(&harness).ok();
                eprintln!("[plugin] failed to start {runtime}: {err}");
                Self::inactive()
            }
        }
    }

    fn inactive() -> Self {
        Self {
            inner: None,
            count: 0,
            harness: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.inner.is_some()
    }

    pub fn plugin_count(&self) -> usize {
        self.count
    }

    pub async fn tool_before(&self, tool: &str, args: &Value) -> Option<Value> {
        let host = self.inner.as_ref()?;
        let mut host = host.lock().await;
        let response = host
            .call(
                "tool.execute.before",
                json!({ "tool": tool, "args": args }),
                json!({ "args": args }),
            )
            .await
            .ok()?;
        response.get("args").cloned()
    }

    pub async fn tool_after(&self, tool: &str, args: &Value, output: &str) -> Option<String> {
        let host = self.inner.as_ref()?;
        let mut host = host.lock().await;
        let response = host
            .call(
                "tool.execute.after",
                json!({ "tool": tool, "args": args }),
                json!({ "title": "", "output": output, "metadata": {} }),
            )
            .await
            .ok()?;
        response
            .get("output")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        if let Some(path) = &self.harness {
            std::fs::remove_file(path).ok();
        }
    }
}

impl Host {
    async fn call(&mut self, hook: &str, input: Value, output: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "id": id, "hook": hook, "input": input, "output": output });
        let mut line = serde_json::to_string(&request).context("encoding plugin request")?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .context("writing to plugin host")?;
        self.stdin.flush().await.context("flushing plugin host")?;

        loop {
            let mut buffer = String::new();
            let read = tokio::time::timeout(REQUEST_TIMEOUT, self.stdout.read_line(&mut buffer))
                .await
                .with_context(|| format!("plugin hook `{hook}` timed out"))?
                .context("reading from plugin host")?;
            if read == 0 {
                bail!("plugin host exited");
            }
            let text = buffer.trim();
            if text.is_empty() {
                continue;
            }
            let Ok(response) = serde_json::from_str::<Value>(text) else {
                continue;
            };
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error").and_then(Value::as_str) {
                bail!("plugin hook `{hook}` failed: {error}");
            }
            return Ok(response.get("output").cloned().unwrap_or(Value::Null));
        }
    }
}

fn detect_runtime() -> Option<&'static str> {
    ["bun", "node"]
        .into_iter()
        .find(|name| command_exists(name))
}

fn command_exists(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file() || candidate.with_extension("exe").is_file()
    })
}

fn write_harness() -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("oxide-plugin-host-{}.mjs", std::process::id()));
    std::fs::write(&path, HARNESS).context("writing plugin host script")?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_plugin_{tag}_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn runs_js_plugin_hooks() {
        if !command_exists("node") {
            return;
        }
        let dir = temp_dir("hooks");
        let plugin = dir.join("hooks.mjs");
        std::fs::write(
            &plugin,
            r#"
export default async () => ({
  "tool.execute.before": async (input, output) => {
    if (input.tool === "write_file") output.args = { ...output.args, content: "hooked" };
  },
  "tool.execute.after": async (input, output) => {
    output.output = `${output.output} [seen]`;
  },
});
"#,
        )
        .unwrap();

        let host = PluginHost::spawn(&[plugin], &dir).await;
        assert!(host.is_active());

        let before = host
            .tool_before(
                "write_file",
                &json!({ "path": "a.rs", "content": "original" }),
            )
            .await
            .unwrap();
        assert_eq!(
            before.get("content").and_then(Value::as_str),
            Some("hooked")
        );

        let after = host
            .tool_after("write_file", &json!({ "path": "a.rs" }), "done")
            .await
            .unwrap();
        assert_eq!(after, "done [seen]");

        drop(host);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn inactive_without_plugins() {
        let host = PluginHost::spawn(&[], Path::new(".")).await;
        assert!(!host.is_active());
        assert_eq!(host.plugin_count(), 0);
        assert!(host.tool_before("write_file", &json!({})).await.is_none());
    }
}
