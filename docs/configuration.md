# Configuration guide

This guide covers day-to-day configuration: where files live, and how to add or
remove MCP servers, subagents, slash commands, skills, plugins, permissions,
memory, and context pruning.

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
- Built-in commands: `/undo`, `/redo`, `/compact`.
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
    },
  }
}
```

- `tool.execute.before` runs before a tool; mutate `output.args` to alter the
  arguments.
- `tool.execute.after` runs after a tool; mutate `output.output` to alter the
  result text.
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
variables, and `auth.json`, in the precedence order above. Store a key with:

```sh
oxide auth login openai
```

See [Configuration](../README.md#configuration) and
[Providers](../README.md#providers) in the README for the full list.

## Data locations and reset

Everything lives under the oxide config directory:

- `config.json` — provider and behavior settings
- `auth.json` — stored API keys (mode `0600`)
- `sessions/<project>/*.jsonl` — session history and pruning records
- `snapshots/<project>/` — shadow-git snapshots for `/undo` and `/redo`
- `memory/` — persistent memory entries
- `dcp.json` — global context-pruning config

Deleting a session file removes that conversation; deleting `snapshots/`
removes undo history; deleting `auth.json` logs you out.
