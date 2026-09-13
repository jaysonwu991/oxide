# oxide

A native Rust AI coding agent CLI for the terminal. oxide streams from
OpenAI-compatible and Anthropic models, runs a tool-using agent loop against
your project, and understands its own `.oxide/` configuration layout out of the
box, with Claude Code configuration support for compatibility.

## Features

- Interactive TUI (ratatui) plus a non-interactive `-p/--print` mode.
- OpenAI-compatible (OpenAI, DeepSeek, custom) and Anthropic Messages API clients.
- Built-in tools: `read_file`, `write_file`, `list_dir`, `bash`, `glob`, `grep`,
  `patch`, `webfetch`.
- Agent-level tools: `task` (subagents), `skill` (on-demand skill loading),
  `memory` (cross-session notes), `diagnostics` (LSP diagnostics).
- MCP servers over stdio or HTTP (including OAuth-protected remote servers),
  exposed as `<server>__<tool>`.
- Multimodal prompts: attach images/PDFs with `--image` or `@path` references.
- Project + global ecosystem discovery: rules, memory, commands, agents,
  skills, MCP servers, and plugins from `.oxide/` (plus the Claude Code layout).
- Durable sessions, shadow-git snapshots (`/undo`, `/redo`), and context
  compaction (`/compact`).
- Dynamic context pruning: a `compress` tool plus automatic tool-output
  deduplication and error purging that shrink outgoing context without altering
  session history.
- LSP diagnostics via rust-analyzer, typescript-language-server, pyright, gopls.
- Plugin hooks (`tool.execute.before` / `tool.execute.after`) run under bun/node,
  and a hook can end the turn by setting `output.terminate = true`.
- Agent-harness niceties: read-only tool calls in a batch run in parallel while
  preserving model order, `bash` output streams into the UI as it arrives, and
  typing while the agent works steers it between steps.
- Agent modes: `build` (default), `plan` (read-only planning), and `auto-edit`
  (auto-approve file edits), cycled in the TUI with Shift+Tab or set with
  `--mode` / `OXIDE_MODE`.
- Reasoning effort: `auto` (default), `off`, `low`, `medium`, or `high`, cycled
  in the TUI with Ctrl+R or set with `--reasoning` / `OXIDE_REASONING`. `auto`
  enables reasoning for models known to support it and maps to OpenAI
  `reasoning_effort` and Anthropic extended thinking.
- Compact, bounded output: tool bodies are hidden in the TUI by default (Ctrl+O
  toggles them), long action lines are truncated to the terminal width, and tool
  results are capped by lines and bytes before they enter the model's context.
  Capped output is saved to disk with a pointer so it stays recoverable.
- Incremental TUI rendering: only conversation items that changed since the last
  frame are re-wrapped and re-styled.

## Comparison

oxide is a small, native terminal agent that deliberately borrows the Claude
Code configuration layout so existing `.claude/` setups keep working. The table
below compares the high-level shape of the three tools; feature sets move fast,
so check each project's documentation for the current details.

| Capability | oxide | [OpenCode](https://opencode.ai) | [Claude Code](https://docs.claude.com/en/docs/claude-code/overview) |
| --- | --- | --- | --- |
| Distribution | Native Rust binary | Open-source CLI (Node/Bun) | Proprietary CLI + apps |
| License | MIT | Open source | Proprietary |
| Model providers | OpenAI-compatible + Anthropic (OpenAI, DeepSeek, custom) | Any provider (bring your own keys) | Claude (Anthropic API, Bedrock, Vertex, third-party) |
| Interfaces | Terminal TUI, `-p` print mode | Terminal, desktop, IDE, web | Terminal, IDE, desktop, web |
| Project config | `.oxide/` + `AGENTS.md` (also reads `.claude/`) | `opencode.json` + `AGENTS.md` | `CLAUDE.md` + `.claude/` |
| Subagents | `--agent`, `task`, command routing | Agents | Subagents, background agents |
| Permission modes | `build` / `plan` / `auto-edit` (Shift+Tab, `--mode`) | Build / plan agents | default / accept-edits / plan / bypass |
| Reasoning effort | `auto` / `off` / `low` / `medium` / `high` (Ctrl+R, `--reasoning`) | Model-dependent | Extended thinking |
| Slash commands | `.oxide/commands` with `agent`/`subtask` routing | Commands | Commands |
| Skills | `SKILL.md` | Agent Skills | Skills |
| MCP servers | stdio + HTTP + OAuth, managed with `oxide mcp` | MCP servers | MCP servers |
| Plugins / hooks | JS/TS hooks (bun/node) | Plugins | Hooks, plugins, Agent SDK |
| LSP diagnostics | Built in (rust-analyzer, TS, pyright, gopls) | Built in (LSP servers) | — |
| Undo file changes | Shadow-git `/undo`, `/redo` | `/undo`, `/redo` | Git / checkpoints |
| Sessions | Durable JSONL, `-c` / `--resume` | Sessions, share links | Sessions across surfaces |
| Context management | Built-in pruning (`compress` tool, dedup, error purge) | Auto-compaction + DCP plugin | Auto-compaction |
| Multimodal input | Images and PDFs (`--image`, `@path`) | Images | Images |

A dash indicates no first-class built-in equivalent. Where oxide differs most:
it is a single dependency-light Rust binary, it speaks both the
OpenAI-compatible and Anthropic APIs directly, it builds dynamic context
pruning into the agent loop instead of requiring a plugin, and it is compatible
with the Claude Code on-disk layout while using its own `.oxide/` format.

## Installation

### Prebuilt binary

#### macOS and Linux

```sh
curl -fsSL https://github.com/jaysonwu991/oxide/releases/latest/download/install.sh | bash
```

The installer detects your OS/arch, downloads the matching release, verifies its
SHA-256 checksum, and installs `oxide` to `~/.local/bin` by default.

Overrides:

| Variable | Purpose |
| --- | --- |
| `OXIDE_VERSION` | Version to install (with or without a leading `v`). Defaults to the latest release. |
| `OXIDE_INSTALL_DIR` | Install directory. Defaults to `$HOME/.local/bin`. |
| `OXIDE_REPO` | GitHub repo slug. Defaults to `jaysonwu991/oxide`. |

#### Windows (x86_64)

```powershell
irm https://github.com/jaysonwu991/oxide/releases/latest/download/install.ps1 | iex
```

The PowerShell installer detects your OS/arch, downloads the matching release,
verifies its SHA-256 checksum, and installs `oxide.exe` to
`%LOCALAPPDATA%\Programs\oxide` by default. The same script also runs on macOS
and Linux under PowerShell (`pwsh`), installing `oxide` to `~/.local/bin`.

Overrides:

| Variable | Purpose |
| --- | --- |
| `OXIDE_VERSION` | Version to install (with or without a leading `v`). Defaults to the latest release. |
| `OXIDE_INSTALL_DIR` | Install directory. Defaults to `%LOCALAPPDATA%\Programs\oxide` on Windows, `$HOME/.local/bin` elsewhere. |
| `OXIDE_REPO` | GitHub repo slug. Defaults to `jaysonwu991/oxide`. |

### Supported platforms

| Platform | Rust target | Archive |
| --- | --- | --- |
| macOS (Apple Silicon) | `aarch64-apple-darwin` | `.tar.gz` |
| macOS (Intel) | `x86_64-apple-darwin` | `.tar.gz` |
| Linux (x86_64) | `x86_64-unknown-linux-gnu` | `.tar.gz` |
| Linux (ARM64) | `aarch64-unknown-linux-gnu` | `.tar.gz` |
| Windows (x86_64) | `x86_64-pc-windows-msvc` | `.zip` |

### From source

Requires a stable Rust toolchain (edition 2021).

```sh
cargo install --path .
```

## Quick start

```sh
# Store a provider API key (interactive)
oxide auth login openai

# Launch the TUI in the current project
oxide
```

Or provide credentials through the environment:

```sh
export OPENAI_API_KEY=sk-...
oxide
```

Non-interactive use:

```sh
oxide -p "summarize this repository"
echo "explain src/agent.rs" | oxide -p
oxide -p "review the diff" --image screenshot.png
```

Manage MCP servers:

```sh
oxide mcp add filesystem npx -y @modelcontextprotocol/server-filesystem .
oxide mcp add --transport http slack https://mcp.slack.com/mcp
oxide mcp list
```

## CLI

```
oxide [OPTIONS] [PROMPT] [COMMAND]
```

| Flag | Description |
| --- | --- |
| `[PROMPT]` | Prompt to run. Providing one implies non-interactive mode. |
| `-m, --model <MODEL>` | Model to use (overrides config). |
| `--provider <PROVIDER>` | Provider name (overrides config). |
| `--agent <AGENT>` | Agent to run, from `.oxide/agents` (or `.claude/agents`). |
| `--mode <MODE>` | Permission mode: `build` (default), `plan` (read-only), or `auto-edit`. |
| `--reasoning <LEVEL>` | Reasoning effort: `auto` (default), `off`, `low`, `medium`, or `high`. |
| `-p, --print` | Print the response and exit instead of launching the TUI. |
| `-c, --continue` | Resume the most recent session for this project. |
| `--resume <ID>` | Resume a specific session by id. |
| `--image <PATH>` | Attach an image or PDF (repeatable). |
| `-C, --cwd <DIR>` | Working directory for the agent. |

Credential management:

```sh
oxide auth login [provider] [--key <KEY>]
oxide auth list
oxide auth logout [provider]
```

Keys are stored in `auth.json` in the oxide config directory (mode `0600`) and
resolved after environment variables and before the config file. You can also
launch `oxide` with no key and run `/connect` inside the TUI to pick a provider
and paste a key; the provider is then saved to `config.json`.

MCP server management:

```sh
oxide mcp list
oxide mcp get <name>
oxide mcp add [--scope project|global] [--transport stdio|http] <name> <command|url> [args...]
oxide mcp add-json [--scope project|global] <name> '<json>'
oxide mcp remove [--scope project|global] <name>
```

`--scope project` (the default) writes `<root>/.oxide/mcp.json`; `--scope global`
writes `~/.oxide/mcp.json`. See
[docs/configuration.md](docs/configuration.md#mcp-servers) for examples.

Remote servers can require OAuth. Slack's MCP server works with the URL alone
(its public client is built in); for other servers add an `oauth` block and
authorize with `oxide mcp auth <name>`. oxide runs the authorization-code flow
with PKCE and refreshes the token automatically. See
[Connecting to the Slack MCP server](docs/configuration.md#connect-to-the-slack-mcp-server)
for a worked example.

## Configuration

oxide reads `config.json` from the platform config directory:

- Linux: `~/.config/oxide/config.json`
- macOS: `~/Library/Application Support/oxide/config.json`
- Windows: `%APPDATA%\oxide\config.json`

```json
{
  "provider": "deepseek",
  "model": "deepseek-chat",
  "base_url": "https://api.deepseek.com/v1",
  "api_key": "",
  "system_prompt": "You are Oxide...",
  "max_tokens": 8192,
  "auto_approve": true,
  "mode": "build",
  "reasoning": "auto"
}
```

`api_key` may be left empty when a key is available via `oxide auth` or the
environment. `auto_approve` controls whether tool calls run without prompting;
when `false`, permission rules that resolve to `ask` are denied in
non-interactive mode.

`mode` selects the agent's permission mode. `build` follows the active agent's
permission rules; `plan` is read-only (workspace mutations and unknown MCP tools
are denied) and instructs the model to produce an implementation plan; `auto-edit`
auto-approves `write_file` and `patch` while other rules still apply. In the TUI
press Shift+Tab to cycle modes; `--mode` and `OXIDE_MODE` set the starting mode.

`reasoning` controls how much reasoning effort oxide requests. `auto` (the
default) turns reasoning on for models known to support it (OpenAI o-series and
`gpt-5`, Anthropic Claude 3.7/4) and off otherwise; `off`, `low`, `medium`, and
`high` force a level. It maps to OpenAI's `reasoning_effort` and Anthropic's
extended-thinking budget (kept below `max_tokens`). In the TUI press Ctrl+R to
cycle levels; `--reasoning` and `OXIDE_REASONING` set the starting level.

### Environment variables

| Variable | Purpose |
| --- | --- |
| `OXIDE_PROVIDER` | Provider name. |
| `OXIDE_MODEL` | Model name. |
| `OXIDE_BASE_URL` | API base URL. |
| `OXIDE_API_KEY` | API key. |
| `OXIDE_MODE` | Permission mode (`build`, `plan`, `auto-edit`). |
| `OXIDE_REASONING` | Reasoning effort (`auto`, `off`, `low`, `medium`, `high`). |
| `OXIDE_TRUNCATION_DIR` | Directory for saved truncated tool output (default `truncated/` in the config dir). |
| `OPENAI_API_KEY` / `OPENAI_BASE_URL` | OpenAI credentials. |
| `DEEPSEEK_API_KEY` / `DEEPSEEK_BASE_URL` | DeepSeek credentials. |
| `ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` | Anthropic credentials. |

### Providers

| Name | API | Default model | Base URL | Key env |
| --- | --- | --- | --- | --- |
| `openai`, `gpt`, `gpt-4`, `gpt-4o` | OpenAI-compatible | `gpt-4o-mini` | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
| `deepseek` | OpenAI-compatible | `deepseek-chat` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` |
| `anthropic` | Anthropic Messages | `claude-3-5-sonnet-latest` | `https://api.anthropic.com/v1` | `ANTHROPIC_API_KEY` |

Any OpenAI-compatible endpoint can be used by setting `provider`, `base_url`,
`model`, and a key.

## Ecosystem

oxide discovers configuration from the project root (the nearest ancestor
containing `.git`, `.oxide`, or `.claude`) and the user's global scope. Project
entries override global entries with the same name, and the Oxide layout
overrides the Claude Code layout.

**Oxide layout**

- `AGENTS.md` — project memory and instructions
- `.oxide/agents/*.md` — subagents (frontmatter: `name`, `description`, `mode`, `permission`)
- `.oxide/commands/*.md` — slash commands (`$ARGUMENTS`, `$1`, `$2`, …; optional `agent` and `subtask` frontmatter)
- `.oxide/skills/*/SKILL.md` — on-demand skills
- `.oxide/plugins/` — JS/TS plugin hooks
- `.oxide/mcp.json` — MCP servers (same schema as `.mcp.json`; manage with `oxide mcp`)
- Global scope: `~/.oxide/`

This repository keeps its own agents, commands, skills, and plugins in
`.oxide/`.

For task-by-task instructions — adding and removing MCP servers, subagents,
slash commands, skills, plugins, and permission rules — see
[docs/configuration.md](docs/configuration.md).

A command's frontmatter can route it: `agent: <name>` runs the command with that
agent's prompt and permissions, and `subtask: true` runs it in an isolated
subagent context (the command's output is reported back to the main
conversation). For example:

```markdown
---
description: Lint the crate and fix every warning.
agent: build
---

Run `cargo clippy --all-targets -- -D warnings` and fix each finding.
```

**Claude Code compatibility**

oxide also reads the Claude Code layout, so existing configurations work as-is:

- `CLAUDE.md`, `CLAUDE.local.md` — project memory
- `.claude/agents/`, `.claude/commands/`, `.claude/skills/`, `.claude/plugins/`
- `.mcp.json` — MCP servers
- Global scope: `~/.claude/`, `~/.claude.json`

Slash commands are expanded from the ecosystem and also include built-ins:
`/help`, `/undo`, `/redo`, `/compact`, and `/connect`.

## Tools

Built-in file and shell tools: `read_file`, `write_file`, `list_dir`, `bash`,
`glob`, `grep`, `patch`, `webfetch`. Agent-level tools: `task`, `skill`,
`memory`, `diagnostics`, and `compress` (when context pruning is enabled).
Connected MCP tools appear as `<server>__<tool>`.

`read_file` returns images and PDFs as viewable attachments, and `write_file`
appends LSP diagnostics for the edited file.

Tool results are capped before they enter the model's context: at most 400 lines
and 8 KB, with individual `read_file` lines trimmed at 2 000 characters. `bash`
keeps the **tail** so the exit code and recent errors survive; other tools keep
the head. When output is dropped, the full text is written under `truncated/` in
the oxide config directory and the result includes the path plus a hint to grep
it or `read_file` it with an offset, so the model can recover detail without
re-running the tool. Set `OXIDE_TRUNCATION_DIR` to change where those files go;
they are retained for 7 days.

When the model requests several tools at once, the ones with no side effects
(`read_file`, `list_dir`, `glob`, `grep`, `webfetch`, `memory`, `skill`,
`diagnostics`) run concurrently; anything that writes to the workspace, spawns a
subagent, or has unknown remote effects stays sequential. Results are recorded in
the model's original call order. `bash` streams stdout and stderr line by line
into the TUI (and to stderr in `-p` mode) before the final combined output.

While the agent is busy, pressing Enter queues the current input as steering
rather than starting a new run; the message is injected into the conversation
before the next model call. A `tool.execute.after` plugin can also request
termination for the batch with `output.terminate = true`.

## Context pruning

oxide prunes the context it sends to the model without ever modifying the
session history. Pruning is controlled by `.oxide/dcp.json` (project) and
`dcp.json` in the oxide config directory (global), with the project file
overriding the global one.

- **`compress` tool** — the model can replace closed, stale spans of the
  conversation with a concise summary. It receives message numbers in periodic
  context reminders and passes one or more `ranges` plus a `summary`. Overlapping
  compressions keep the newest summary.
- **Deduplication** — repeated tool calls with identical arguments keep only the
  most recent output.
- **Purge errors** — errored tool outputs are replaced with a short marker after
  a configurable number of turns.
- **Nudges** — when the estimated context grows large, a reminder with the
  conversation index is injected so the model can compress.

Compression records are stored in the session log, so a resumed session rebuilds
the same pruned view. When pruning is enabled it replaces the legacy automatic
`/compact` pass (the `/compact` command remains available).

```json
{
  "enabled": true,
  "compress": {
    "permission": "allow",
    "minContextLimit": 16000,
    "maxContextLimit": 32000,
    "nudgeFrequency": 5,
    "iterationNudgeThreshold": 15
  },
  "strategies": {
    "deduplication": { "enabled": true },
    "purgeErrors": { "enabled": true, "turns": 4 }
  },
  "protectedTools": [],
  "protectedFilePatterns": []
}
```

Set `"enabled": false` to disable pruning, or `"compress": {"permission":
"deny"}` to disable only the `compress` tool. `protectedTools` and
`protectedFilePatterns` exclude tools and file paths from pruning.

## Data locations

Everything lives under the oxide config directory:

- Credentials: `auth.json`
- MCP OAuth tokens: `mcp-oauth/<server>.json` (mode `0600`)
- Sessions: `sessions/<project>/*.jsonl`
- Snapshots: `snapshots/<project>/` (bare git repo)
- Memory: `memory/`
- Truncated tool output: `truncated/` (retained 7 days; see `OXIDE_TRUNCATION_DIR`)
- Context pruning config: `dcp.json` (global) and `.oxide/dcp.json` (project)

## Development

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for architecture notes and guidelines.

## License

MIT
