# Configuration guide

This guide covers day-to-day configuration: where files live, and how to add or
remove MCP servers, subagents, slash commands, skills, plugins, permissions,
modes, reasoning, memory, and context pruning.

## Scopes and precedence

oxide merges two scopes:

- **Global** — the oxide config directory: `~/.config/oxide` on Linux,
  `~/Library/Application Support/oxide` on macOS, `%APPDATA%\oxide` on Windows.
- **Project** — the nearest ancestor of the working directory containing `.git`,
  `.oxide`, or `.claude`.

Project settings override global settings with the same name. oxide reads its
native `.oxide/` layout and also reads the Claude Code layout (`.claude/`,
`CLAUDE.md`, `.mcp.json`) for compatibility; within a scope, `.oxide/` wins over
`.claude/`.

Provider and credential precedence is: CLI flags > environment variables >
`auth.json` > `config.json` > provider preset.

## MCP servers

MCP servers are declared under `mcpServers` in a JSON file. oxide reads, in
increasing precedence:

1. Global `~/.claude.json`
2. Global `~/.oxide/mcp.json`
3. Project `<root>/.mcp.json`
4. Project `<root>/.oxide/mcp.json`

### Manage from the CLI

`oxide mcp` reads and writes the native files for you — `--scope project`
(the default) targets `<root>/.oxide/mcp.json`, `--scope global` targets
`~/.oxide/mcp.json`:

```sh
oxide mcp list
oxide mcp get filesystem

# stdio server: name, then the command and its arguments
oxide mcp add filesystem npx -y @modelcontextprotocol/server-filesystem /path/to/dir
oxide mcp add --env SOME_TOKEN=secret filesystem npx -y server-filesystem /tmp

# HTTP server
oxide mcp add --transport http remote https://example.com/mcp \
  --header "Authorization=Bearer TOKEN"

# store in the global file instead of the project
oxide mcp add --scope global --transport http remote https://example.com/mcp

# add from a JSON object, then remove
oxide mcp add-json extra '{"command":"uvx","args":["mcp-server"]}'
oxide mcp remove extra
```

Options may appear before or after the server name. `oxide mcp remove` falls
back to every scope when the server is not in the requested one.

### OAuth for remote servers

Remote servers that require OAuth advertise it through MCP metadata discovery.
Add an `oauth` block (Claude Code-compatible); public clients using PKCE need no
secret:

```json
{
  "mcpServers": {
    "remote": {
      "type": "http",
      "url": "https://example.com/mcp",
      "oauth": { "clientId": "YOUR_CLIENT_ID", "callbackPort": 3118 }
    }
  }
}
```

oxide discovers the authorization server from
`/.well-known/oauth-protected-resource`, runs the authorization-code flow with
PKCE (`S256`) on a loopback callback, stores the token under
`mcp-oauth/<server>.json` in the oxide config directory (mode `0600`), and
refreshes it automatically. Run the flow up front with:

```sh
oxide mcp auth <name>
```

`oxide mcp auth` also runs automatically on first use when a terminal is
attached; in non-interactive runs, authorize first. `clientSecret` is optional
and only sent for confidential clients; `scopes` and `redirectUri` override the
discovered defaults.

#### Connect to the Slack MCP server

Slack's MCP server is a remote HTTP server that uses OAuth but does not support
dynamic client registration. oxide has Slack's public PKCE client built in, so
adding the server by URL is enough:

```sh
oxide mcp add --transport http slack https://mcp.slack.com/mcp
oxide mcp auth slack
```

oxide fills in the `oauth` block with the client ID
`1601185624273.8899143856786` and callback port `3118`. To use your own Slack
app instead, pass `--oauth-client-id` (and optionally
`--oauth-client-secret` / `--oauth-scope`) explicitly.

Or add it to `.mcp.json` / `.oxide/mcp.json` directly:

```json
{
  "mcpServers": {
    "slack": {
      "type": "http",
      "url": "https://mcp.slack.com/mcp",
      "oauth": {
        "clientId": "1601185624273.8899143856786",
        "callbackPort": 3118
      }
    }
  }
}
```

`oxide mcp auth slack` opens Slack's consent screen in your browser and waits on
`http://localhost:3118/callback`. Once authorized, the Slack tools appear as
`slack__<tool>`.

### File schema

You can also edit the files directly. The schema is the same in every file.
Add a stdio server:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/dir"],
      "env": { "SOME_TOKEN": "{env:SOME_TOKEN}" }
    }
  }
}
```

Add a remote (HTTP) server:

```json
{
  "mcpServers": {
    "remote": {
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer {env:MCP_TOKEN}" }
    }
  }
}
```

- `env` and `headers` values support `{env:VAR}` interpolation from the
  environment.
- Tools are exposed to the model as `<server>__<tool>`; characters outside
  `A-Za-z0-9_-` are replaced with `_`.
- **Remove** a server with `oxide mcp remove <name>`, or by deleting its entry
  (or the file). There is no per-server enable flag.
- Servers start when a session begins. A server that fails to start or list
  tools is logged to stderr and skipped.
- Restart oxide after editing.

## Subagents

Create `.oxide/agents/<name>.md` (or `.claude/agents/<name>.md`):

```markdown
---
name: rust-reviewer
description: Reviews Rust changes for correctness and style.
mode: subagent
permission:
  write_file: deny
  patch: deny
  bash:
    "cargo *": allow
    "*": ask
---

You review Rust changes. Report findings by severity.
```

- `name` defaults to the file stem; `description` is shown to the model.
- `mode` is `subagent` (default), `primary`, or `all`. Only non-primary agents
  can be spawned as subagents; `primary` agents are selectable with `--agent`.
- `permission` (optional) overrides the default tool permissions (see
  [Permissions](#permissions)).
- Run an agent with `oxide --agent <name>`, through the `task` tool, or via a
  command's `agent:` frontmatter.
- **Remove** an agent by deleting its file.

## Slash commands

Create `.oxide/commands/<name>.md` (or `.claude/commands/<name>.md`):

```markdown
---
description: Lint the crate and fix every warning.
agent: build
subtask: true
---

Run `cargo clippy --all-targets -- -D warnings` and fix each finding.

Focus: $ARGUMENTS
```

- Invoke with `/<name> [args]` in the TUI.
- `$ARGUMENTS` expands to the full argument string; `$1`, `$2`, … expand per
  word.
- `agent: <name>` runs the command as that agent. `subtask: true` runs it in an
  isolated subagent context whose result is reported back to the main
  conversation.
- Built-in commands: `/undo`, `/redo`, `/compact`, `/connect`.
- **Remove** a command by deleting its file.

## Skills

Create `.oxide/skills/<name>/SKILL.md` (or `.claude/skills/<name>/SKILL.md`):

```markdown
---
name: release-checklist
description: Steps to cut and publish a release.
---

Detailed instructions loaded on demand.
```

- The `description` is listed in the system prompt; the model loads the full
  skill with the `skill` tool when the task matches.
- **Remove** a skill by deleting its directory.

## Plugins and hooks

Place a `.ts` or `.js` file in `.oxide/plugins/` (or `.claude/plugins/`). oxide
runs it under `bun` or `node`, whichever is found first. Export a function
(default or named) that returns hook handlers:

```ts
export const RustFmt = async ({ $, directory }) => {
  return {
    "tool.execute.before": async (input, output) => {
      // input.tool, input.args — mutate output.args to change the call
    },
    "tool.execute.after": async (input, output) => {
      // input.tool, input.args — mutate output.output to change the result
      // output.terminate = true — end the turn once this batch finishes
    },
  }
}
```

- `tool.execute.before` runs before a tool; mutate `output.args` to alter the
  arguments.
- `tool.execute.after` runs after a tool; mutate `output.output` to alter the
  result text.
- Set `output.terminate = true` in `tool.execute.after` to skip the follow-up
  model call. The turn ends only when every tool result in the batch terminates.
- The `$` helper runs shell commands (`await $\`cmd\`.cwd(dir).quiet().nothrow()`).
- **Remove** a plugin by deleting its file.

## Permissions

Default actions: `read_file`, `list_dir`, `glob`, `grep`, and `webfetch` are
allowed; `write_file`, `patch`, and `bash` ask; everything else is allowed.

Override per agent with `permission` in the agent frontmatter. Use a flat action
for every tool:

```yaml
permission: allow
```

Or per-tool rules, with optional command/path patterns for `bash` and file
tools:

```yaml
permission:
  write_file: allow
  patch: deny
  bash:
    "cargo *": allow
    "git *": allow
    "*": ask
```

- Actions are `allow`, `ask`, and `deny`.
- Tool keys are real tool names (`read_file`, `write_file`, `patch`, `bash`,
  `list_dir`, `glob`, `grep`, `webfetch`).
- Patterns match the `bash` command or a file tool's `path`; `*` and `?` are
  wildcards. The last matching rule wins.
- `auto_approve: true` in `config.json` skips prompts for `ask` rules. When
  `false`, `ask` is denied in non-interactive (`-p`) mode.
- The active [mode](#modes) is applied after the rules: `plan` denies workspace
  mutations, `auto-edit` approves file edits.

## Modes

The agent runs in one of three permission modes, modelled on Claude Code:

| Mode | Behavior |
| --- | --- |
| `build` (default) | Follows the active agent's permission rules. |
| `plan` | Read-only: `write_file`, `patch`, `bash`, and unknown MCP tools are denied, and the model is instructed to produce an implementation plan. |
| `auto-edit` | Auto-approves `write_file` and `patch`; other rules still apply. |

Set the starting mode with `--mode build|plan|auto-edit`, the `OXIDE_MODE`
environment variable, or `"mode": "..."` in `config.json`. In the TUI, press
Shift+Tab to cycle build → auto-edit → plan; the current mode is shown in the
header. Plan mode keeps read-only tools available and is useful for review and
planning before switching back to build.

## Reasoning

Reasoning effort controls how much internal reasoning oxide asks the model to
spend before answering:

| Level | Behavior |
| --- | --- |
| `auto` (default) | Enables reasoning for models known to support it (OpenAI o-series and `gpt-5`, Anthropic Claude 3.7/4) and turns it off otherwise. Resolves to `medium` when supported. |
| `off` | Never request reasoning. |
| `low` / `medium` / `high` | Force that level of effort. |

The level maps to OpenAI's `reasoning_effort` parameter and to Anthropic's
extended-thinking `budget_tokens` (scaled by level and kept below `max_tokens`).
Thinking blocks returned by Anthropic are replayed on later turns so multi-step
tool use keeps its reasoning context.

Set the starting level with `--reasoning auto|off|low|medium|high`, the
`OXIDE_REASONING` environment variable, or `"reasoning": "..."` in `config.json`.
In the TUI, press Ctrl+R to cycle auto → off → low → medium → high; the current
level is shown in the header.

## Memory and instructions

These files are added to the system prompt:

- Project: `AGENTS.md`, and `CLAUDE.md` / `CLAUDE.local.md` for compatibility.
- Global: `~/.oxide/AGENTS.md`, and `~/.claude/CLAUDE.md` for compatibility.

Persistent cross-session memory is managed by the `memory` tool and stored under
`memory/` in the oxide config dir; recent entries are injected automatically.
Use the `memory` tool to add, search, or forget entries.

## Context pruning

Context pruning is configured by `.oxide/dcp.json` (project) and `dcp.json` in
the oxide config dir (global), with the project file overriding the global one.
See [Context pruning](../README.md#context-pruning) in the README for the
options and an example.

## Providers and credentials

Provider, model, base URL, and API key are read from `config.json`, environment
variables, and `auth.json`, in the precedence order above. There are two ways to
connect a provider.

### From the CLI

```sh
oxide auth login openai
oxide auth login deepseek --key sk-...
```

Without arguments, `oxide auth login` prompts for the provider (a name or its
1-based number) and reads the API key with hidden input; bracketed paste works.
The key is stored in `auth.json` (mode `0600`) and the provider is written to
`config.json` so the next launch uses it. Manage stored keys with
`oxide auth list` and `oxide auth logout [provider]`.

### From the TUI

Start `oxide` even without a key, then run `/connect`:

- `/connect` lists the providers — enter a number or name, then paste the API key.
- `/connect deepseek` skips the picker and asks for the key directly.

Press Enter to confirm and Esc to cancel. The key is stored in `auth.json` and
the active provider is written to `config.json`, so it applies to the running
session and the next launch.

You can also provide a key without either flow via the `OPENAI_API_KEY` /
`DEEPSEEK_API_KEY` / `ANTHROPIC_API_KEY` environment variables or an `api_key`
entry in `config.json`; environment variables take precedence over `auth.json`.

See [Configuration](../README.md#configuration) and
[Providers](../README.md#providers) in the README for the full list.

## Data locations and reset

Everything lives under the oxide config directory:

- `config.json` — provider and behavior settings
- `auth.json` — stored API keys (mode `0600`)
- `mcp-oauth/<server>.json` — OAuth tokens for remote MCP servers (mode `0600`)
- `sessions/<project>/*.jsonl` — session history and pruning records
- `snapshots/<project>/` — shadow-git snapshots for `/undo` and `/redo`
- `memory/` — persistent memory entries
- `dcp.json` — global context-pruning config

Deleting a session file removes that conversation; deleting `snapshots/`
removes undo history; deleting `auth.json` logs you out.
