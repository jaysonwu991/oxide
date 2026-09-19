# oxide

A native Rust AI coding agent CLI for the terminal. oxide streams from
OpenAI-compatible and Anthropic models, runs a tool-using agent loop against
your project, and understands its own `.oxide/` configuration layout out of the
box, with Claude Code configuration support for compatibility.

## Features

- Interactive TUI (ratatui) plus non-interactive `-p/--print`, `--mode json`
  (JSONL event stream), and `--mode rpc` (JSONL over stdin/stdout) modes. In the
  TUI, `/login` (`/connect`) and `/logout` manage provider credentials.
- OpenAI-compatible (OpenAI, DeepSeek, Portkey, custom) and Anthropic Messages API clients.
- Built-in tools under Pi-style names: `read`, `write`, `edit`, `bash`, `grep`,
  `find`, `ls`, `webfetch`. Compatibility names (`read_file`, `write_file`,
  `list_dir`, `glob`) and the unified-diff `patch` tool are accepted everywhere,
  including in permission rules.
- Agent-level tools: `task` (subagents), `skill` (on-demand skill loading),
  `memory` (cross-session notes), `diagnostics` (LSP diagnostics), and
  `compress` (when context pruning is enabled).
- MCP servers over stdio or Streamable HTTP, loaded on demand with automatic
  tool selection, OAuth discovery, and session handling, exposed as
  `<server>__<tool>`.
- Multimodal prompts: attach images/PDFs with `--image` or `@path` references,
  and pass prompt files as `oxide @file "message"`.
- Project + global ecosystem discovery: instructions, commands, prompt
  templates, agents, skills, MCP servers, and plugins from `.oxide/` (plus the
  Claude Code layout).
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
  typing while the agent works steers it between steps. Enter queues a steering
  message while busy; Alt+Enter queues a follow-up delivered after all work
  finishes.
- Agent modes: `build` (default), `plan` (read-only planning), and `auto-edit`
  (auto-approve file edits), cycled in the TUI with Shift+Tab or set with
  `--mode` / `OXIDE_MODE`.
- Reasoning effort: `auto` (default), `off`, `low`, `medium`, or `high`, cycled
  in the TUI with Ctrl+R or set with `--reasoning` / `OXIDE_REASONING`. `auto`
  uses the provider/model's native behavior; explicit levels map to
  OpenAI-compatible effort, Anthropic adaptive thinking, or legacy extended
  thinking as appropriate.
- Compact, bounded output: tool bodies are hidden in the TUI by default (Ctrl+O
  toggles them), long action lines are truncated to the terminal width, and tool
  results are capped by lines and bytes before they enter the model's context.
  Capped output is saved to disk with a pointer so it stays recoverable.
- Compact agent transcript: shell calls render as `→ Run <command>` and finish
  as `→ Ran <command> · exit <code>`. Hidden output uses a
  `⋯ <lines> lines · Ctrl+O to expand` affordance, file edits show a colored
  line-numbered diff, and each model turn is timed with `+ Thought: Nms`.
- Tool selection: `--tools`/`-t` allowlists and `--exclude-tools`/`-x`
  disables tools (accepting both Pi and legacy names); disabled tools are hidden
  from the model and refused if requested.
- Pi-style session flags: `--session <path|id>`, `--no-session`, `--name`,
  `-c`/`--continue`, `-r`/`--resume` (browse past sessions), and
  `--fork <path|id>`, plus TUI commands `/new`, `/session`, `/resume`, `/name`,
  `/model`, `/thinking`, `/export`, `/reload`, and `/hotkeys`.
- Session branching: `/tree` lists user messages, `/fork <n>` branches a new
  session from one, and `/clone` duplicates the current session.
- Project trust: project-local resources (agents, commands, prompts, skills,
  plugins, `SYSTEM.md`) load only after the project is trusted; decisions are
  saved per directory in `trust.json`, `defaultProjectTrust` sets the fallback,
  `--approve`/`-a` and `--no-approve` override for one run, and `/trust` saves a
  decision.
- Themes: built-in `dark` and `light` plus custom `.oxide/themes/<name>.json`,
  selected with `--use-theme` or `/theme`.
- Focused terminal layout: onboarding shortcuts live in the welcome tips,
  current activity and elapsed time live in the status row, and the footer
  shows the working directory, session name, token totals (`↑`/`↓`), context
  usage, model, mode, and thinking level. The labeled editor grows to 12 rows,
  and semantic colors keep dark, light, and custom themes consistent.

## Comparison

oxide is a small, native terminal agent that deliberately borrows the Claude
Code configuration layout so existing `.claude/` setups keep working. The table
below compares the high-level shape of the three tools; feature sets move fast,
so check each project's documentation for the current details.

| Capability | oxide | [OpenCode](https://opencode.ai) | [Claude Code](https://docs.claude.com/en/docs/claude-code/overview) |
| --- | --- | --- | --- |
| Distribution | Native Rust binary | Open-source CLI (Node/Bun) | Proprietary CLI + apps |
| License | MIT | Open source | Proprietary |
| Model providers | OpenAI-compatible + Anthropic (OpenAI, DeepSeek, Portkey, custom) | Any provider (bring your own keys) | Claude (Anthropic API, Bedrock, Vertex, third-party) |
| Interfaces | Terminal TUI, `-p` print, JSON/RPC modes | Terminal, desktop, IDE, web | Terminal, IDE, desktop, web |
| Project config | `.oxide/` + `AGENTS.md` (also reads `.claude/`) | `opencode.json` + `AGENTS.md` | `CLAUDE.md` + `.claude/` |
| Subagents | `--agent`, `task`, command routing | Agents | Subagents, background agents |
| Permission modes | `build` / `plan` / `auto-edit` (Shift+Tab, `--mode`) | Build / plan agents | default / accept-edits / plan / bypass |
| Reasoning effort | `auto` / `off` / `low` / `medium` / `high` (Ctrl+R, `--reasoning`) | Model-dependent | Extended thinking |
| Slash commands | `.oxide/commands` + `.oxide/prompts`, `agent`/`subtask` routing | Commands | Commands |
| Skills | `SKILL.md` | Agent Skills | Skills |
| MCP servers | stdio + HTTP + OAuth, managed with `oxide mcp` | MCP servers | MCP servers |
| Plugins / hooks | JS/TS hooks (bun/node) | Plugins | Hooks, plugins, Agent SDK |
| LSP diagnostics | Built in (rust-analyzer, TS, pyright, gopls) | Built in (LSP servers) | — |
| Undo file changes | Shadow-git `/undo`, `/redo` | `/undo`, `/redo` | Git / checkpoints |
| Sessions | Durable JSONL, `-c` / `-r`, `/resume` / `/tree` / `/fork` / `/clone` | Sessions, share links | Sessions across surfaces |
| Project trust | `trust.json`, `--approve` / `/trust` | — | — |
| Themes | Built-in `dark` / `light`, custom `.oxide/themes` | Themes | — |
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
# Launch the TUI in the current project
oxide

# Then connect a provider from inside the TUI
/login
```

### Keyboard shortcuts

The welcome area summarizes the loaded ecosystem and shows the most commonly
used shortcuts above the editor. The two-row footer combines live state such as
`ready`, `thinking`, tool activity, and elapsed time with the current project,
Git branch, model, mode, and thinking level.

| Key | Action |
| --- | --- |
| Enter | Send a message; while the agent is busy, queue guidance for its next step. |
| Shift+Enter | Insert a newline without sending. |
| Alt+Enter | While busy, queue a follow-up to run after the current work finishes. |
| Esc | Clear the input; with empty input, quit. In dialogs, cancel or close. |
| `/` | Open slash-command autocomplete. |
| Tab | Complete the selected slash command. |
| Up / Down | Recall input history or move through a picker. |
| Shift+Tab | Cycle `build` → `auto-edit` → `plan`. |
| Ctrl+R | Cycle the thinking level: `auto` → `off` → `low` → `medium` → `high`. |
| Ctrl+O | Expand or collapse tool-output details. |
| Ctrl+V | Attach an image from the clipboard when the platform helper is available. |
| PgUp / PgDn / mouse wheel | Scroll the transcript. |
| Ctrl+A / Ctrl+E | Jump to the start or end of the message box. |
| Ctrl+Y / Ctrl+E | Scroll one line (Ctrl+E only when the message box is empty). |
| Ctrl+U / Ctrl+D | Scroll half a page. |
| Ctrl+G / Home | Scroll to the top. |
| End | Return to the latest message and resume automatic scrolling. |
| Ctrl+C | Quit. |

Run `/hotkeys` for the in-app list and `/help` for commands, agents, and skills.

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
oxide mcp add --transport http atlassian https://mcp.atlassian.com/v1/mcp
oxide mcp list
```

## CLI

```
oxide [OPTIONS] [@files...] [PROMPT...]
oxide mcp <COMMAND>
oxide sessions <COMMAND>
oxide uninstall [--keep-config] [--keep-data] [--dry-run] [--force]
```

| Flag | Description |
| --- | --- |
| `[PROMPT]...` | Prompt words. `@path` reads a file into the prompt (images/PDFs become attachments). Providing one implies non-interactive mode. |
| `-m, --model <MODEL>` | Model to use (overrides config). |
| `--provider <PROVIDER>` | Provider name (overrides config). |
| `--agent <AGENT>` | Agent to run, from `.oxide/agents` (or `.claude/agents`). |
| `--mode <MODE>` | Permission mode `build` (default), `plan` (read-only), `auto-edit`, or output mode `print`, `json`, or `rpc`. |
| `--reasoning <LEVEL>` | Reasoning effort: `auto` (default), `off`, `low`, `medium`, or `high`. |
| `--system-prompt <TEXT>` | Replace the default system prompt for this run. |
| `--append-system-prompt <TEXT>` | Append text to the system prompt (repeatable). |
| `--no-context-files` | Disable `AGENTS.md`/`CLAUDE.md` context-file discovery. |
| `-t, --tools <LIST>` | Allowlist tools (comma-separated, Pi or legacy names). |
| `-x, --exclude-tools <LIST>` | Disable tools (comma-separated). |
| `-a, --approve` | Trust project-local resources for this run. |
| `--no-approve` | Ignore project-local resources for this run. |
| `--use-theme <NAME>` | TUI theme (`dark`, `light`, or a custom `.oxide/themes` file). |
| `--session <PATH\|ID>` | Use a specific session file or id. |
| `-n, --name <NAME>` | Set the session display name at startup. |
| `--no-session` | Ephemeral mode: do not save the session. |
| `-p, --print` | Print the response and exit instead of launching the TUI. |
| `-c, --continue` | Resume the most recent session for this project. |
| `-r, --resume` | Browse and select a past session to resume. |
| `--fork <PATH\|ID>` | Fork a session file or id into a new session. |
| `--image <PATH>` | Attach an image or PDF (repeatable). |
| `-C, --cwd <DIR>` | Working directory for the agent. |

Non-interactive examples:

```sh
oxide -p "summarize this repository"
oxide @prompt.md "answer this"          # include a file in the prompt
cat README.md | oxide -p "summarize"    # merge piped stdin
oxide --mode json "list files"           # JSONL events on stdout
oxide --mode rpc                         # JSONL prompts over stdin
oxide -t read,grep,find -p "review"      # read-only tool allowlist
oxide --session <id> -p "continue"       # reuse a specific session
oxide --fork <id> -p "try another path"  # branch a saved session
```

Manage saved sessions (list, clean up stale ones, compact, and merge):

```sh
oxide sessions list                        # current project
oxide sessions list --all                  # every project
oxide sessions list --older-than 30        # stale sessions only
oxide sessions delete <id>                 # delete one (prompts)
oxide sessions delete --older-than 30 --force
oxide sessions compact <id>                # summarize older history, keep recent tail
oxide sessions compact --all               # refresh every session in this project
oxide sessions merge <a> <b>               # concatenate two sessions into a new one
oxide sessions merge <a> <b> --summarize   # summarize the second session first
```

`delete` is token-free. `compact` and `merge --summarize` make one LLM
summarization pass per session so a stale session resumes from a small summary
plus its most recent messages instead of replaying the full transcript.

Uninstall Oxide and its related files:

```sh
oxide uninstall --dry-run                 # preview removals
oxide uninstall                           # preview, confirm, and uninstall
oxide uninstall --keep-config --keep-data # retain configuration and user data
oxide uninstall --force                   # skip confirmation
```

Package-manager installations are removed through Cargo or Homebrew when
detected. Prebuilt installations print the final command needed to remove the
currently running executable after cleanup completes.

Credential management happens inside the TUI with the Pi-style commands:

```text
/login [provider]    connect a provider and store its API key
/logout [provider]   remove stored credentials
```

`/login` opens a provider picker (`/login <provider>` skips straight to the key).
Keys are stored in `auth.json` in the oxide config directory (mode `0600`) and
resolved after environment variables and before the config file. The active
provider is written to `config.json` so the next launch uses it.

MCP server management:

```sh
oxide mcp list
oxide mcp get <name>
oxide mcp add [--scope project|global] [--transport stdio|http] <name> <command|url> [args...]
oxide mcp add-json [--scope project|global] <name> '<json>'
oxide mcp remove [--scope project|global] <name>
oxide mcp auth [--scope project|global] <name>
```

`--scope project` (the default) writes `<root>/.oxide/mcp.json`; `--scope global`
writes `~/.oxide/mcp.json`. See
[docs/configuration.md](docs/configuration.md#mcp-servers) for examples.

Remote servers can require OAuth. Add the server by URL; oxide detects a `401`
authentication challenge, discovers the authorization server, runs the
authorization-code flow with PKCE, and refreshes the token automatically. You
can also authorize up front with `oxide mcp auth <name>`. See
[Connecting to the Atlassian Rovo MCP server](docs/configuration.md#connect-to-the-atlassian-rovo-mcp-server)
for a worked example.

Configured servers are connected lazily. The model sees a compact `mcp_load`
discovery tool at startup and loads the matching server automatically when a
prompt names its service or contains one of its URLs. OAuth and full tool-schema
discovery therefore happen only when that server is first needed.

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
  "reasoning": "auto",
  "theme": "dark"
}
```

`api_key` may be left empty when a key is available via `/login` or the
environment. `auto_approve` controls whether tool calls run without prompting;
when `false`, permission rules that resolve to `ask` are denied in
non-interactive mode.

`mode` selects the agent's permission mode. `build` follows the active agent's
permission rules; `plan` is read-only (workspace mutations and all MCP tools
are denied) and instructs the model to produce an implementation plan;
`auto-edit` auto-approves `write`, `edit`, and `patch` while other rules still
apply. In the TUI press Shift+Tab to cycle modes; `--mode` and `OXIDE_MODE` set
the starting mode.

`reasoning` controls how much reasoning effort oxide requests. `auto` (the
default) leaves reasoning behavior and effort to the provider/model. Newer
Claude models use adaptive thinking without a forced effort; other APIs receive
no effort override. `off`, `low`, `medium`, and `high` force a level using
OpenAI-compatible `reasoning_effort`, Anthropic adaptive thinking with
`output_config.effort`, or a legacy Anthropic thinking budget as appropriate.
In the TUI press Ctrl+R to cycle levels; `--reasoning` and `OXIDE_REASONING` set
the starting level.

### Environment variables

| Variable | Purpose |
| --- | --- |
| `OXIDE_PROVIDER` | Provider name. |
| `OXIDE_MODEL` | Model name. |
| `OXIDE_BASE_URL` | API base URL. |
| `OXIDE_API_KEY` | API key. |
| `OXIDE_MODE` | Permission mode (`build`, `plan`, `auto-edit`). |
| `OXIDE_REASONING` | Reasoning effort (`auto`, `off`, `low`, `medium`, `high`). |
| `OXIDE_CONTEXT_LIMIT` | Model context window in tokens, used for the footer's context percentage (default 128000). |
| `OXIDE_TRUNCATION_DIR` | Directory for saved truncated tool output (default `truncated/` in the config dir). |
| `OPENAI_API_KEY` / `OPENAI_BASE_URL` | OpenAI credentials. |
| `DEEPSEEK_API_KEY` / `DEEPSEEK_BASE_URL` | DeepSeek credentials. |
| `ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` | Anthropic credentials. |
| `PORTKEY_API_KEY` / `PORTKEY_BASE_URL` | Portkey AI Gateway credentials. |
| `PORTKEY_CONFIG` | Optional Portkey config ID sent as `x-portkey-config`. |
| `PORTKEY_MODELS` | Optional comma-separated model catalog for keys that cannot call `/models`. |

### Providers

| Name | API | Default model | Base URL | Key env |
| --- | --- | --- | --- | --- |
| `openai`, `gpt`, `gpt-4`, `gpt-4o` | OpenAI-compatible | `gpt-4o-mini` | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
| `deepseek` | OpenAI-compatible | `deepseek-chat` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` |
| `anthropic` | Anthropic Messages | `claude-3-5-sonnet-latest` | `https://api.anthropic.com/v1` | `ANTHROPIC_API_KEY` |
| `portkey`, `port-key` | OpenAI-compatible gateway | `claude-sonnet-5` | `https://api.portkey.ai/v1` | `PORTKEY_API_KEY` |

Any OpenAI-compatible endpoint can be used by setting `provider`, `base_url`,
`model`, and a key.

### Portkey

Run `/login portkey` in the TUI, or set `PORTKEY_API_KEY`, then select a model
with `/models` or `"model"` in `config.json`. The preset uses
`https://api.portkey.ai/v1`, sends the key as `x-portkey-api-key`, and defaults
to `claude-sonnet-5`.

For custom gateways, Config IDs, environment precedence, and model-catalog
fallbacks, see the full [Portkey configuration](docs/configuration.md#portkey)
section.

## Context files and system prompt

oxide loads `AGENTS.md` (or `CLAUDE.md`) as project instructions by walking
every ancestor directory from the filesystem root down to the working
directory, so nested projects layer their instructions. If a directory contains
`AGENTS.override.md`, it replaces `AGENTS.md`/`CLAUDE.md` for that directory
only. The global `~/.oxide/AGENTS.md` is loaded first (lowest precedence).

- Disable ancestor context-file discovery with `--no-context-files`.
- Replace the default system prompt with `.oxide/SYSTEM.md` (project),
  `~/.oxide/SYSTEM.md`, or the corresponding platform config-directory file.
  Append without replacing with `APPEND_SYSTEM.md` in the same locations.
- `--system-prompt <text>` replaces the configured base prompt for one run, and
  `--append-system-prompt <text>` appends to that base (repeatable). A loaded
  `SYSTEM.md` remains the higher-precedence replacement.
- The startup welcome area lists loaded context files, and `/reload` re-reads them.

## Ecosystem

oxide discovers configuration from the project root (the nearest ancestor
containing `.git`, `.oxide`, or `.claude`) and the user's global scope. Project
entries override global entries with the same name, and the Oxide layout
overrides the Claude Code layout.

**Oxide layout**

- `AGENTS.md` — project memory and instructions
- `.oxide/AGENTS.md` — additional layout-scoped instructions
- `.oxide/agents/*.md` — subagents (frontmatter: `name`, `description`, `mode`, `permission`)
- `.oxide/commands/*.md` — slash commands (`$ARGUMENTS`, `$1`, `$2`, …; optional `agent` and `subtask` frontmatter)
- `.oxide/prompts/*.md` — prompt templates (Pi-style; frontmatter `description` and `argument-hint`, arguments `$1`, `$@`, `${1:-default}`, `${@:2:3}`)
- `.oxide/skills/*/SKILL.md` — on-demand skills
- `.oxide/themes/*.json` — TUI color themes (built-in `dark`/`light` plus custom)
- `.oxide/plugins/` — JS/TS plugin hooks
- `.oxide/SYSTEM.md`, `.oxide/APPEND_SYSTEM.md` — replace or extend the system prompt
- `.oxide/mcp.json` — MCP servers (same schema as `.mcp.json`; manage with `oxide mcp`)
- Global scope: `~/.oxide/` and the platform oxide config directory (the latter
  has higher precedence)

This repository keeps its own agents, commands, prompts, skills, and plugins in
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

- `CLAUDE.md` — project memory and instructions
- `.claude/CLAUDE.md` — additional layout-scoped instructions
- `.claude/agents/`, `.claude/commands/`, `.claude/skills/`, `.claude/plugins/`
- `.mcp.json` — MCP servers
- Global scope: `~/.claude/`, `~/.claude.json`

Slash commands are expanded from the ecosystem and also include built-ins:
`/help`, `/hotkeys`, `/new`, `/session`, `/resume`, `/tree`, `/fork`, `/clone`, `/name`,
`/model`, `/thinking`, `/theme`, `/trust`, `/export`, `/reload`, `/init`,
`/login`, `/logout`, `/models`, `/mcps`, `/connect`, `/undo`, `/redo`, and
`/compact`.

## Tools

Built-in file and shell tools use Pi-style names: `read`, `write`, `edit`,
`bash`, `grep`, `find`, `ls`, and `webfetch`. Compatibility names
`read_file`, `write_file`, `list_dir`, and `glob` remain accepted; `patch` is
the unified-diff editing tool. Agent-level tools: `task`, `skill`, `memory`,
`diagnostics`, and `compress` (when context pruning is enabled). `mcp_load`
reveals a configured server on demand; its tools then appear as
`<server>__<tool>`.

| Tool | Parameters |
| --- | --- |
| `read` | `path`, `offset?` (1-based), `limit?` (default 250 lines) |
| `write` | `path`, `content` |
| `edit` | `path`, `edits: [{ oldText, newText }]` |
| `bash` | `command`, `timeout?` in milliseconds (default 120000) |
| `grep` | `pattern`, `path?`, `glob?`, `ignoreCase?`, `context?`, `limit?` |
| `find` | `pattern`, `path?`, `limit?` |
| `ls` | `path?`, `limit?` |
| `patch` | `diff` (unified diff) |
| `webfetch` | `url`, `format?` (`markdown` default, `text`, or raw `html`) |

`edit` performs exact text replacement: each `oldText` is matched against the
original file (never incrementally) and must be unique, so a non-unique or
missing match is rejected rather than silently corrupting a file. Line endings
and a leading BOM are preserved. `write` and `edit` append LSP diagnostics for
the edited file. `find` and `grep` respect `.gitignore`. `webfetch` converts
HTML to Markdown (`format: "markdown"`, the default), readable plain text
(`format: "text"`), or returns the raw body (`format: "html"`); the converter
is dependency-free and handles headings, paragraphs, lists, tables, links,
images, inline and fenced code, blockquotes, and HTML entities.

Tool results are capped before they enter the model's context. The default cap
is 250 lines and 6 KB (per-tool overrides: `bash` 160 lines / 5 KB, `grep`,
`find`, and `ls` 160 / 4 KB, `webfetch` 200 / 6 KB, and `write`, `edit`, and
`patch` 120 / 3 KB), with individual `read` lines trimmed at 1 000 characters.
`bash` keeps the **tail** so the exit code and recent errors survive; other
tools keep the head. When output is dropped, the full text is written under
`truncated/` in the oxide config directory and the result includes the path plus
a hint to grep it or `read` it with an offset, so the model can recover detail
without re-running the tool. Set `OXIDE_TRUNCATION_DIR` to change where those
files go; they are retained for 7 days.

When the model requests several tools at once, the ones with no side effects
(`read`, `ls`, `find`, `grep`, `webfetch`, `memory`, `skill`, `diagnostics`) run
concurrently; anything that writes to the workspace, spawns a subagent, or has
unknown remote effects stays sequential. Results are recorded in the model's
original call order. `bash` streams stdout and stderr line by line into the TUI
(and to stderr in `-p` mode) before the final combined output.

While the agent is busy, pressing Enter queues the current input as steering
rather than starting a new run; the message is injected into the conversation
before the next model call. A `tool.execute.after` plugin can also request
termination for the batch with `output.terminate = true`.

## Project trust

Projects may contain local resources that change how the agent behaves or
execute code — agents, commands, prompts, skills, plugins, `SYSTEM.md`, and
`APPEND_SYSTEM.md`.
oxide treats the presence of any of these as requiring trust. When a project
requires trust and no decision has been saved for it (or a parent directory),
the TUI asks before loading them.

- `defaultProjectTrust` in `settings.json` controls the fallback: `ask`
  (default), `always`, or `never`.
- `--approve`/`-a` trusts project resources for one run; `--no-approve` ignores
  them.
- `/trust [show|off]` saves a decision for the current directory to `trust.json`.
- Non-interactive modes (`-p`, `--mode json`, `--mode rpc`) never prompt: with
  the `ask` or `never` setting they ignore project resources unless approved.
- Context files (`AGENTS.md`/`CLAUDE.md`) always load, trusted or not.

## Themes

oxide ships `dark` and `light` themes. Add a custom theme as JSON under
`.oxide/themes/<name>.json` (project) or `<config>/oxide/themes/<name>.json`
(global), then select it with `--use-theme <name>` or `/theme <name>`. Colors
accept names (`cyan`, `lightblue`) or `#rrggbb`; unspecified slots fall back to
the built-in `dark` theme:

```json
{
  "accent": "#5fd7ff",
  "success": "lightgreen",
  "tool": "cyan",
  "error": "lightred",
  "info": "gray",
  "border": "#5fd7ff"
}
```

Available slots: `accent`, `user`, `assistant`, `success`, `tool`, `error`,
`info`, `dim`, `border`, `thinking_off`, `thinking_low`, `thinking_medium`,
`thinking_high`.

Theme slots are semantic: `accent` marks focus and selections, `user` and
`assistant` label speakers, `success` and `error` communicate outcomes, `tool`
marks active tool work, and `info`/`dim` render supporting text. Selection rows
also use reverse video and outcomes include text or symbols, so meaning does not
depend on color alone. For accessible custom themes, keep every foreground
readable against the terminal background and avoid assigning the same color to
`success`, `error`, and `tool`.

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
    "iterationNudgeThreshold": 15,
    "protectedTools": ["task", "skill", "memory", "diagnostics"]
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
"deny"}` to disable only the `compress` tool. Top-level `protectedTools` and
`protectedFilePatterns` protect matching results from deduplication and error
purging. `compress.protectedTools` retains matching tool outputs alongside a
model-written compression summary.

## Data locations

Runtime state lives under the platform oxide config directory:

- Main configuration: `config.json`
- Credentials: `auth.json`
- Cached provider model lists: `model-cache.json` (refreshed after 24 hours)
- MCP OAuth tokens: `mcp-oauth/<server>.json` (mode `0600`)
- Sessions: `sessions/<project>/*.jsonl`
- Session names: `sessions/<project>/<id>.name`
- Snapshots: `snapshots/<project>/` (bare git repo)
- Memory: `memory/`
- Project trust: `trust.json`
- Settings: `settings.json` (e.g. `defaultProjectTrust`)
- Themes: `themes/<name>.json`
- Truncated tool output: `truncated/` (retained 7 days; see `OXIDE_TRUNCATION_DIR`)
- Context pruning config: `dcp.json` (global) and `.oxide/dcp.json` (project)

Global ecosystem resources such as agents, commands, prompts, skills, plugins,
and MCP definitions may also live under `~/.oxide/`; compatibility resources
are read from `~/.claude/` and `~/.claude.json`.

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
