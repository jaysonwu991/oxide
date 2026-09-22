# Configuration guide

This guide covers day-to-day configuration: where files live, and how to add or
remove MCP servers, subagents, slash commands, prompt templates, skills, plugins,
permissions, modes, reasoning, memory, project trust, themes, and context
compaction.

## Scopes and precedence

oxide merges two scopes:

- **Global** — native resources can live in either `~/.oxide/` or the platform
  oxide config directory (`~/.config/oxide` on Linux,
  `~/Library/Application Support/oxide` on macOS, and `%APPDATA%\oxide` on
  Windows). Claude Code-compatible resources come from `~/.claude/` and
  `~/.claude.json`.
- **Project** — the nearest ancestor of the working directory containing `.git`,
  `.oxide`, or `.claude`.

Project entries override global entries with the same name (for agents,
commands, prompt templates, skills, and MCP servers). oxide reads its native
`.oxide/` layout and also reads the Claude Code layout (`.claude/`, `CLAUDE.md`,
`.mcp.json`) for compatibility. Native Oxide entries override Claude-compatible
entries within the global or project scope. If both native global locations
contain the same entry, the platform config directory wins over `~/.oxide/`.

Provider and model CLI flags take precedence over `OXIDE_*` environment
variables, then `config.json` and provider presets. For API keys, the order is
`OXIDE_API_KEY`, the selected provider's key variable, `auth.json`, and
`config.json`; OpenAI-compatible providers also accept `OPENAI_API_KEY` as a
last fallback. `OXIDE_BASE_URL` overrides the selected provider's base-URL
variable, which overrides the file. Behavior settings live in `config.json`
(global); the global `settings.json` (and the project `.oxide/settings.json`)
supply `defaultProjectTrust`, `compaction`, `modelPrices`, `hideThinkingBlock`,
`notifyOnComplete`, and `notifySound`.

Installed plugin packages (see [Plugins and hooks](#plugins-and-hooks)) load
after global resources and before project resources, so project entries still
override plugins with the same name.

## MCP servers

MCP servers are declared under `mcpServers` in a JSON file. oxide reads, in
increasing precedence:

1. Global `~/.claude.json`
2. Global `~/.oxide/mcp.json`
3. Global `<platform-config>/oxide/mcp.json`
4. Project `<root>/.mcp.json`
5. Project `<root>/.oxide/mcp.json`

The `oxide mcp` management commands only write the native files:
`<root>/.oxide/mcp.json` for `--scope project` and `~/.oxide/mcp.json` for
`--scope global`. The Claude Code and platform-config files listed above are read
(for `list`, `get`, `auth`, and `remove`), never written.

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

# with explicit routing domains (repeatable or comma-separated)
oxide mcp add --transport http docs https://mcp.example.com \
  --domains docs.example.com --domains '*.example.com'

# store in the global file instead of the project
oxide mcp add --scope global --transport http remote https://example.com/mcp

# add from a JSON object, then remove
oxide mcp add-json extra '{"command":"uvx","args":["mcp-server"]}'
oxide mcp remove extra
```

`oxide mcp list` checks every configured server in parallel and reports
`Connected`, `Needs Auth`, `Needs Trust`, `Disabled`, or the connection error.
Status checks never open a browser or execute untrusted project servers; use
`oxide mcp auth <name>` for a server that needs OAuth. The TUI `/mcps` command
performs the same check.

Options may appear before or after the server name. `oxide mcp auth` and
`oxide mcp remove` treat `--scope` as a boundary rather than just a file: a
pinned `--scope project` searches `<root>/.oxide/mcp.json` then
`<root>/.mcp.json`, `--scope global` searches the three global files
highest-precedence first, and neither touches the other scope — so a name
defined in both project and global resolves to the requested one. Without
`--scope`, both search every configured source in precedence order, so
`oxide mcp remove <name>` still deletes the entry the runtime actually uses.

At startup, oxide adds only the enabled server names and configured URLs or
commands to the model context. It does not start a local process, make a remote
request, or check OAuth until a matching server is needed. A server is selected
two ways:

- **URL routing** — when a user message contains a URL whose host matches a
  server's routing domains, oxide loads that server before the next model call
  so its tools are ready on the first turn. `webfetch` also redirects to the
  matching server instead of making an unauthenticated request.
- **`mcp_load`** — the model loads a server on demand through the built-in
  `mcp_load` tool, whose description lists each server's domains.

The server's tools are discovered when loaded and become available on the next
agent step for the rest of the session.

Routing domains come from the optional `domains` array in a server's config,
falling back to built-in presets for well-known servers (Atlassian/Jira/Confluence,
New Relic, Context7, Contentful, Figma, GitHub, GitLab, Notion, Linear, and
Sentry). Exact hosts match exactly; a `*.` prefix (or leading `.`) matches the
host and its subdomains. For example:

```json
{
  "mcpServers": {
    "docs": {
      "type": "http",
      "url": "https://mcp.example.com",
      "domains": ["docs.example.com", "*.example.com"]
    }
  }
}
```

This behavior applies to every configured MCP server, not a hard-coded list of
services. Remote servers that establish a Streamable HTTP session have their
`Mcp-Session-Id` preserved across subsequent requests.

### OAuth for remote servers

Remote servers that require OAuth advertise it through MCP metadata discovery.
Add the server by URL; no `oauth` block is required:

```json
{
  "mcpServers": {
    "remote": {
      "type": "http",
      "url": "https://example.com/mcp"
    }
  }
}
```

On a `401 Unauthorized` response, oxide reads the `WWW-Authenticate` challenge
and falls back to the standard `/.well-known/oauth-protected-resource` URLs. It
then discovers the authorization server, dynamically registers a client when
supported, runs the authorization-code flow with PKCE (`S256`) on a loopback
callback, stores the token under
`mcp-oauth/<server>.json` in the oxide config directory (mode `0600`), and
refreshes it automatically. Run the flow up front with:

```sh
oxide mcp auth <name>
```

`oxide mcp auth` also runs automatically on first use when a terminal is
attached; in non-interactive runs, authorize first. An optional Claude
Code-compatible `oauth` block can provide a pre-registered `clientId`,
`clientSecret`, `callbackPort`, `scopes`, or `redirectUri` when the authorization
server does not support dynamic client registration or needs overrides.

#### Connect to the Atlassian Rovo MCP server

Atlassian's remote MCP server exposes Jira, Confluence, and Compass tools over
Streamable HTTP. Adding the server by URL is enough:

```sh
oxide mcp add --transport http atlassian https://mcp.atlassian.com/v1/mcp
```

On the first prompt that needs Atlassian, oxide loads the server, follows its
OAuth discovery metadata, opens the consent screen, and dynamically registers
the client. To authorize before starting oxide, run `oxide mcp auth atlassian`.

The equivalent `.mcp.json` / `.oxide/mcp.json` entry is:

```json
{
  "mcpServers": {
    "atlassian": {
      "type": "http",
      "url": "https://mcp.atlassian.com/v1/mcp"
    }
  }
}
```

Once authorized, the tools appear as `atlassian__<tool>`.

#### Connect to the Context7 MCP server

Context7's own setup command (`npx ctx7 setup --oxide` or `npx @upstash/context7-mcp@latest --setup --oxide`) does not
recognize oxide, since it only writes config files for editors it knows about.
Add the server directly with the CLI instead:

```sh
oxide mcp add --transport http context7 https://mcp.context7.com/mcp/oauth
```

Or add the equivalent entry to `.mcp.json` / `.oxide/mcp.json` by hand:

```json
{
  "mcpServers": {
    "context7": {
      "type": "http",
      "url": "https://mcp.context7.com/mcp/oauth"
    }
  }
}
```

On the first prompt that needs Context7, oxide loads the server, follows its
OAuth discovery metadata, and opens the consent screen. To authorize before
starting oxide, run:

```sh
oxide mcp auth context7
```

Once authorized, the tools appear as `context7__<tool>`.

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
- Set `"enabled": false` (or `"disabled": true`) to keep a server in the file
  without loading it. Disabled servers are omitted from the model context.
- Tools are exposed to the model as `<server>__<tool>`; characters outside
  `A-Za-z0-9_-` are replaced with `_`.
- **Remove** a server with `oxide mcp remove <name>`, or by deleting its entry
  (or the file).
- Enabled servers start lazily when first needed (URL routing or `mcp_load`). A
  server that fails to start or list tools is logged to stderr and skipped.
- Restart oxide after editing.

## Subagents

Create `.oxide/agents/<name>.md` (or `.claude/agents/<name>.md`):

```markdown
---
name: rust-reviewer
description: Reviews Rust changes for correctness and style.
mode: subagent
permission:
  write: deny
  edit: deny
  bash:
    "cargo *": allow
    "*": ask
---

You review Rust changes. Report findings by severity.
```

- `name` defaults to the file stem; `description` is shown to the model.
- `mode` is `subagent` (default), `primary`, or `all`. Only `subagent` and `all`
  agents can be spawned through `task`; `--agent` can select any discovered
  agent. The `task` tool is only offered when at least one such agent exists.
- Subagents can spawn subagents one level deep: `task` is available at depths 0
  and 1 and omitted afterwards (`MAX_TASK_DEPTH`), which bounds a runaway tree.
- `permission` (optional) overrides the default tool permissions (see
  [Permissions](#permissions)).
- Run an agent with `oxide --agent <name>`, through the `task` tool, or via a
  command's `agent:` frontmatter.
- A running `task` call reports what the subagent is doing: its panel gains a
  live `Elapsed` timer and a `↳ <agent> · <activity> · <n> call(s)` line that
  follows each tool call the subagent makes, and the status row names the
  subagent and its current tool. Without it a long review is indistinguishable
  from a hang.
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
- The agent can also invoke a command on its own with the `command` tool when
  your request matches the command's description; commands whose frontmatter
  sets `subtask: true` run in an isolated subagent.
- `$ARGUMENTS` expands to the full argument string; `$1`, `$2`, … expand per
  word.
- `agent: <name>` runs the command as that agent. `subtask: true` runs it in an
  isolated subagent context whose result is reported back to the main
  conversation.
- Built-in commands: `/help`, `/hotkeys`, `/exit`, `/new`, `/session`, `/resume`,
  `/tree`, `/fork`, `/clone`, `/name`, `/model`, `/thinking`, `/theme`,
  `/trust`, `/export`, `/reload`, `/init`, `/login`, `/logout`, `/models`,
  `/mcps`, `/plugins`, `/marketplaces`, `/notify`, `/usage`, `/connect`, `/undo`,
  `/redo`, `/compact`, `/copy`, `/copy all`, and `/skill:<name>`. After a space,
  the built-in commands autocomplete their fixed arguments too (`/notify sound`,
  `/usage currency usd`, `/plugins marketplace update`, and providers for
  `/login`); Tab accepts the highlighted suggestion.
- **Remove** a command by deleting its file.

## Prompt templates

Create `.oxide/prompts/<name>.md` (or `.claude/prompts/<name>.md`):

```markdown
---
description: Create a component
argument-hint: <name> [features]
---
Create a component named $1 with features: $@
```

- Invoke with `/<name> [args]`; the filename becomes the command name.
- `description` is optional (the first non-empty line is used when absent), and
  `argument-hint` shows the expected arguments in autocomplete.
- Arguments: `$1`, `$2`, … positional; `$@` or `$ARGUMENTS` for all;
  `${1:-default}` and `${@:-default}` for defaults; `${@:2}` and `${@:2:3}` for
  slices.
- Prompt templates resolve through the same `/` path as commands; a command with
  the same name wins.
- **Remove** a template by deleting its file.

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
- Force-load a skill with `/skill:<name> [args]`; extra arguments are appended
  as `User: <args>`. Skills appear in the `/` autocomplete.
- **Remove** a skill by deleting its directory.

## Plugins and hooks

### Hook plugins (single files)

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
- `status` runs after each completed turn; write strings into
  `output.statuses` (a keyed object) to show them on the footer's third row.
- The `$` helper runs shell commands (`await $\`cmd\`.cwd(dir).quiet().nothrow()`).
- **Remove** a plugin by deleting its file.

### Plugin packages and marketplaces

oxide also supports Claude Code-style plugin packages: directories with a
`.claude-plugin/plugin.json` manifest that bundle slash commands, subagents,
skills, MCP servers, and command hooks. The manifest may also live at
`.oxide/plugin.json` (preferred when both are present).

```jsonc
// .claude-plugin/plugin.json
{
  "name": "my-plugin",
  "version": "1.0.0",
  "description": "What it does",
  "mcpServers": { "fs": { "command": "npx", "args": ["-y", "server-fs"] } },
  "hooks": {
    "PostToolUse": [
      { "matcher": "Write|Edit", "hooks": [{ "type": "command", "command": "fmt" }] }
    ]
  }
}
```

Layout inside a plugin package:

- `commands/*.md` — slash commands
- `agents/*.md` — subagents
- `skills/<name>/SKILL.md` — skills
- `plugins/*.js|ts` — JS/TS hook plugins
- `mcpServers` in the manifest — MCP servers
- `.mcp.json` — MCP servers as a top-level `mcpServers` map, or the server
  entries directly (the map form Claude Code plugins commonly use)
- `hooks` in the manifest — Claude Code command hooks (`PreToolUse`/
  `PostToolUse`, with `matcher` regexes). They run through the hook host, which
  pipes tool info as JSON on stdin and reads a Claude Code-style JSON response:
  `PreToolUse` applies `updatedInput`; `PostToolUse` appends
  `additionalContext` and honors `decision: "block"`.

A *marketplace* is a directory or git repository with a
`.claude-plugin/marketplace.json` (or `.oxide/marketplace.json`, preferred)
manifest:

```jsonc
{
  "name": "my-marketplace",
  "plugins": [
    { "name": "my-plugin", "source": "https://github.com/you/my-plugin.git" }
  ]
}
```

#### Installing and managing plugins

Install and manage plugins from the CLI or the TUI:

```
oxide plugin marketplace add <url|path|owner/repo>
oxide plugin marketplace list
oxide plugin marketplace update <name>
oxide plugin marketplace remove <name>
oxide plugin install <name>[@marketplace]
oxide plugin list
oxide plugin enable|disable <name>[@marketplace]
oxide plugin uninstall <name>[@marketplace]
```

`add` accepts a git URL, a local directory, or GitHub's `owner/repo` shorthand
(expanded to `https://github.com/owner/repo.git`). Git marketplaces are cloned
under `<config>/oxide/plugins/marketplaces/<name>/`; a local directory is
recorded in place, so edits to it are live. `update` fast-forwards a git
marketplace to its remote head and reports the plugin count (local directories
report that there is nothing to fetch).

Each `plugins` entry names a plugin and points at its source. Three forms are
accepted:

```jsonc
{ "name": "git-plugin", "source": "https://github.com/you/plugin.git" }
{ "name": "local-plugin", "source": "./plugin" }   // relative to the marketplace
{ "name": "object-plugin", "source": { "source": "github", "repo": "you/plugin" } }
```

`install <name>@<marketplace>` disambiguates when several marketplaces offer the
same name; a bare `<name>` searches every configured marketplace. `uninstall`,
`enable` and `disable` accept the same `[@marketplace]` suffix (and `remove` is
an alias for `uninstall`), so names can be copy-pasted from `plugin list`
output. A `@marketplace` that does not match the installed plugin's marketplace
is rejected.

Installed plugins are copied under
`<config>/oxide/plugins/<marketplace>/<plugin>/`, so uninstalling a plugin or
removing its marketplace never touches the upstream source.

In the TUI, `/plugins` lists installed plugins — marketplace, version,
description, and path per entry — and accepts
`/plugins install <name>[@marketplace]`, `/plugins uninstall <name>`,
`/plugins enable|disable <name>`, and `/plugins marketplace
<list|add <url|path|owner/repo>|update <name>|remove <name>>`. `/marketplaces`
opens an interactive browser: the left pane lists marketplaces, the right pane
shows the selected marketplace's plugins and their install state, `Enter`
installs or enables/disables a plugin, `Ctrl+U` fetches the selected
marketplace's latest manifest, `Ctrl+A` adds a marketplace, `Ctrl+X` removes
one, and `Ctrl+R` reloads the local view. Typing filters the focused pane:
plugin names match first, and a plugin's description is only searched when no
name matches, so a query like `doc` stays on `doc-mcp` rather than listing
every plugin that mentions "documentation".

Installed plugins live under `<config>/oxide/plugins/` (next to `auth.json` and
`trust.json`), with their state in `plugins/config.json`. Their commands,
agents, skills, and MCP servers load at startup before project resources, so
project-local entries still override plugins with the same name. After
installing, `/reload` picks up new commands, agents, and skills; hooks and MCP
servers require a restart.

## Permissions

Default actions: `read`, `ls`, `find`, `grep`, and `webfetch` are allowed;
`write`, `edit`, `patch`, and `bash` ask; everything else is allowed.

Override per agent with `permission` in the agent frontmatter. Use a flat action
for every tool:

```yaml
permission: allow
```

Or per-tool rules, with optional command/path patterns for `bash` and file
tools:

```yaml
permission:
  write: allow
  edit: deny
  bash:
    "cargo *": allow
    "git *": allow
    "*": ask
```

- Actions are `allow`, `ask`, and `deny`.
- Tool keys accept Pi names (`read`, `write`, `edit`, `bash`, `ls`, `find`,
  `grep`, `webfetch`) and legacy aliases (`read_file`, `write_file`, `patch`,
  `list_dir`, `glob`).
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
| `plan` | Read-only: `write`, `edit`, `patch`, `bash`, and all MCP tools are denied, and the model is instructed to produce an implementation plan. |
| `auto-edit` | Auto-approves `write`, `edit`, and `patch`; other rules still apply. |

Set the starting mode with `--mode build|plan|auto-edit`, the `OXIDE_MODE`
environment variable, or `"mode": "..."` in `config.json`. In the TUI, press
Shift+Tab to cycle build → auto-edit → plan; cycling shows the current mode in
the editor status row. Plan mode keeps read-only tools available and is useful
for review and planning before switching back to build.

## Reasoning

Reasoning effort controls how much internal reasoning oxide asks the model to
spend before answering:

| Level | Behavior |
| --- | --- |
| `auto` (default) | Uses provider-native behavior. Newer Claude models use adaptive thinking; other providers receive no forced effort. |
| `off` | Never request reasoning. |
| `low` / `medium` / `high` | Force that level of effort. |

Explicit levels map to OpenAI-compatible `reasoning_effort`, newer Anthropic
adaptive thinking with `output_config.effort`, or legacy Anthropic
extended-thinking `budget_tokens` (scaled by level and kept below `max_tokens`).
Thinking blocks returned by Anthropic are replayed on later turns so multi-step
tool use keeps its reasoning context.

GLM is the exception: Z.AI selects thinking with `thinking.type` and accepts only
its own `reasoning_effort` levels (`low`, `high`, and `max` on the GLM-5.3
series). `auto` leaves the provider default, `off` sends
`thinking.type = "disabled"`, `low` → `low`, `medium` → `high`, and `high` →
`max`. GLM-5.3 always thinks, so `off` there asks for the lowest effort instead
of an unsupported `disabled`.

Set the starting level with `--reasoning auto|off|low|medium|high`, the
`OXIDE_REASONING` environment variable, or `"reasoning": "..."` in `config.json`.
In the TUI, press Ctrl+R to cycle auto → off → low → medium → high; the current
level is shown in the footer (for models that support reasoning) and colors the
composer rules.

### Reasoning in the transcript

Reasoning that a provider streams (`reasoning_content`, `reasoning`, or
`reasoning_text` on OpenAI-compatible APIs, `thinking` blocks on Anthropic) is
shown in place, before the answer it produced:

```
✦ Thought for 1.4s
  The failing test reads a file that moved, so the fix belongs in the loader.
```

The block streams as `✦ Thinking` and picks up its duration once the model moves
on. Press Ctrl+T to collapse reasoning to its label
(`✦ Thought for 1.4s · Ctrl+T to expand`) and again to expand it;
`hideThinkingBlock` in the global `settings.json` makes oxide start collapsed,
matching Pi. Reasoning is stored in the session as thinking blocks and, for
Anthropic, replayed on later turns; OpenAI-compatible requests strip it. While a
turn is in flight the status row names the phase (`thinking...`,
`running tool...`, `compacting...`, `summarizing branch...`) and reports
transient stream failures as `retrying (1/3) in 1s...` before the client backs
off and tries again. A response that arrives with neither text nor tool calls is
retried on the same schedule, so an occasional empty reply does not end the
turn; only after the retries are exhausted is it reported as an error.

### Status tips and queued messages

That status row only exists while the agent is busy, so anything you do at rest
reports itself as a dim line in the transcript instead — `copied 243 chars` after
you drag over text, `tool output collapsed` after Ctrl+O, `restored 2 queued
messages to the editor` after a dequeue. Only the newest tip is kept, so repeated
actions update one line rather than growing the transcript, matching Pi's
`showStatus`.

While the agent is busy, Enter queues guidance for its next step and Alt+Enter
queues a follow-up to run once the work finishes. Both show up as your turns, and
the status row adds `2 queued · Option+Up to edit`. Pressing that key empties the
queues back into the message box — queued text first, whatever you were already
typing after it — so you can extend a message before it is sent. Their entries are
also removed from the transcript, since they were never sent. The key is `Alt+Up`
(`Option+Up` on macOS, where `Alt` is the Option key) and `Alt+Q` on Windows and
WSL, where the terminal claims `Alt+Up` for scrollback.

## Desktop notifications

When an agent turn finishes, oxide raises a system toast (Notification Center on
macOS, `notify-send` on Linux, a Windows toast) whose body is a short snippet of
the reply, so you can switch windows while a long task runs. Only real agent
turns notify — internal work such as `/compact` and branch summaries stays
silent.

Both the toast and its alert sound are on by default. In the TUI, `/notify`
shows the current state, `/notify on|off` toggles the toast, `/notify sound
on|off` toggles the alert sound, and `/notify test` sends a sample; the choice is
saved to the global `settings.json`. The same keys (`notifyOnComplete` and
`notifySound`) can be edited by hand in the global `settings.json` or the
project `.oxide/settings.json`, with `OXIDE_NOTIFY_ON_COMPLETE` and
`OXIDE_NOTIFY_SOUND` overriding them for one run. The sound uses the native
alert — `Glass` on macOS, the freedesktop `complete` sound (via
`canberra-gtk-play`, `paplay`, `aplay`, or `ffplay`) on Linux, and
`Notification.Default` on Windows. Notification delivery is best-effort: if the
platform helper or sound player is unavailable it is skipped without affecting
the turn.

## Memory and instructions

Context files are collected by walking every ancestor directory from the
filesystem root down to the working directory, so nested projects layer their
instructions:

- `AGENTS.md` (or `CLAUDE.md`) in each directory.
- `AGENTS.override.md` replaces `AGENTS.md`/`CLAUDE.md` for that directory only.
- Global `~/.oxide/AGENTS.md` is loaded first (lowest precedence).
- Disable ancestor context-file discovery with `--no-context-files`. This does
  not disable layout-scoped `.oxide/AGENTS.md` or `.claude/CLAUDE.md` files.

Replace the default system prompt with `.oxide/SYSTEM.md` (project),
`~/.oxide/SYSTEM.md`, or the corresponding platform config-directory file;
append without replacing with `APPEND_SYSTEM.md` in the same locations.
`--system-prompt <text>` and `--append-system-prompt <text>` change the
configured base for one run, but a loaded `SYSTEM.md` remains the
higher-precedence replacement.

Persistent cross-session memory is managed by the `memory` tool and stored under
`memory/` in the oxide config dir; the 8 most recent entries are injected into
the system prompt automatically. Use the `memory` tool to add, search, or forget
entries.

## Project trust

Project-local resources that can change behavior or execute code (agents,
commands, prompts, skills, plugins, `SYSTEM.md`) load only after the project is
trusted. On interactive startup oxide asks when a project requires trust and no
decision is saved; non-interactive runs use `defaultProjectTrust` (in
`settings.json`) without prompting.

- `defaultProjectTrust`: `ask` (default), `always`, or `never`.
- `--approve`/`-a` and `--no-approve` override for one run.
- `/trust [show|off]` saves a decision for the current directory to `trust.json`
  (the closest saved decision on the current or a parent path applies).
- Context files always load regardless of trust.

## Themes

oxide ships `dark` and `light`. Add custom themes as JSON under
`.oxide/themes/<name>.json` or `<config>/oxide/themes/<name>.json`, then select
one with `--use-theme <name>` or `/theme <name>`. Colors accept names or
`#rrggbb`; unset slots fall back to the built-in `dark` theme.

```json
{
  "accent": "#5fd7ff",
  "user": "lightcyan",
  "assistant": "white",
  "success": "lightgreen",
  "tool": "lightyellow",
  "error": "lightred",
  "info": "gray",
  "border": "#5fd7ff"
}
```

Available slots are `accent`, `user`, `assistant`, `success`, `tool`, `error`,
`info`, `dim`, `border`, `tool_pending_bg`, `tool_success_bg`, `tool_error_bg`,
`usage_bar_bg`, `usage_bar_fg`, `usage_bar_label`, `thinking_off`,
`thinking_low`, `thinking_medium`, `thinking_high`, and `thinking_text`. The
`thinking_text` slot colors the `✦ Thinking` label and the reasoning body.
The transcript, dialogs,
autocomplete, status row, input, and footer use these semantic roles.
`tool_*_bg` fill the background behind a tool's header, body, and
`Took`/`Elapsed` footer (pending while running, success or error once it
settles), and `usage_bar_*` paint the [Portkey spend bar](#portkey-usage-bar).
`/theme` lists available themes; after a switch, oxide immediately rebuilds the
styled transcript.

Selections use reverse video and outcome rows retain words or symbols, so color
is not the only state cue. When authoring a custom theme, choose foregrounds
with strong contrast against the terminal background and keep `success`,
`error`, and `tool` visually distinct.

For the complete keyboard guide, see
[Keyboard shortcuts](../README.md#keyboard-shortcuts).

## Sessions and context

Sessions use Pi's on-disk format: an append-only JSONL tree whose first line is
a `session` header and whose remaining lines are `message`, `compaction`,
`branch_summary`, `session_info`, `model_change`, and `thinking_level_change`
entries linked by `id`/`parentId`. The last entry is the active leaf, and the
model sees the leaf path with the latest compaction applied.

## Context compaction

Oxide compacts the conversation Pi-style: once the outgoing context approaches
the model window, older turns are replaced with a structured summary while the
most recent tokens stay verbatim. A `compaction` entry anchored at
`firstKeptEntryId` is stored on the session branch, so resuming a session
rebuilds the same compacted view.

Configure it under `compaction` in `settings.json` (global) or
`.oxide/settings.json` (project), which override each other:

```json
{
  "compaction": {
    "enabled": true,
    "reserveTokens": 16384,
    "keepRecentTokens": 20000,
    "modelOverrides": {
      "openai/gpt-4o": { "reserveTokens": 400000 }
    }
  }
}
```

- `reserveTokens` — tokens reserved for the model response; compaction triggers
  above `contextWindow - reserveTokens`.
- `keepRecentTokens` — recent tokens kept verbatim.
- `modelOverrides` — per `provider/model` budget overrides; omitted fields fall
  back to the ordinary settings.

`OXIDE_COMPACTION_ENABLED`, `OXIDE_COMPACTION_RESERVE_TOKENS`, and
`OXIDE_COMPACTION_KEEP_RECENT_TOKENS` override the file settings, and
`OXIDE_CONTEXT_LIMIT` sets the model window. Manual compaction is available with
`/compact [focus]` in the TUI or `oxide sessions compact`.

Branching with `/tree <n>` or `/fork <n>` summarizes the abandoned branch with
the same structured format and appends it as a `branch_summary` entry.
`/tree <n>` branches the current session in place, keeping alternatives in the
same file; `/fork <n>` creates a new session seeded with the summary.

## Model prices

The footer's `$cost` segment uses a built-in price table (USD per million
tokens) overlaid by a `modelPrices` map in `settings.json` (global) or
`.oxide/settings.json` (project):

```json
{
  "modelPrices": {
    "my-model": { "input": 1.0, "output": 4.0, "cacheRead": 0.1, "cacheWrite": 1.25 }
  }
}
```

Models without a price cost 0 and the segment is omitted.

## Providers and credentials

Provider, model, base URL, and API key are read from `config.json`, environment
variables, and `auth.json`, in the precedence order above. Authentication happens
inside the TUI, like Pi.

### From the TUI

Start `oxide` even without a key, then run `/login`:

- `/login` lists the providers — enter a number or name, then paste the API key
  (Enter with the field empty reuses the stored key when there is one). Providers
  with a stored key are marked `connected`.
- `/login deepseek` or `/login portkey` skips the picker. When that provider is
  already connected the command switches to it; otherwise it asks for the key.
- `/logout` removes the active provider's stored credential and switches to
  another logged-in provider when one is left; `/logout <provider>` removes a
  specific one without disturbing the active session.

Press Enter to confirm and Esc to cancel. The key is stored in `auth.json`
(mode `0600`) and the active provider is written to `config.json`, so it applies
to the running session and the next launch. `/connect` remains an alias of
`/login`.

### Several providers at once

Credentials for different providers live side by side in `auth.json`, so you can
log in to OpenAI, Anthropic, and Portkey and move between them without re-entering
a key. Switching happens by:

- `/login <provider>` for a provider that is already connected.
- Enter in the login dialog's key step, which reuses the stored key — type a key
  first to replace it instead.
- Picking a model of another provider in `/models` (see below).
- `--provider <name>` on the command line, for one run.

Each provider keeps the model it was last used with in the `provider_models` map
in `config.json`, so switching back restores that choice instead of carrying the
other provider's model id:

```json
{
  "provider": "anthropic",
  "model": "claude-sonnet-5",
  "provider_models": { "openai": "gpt-4o-mini", "anthropic": "claude-sonnet-5" }
}
```

A custom (non-preset) provider has no remembered endpoint: the `base_url` in
`config.json` is global, so switching away from a custom endpoint and back keeps
the preset's URL. Provide the endpoint again, or run with `OXIDE_BASE_URL`.

You can also provide a key without the login flow via the `OPENAI_API_KEY` /
`DEEPSEEK_API_KEY` / `ANTHROPIC_API_KEY` / `PORTKEY_API_KEY` / `ZAI_API_KEY`
environment variables or an `api_key` entry in `config.json`; environment
variables take precedence over `auth.json`.

See [Configuration](../README.md#configuration) and
[Providers](../README.md#providers) in the README for the full list.

### Z.AI (GLM)

Z.AI serves the GLM models over the OpenAI-compatible Chat Completions API, so
the provider only differs in its endpoint and defaults:

| Setting | Value |
| --- | --- |
| Names | `zai`, `glm`, `z.ai`, `z-ai`, `zhipu`, `bigmodel` |
| Base URL | `https://api.z.ai/api/paas/v4` (`ZAI_BASE_URL`) |
| Default model | `glm-5.3` (`glm-5.3-flash` is cheaper and faster) |
| Key | `ZAI_API_KEY`, or `/login glm` |

```text
/login glm
```

Keys are created at <https://z.ai/manage-apikey/apikey-list>. The preset is
international; for the mainland-China BigModel endpoint, set `ZAI_BASE_URL` or
`base_url` to `https://open.bigmodel.cn/api/paas/v4` (a `zhipu` key does not work
against the international host, and vice versa). A custom provider whose
`base_url` points at either Z.AI host is treated as Z.AI too, so the GLM request
shape and bundled catalog still apply.

Z.AI documents no model listing endpoint, so `/models` falls back to a bundled
list of current GLM models when the request is refused or not found, and always
includes the active model. Prices for the GLM models ship in the built-in price
table, so the footer's `$cost` works without extra configuration. Choose a model
with `/models` or a `"model"` entry:

```json
{
  "provider": "zai",
  "model": "glm-5.3-flash"
}
```

Thinking is controlled by `thinking.type` rather than `reasoning_effort`; see
[Reasoning](#reasoning) for how `/thinking` maps onto GLM's levels.

### Portkey

Portkey uses the OpenAI-compatible Chat Completions API. In both setups below,
keep the API key out of `config.json`: `/login portkey` stores it in
`auth.json` with mode `0600` and selects Portkey as the active provider.

#### Login without a custom gateway

Start Oxide and run:

```text
/login portkey
```

Paste your Portkey API key when prompted. For a new setup, no other
configuration is required: Oxide uses `https://api.portkey.ai/v1` and
`claude-sonnet-5` by default. If you previously configured a custom gateway,
remove its `base_url` before using the default endpoint. To use a model from
the Portkey Model Catalog, set its identifier in `config.json`:

```json
{
  "provider": "portkey",
  "model": "@provider-slug/model-name"
}
```

The global file is `~/Library/Application Support/oxide/config.json` on macOS,
`~/.config/oxide/config.json` on Linux, and `%APPDATA%\oxide\config.json` on
Windows.

#### Login with a custom gateway

First add the gateway and routing settings to the global `config.json`:

```json
{
  "provider": "portkey",
  "model": "account-model",
  "base_url": "https://gateway.example.com/v1",
  "portkey_config": "pc-example",
  "model_catalog": ["account-model", "fallback-model"]
}
```

After saving the file, start Oxide and run `/login portkey`. If Oxide is already
running, save the file, run `/login portkey`, then run `/reload`. Paste the API
key for that gateway when prompted. Oxide sends the key as
`x-portkey-api-key` and the Config ID as `x-portkey-config`; it does not send
the key as a bearer token.

`portkey_config` is optional when the gateway does not require a saved Portkey
Config. `model_catalog` is also optional, but is useful when the gateway blocks
`GET <base_url>/models` or exposes account-specific model names. The active
`model` is always included in the model picker.

The same setup can be supplied with environment variables instead of storing
the gateway settings in the file:

```sh
PORTKEY_API_KEY=... \
PORTKEY_BASE_URL=https://gateway.example.com/v1 \
PORTKEY_CONFIG=pc-example \
PORTKEY_MODELS=account-model,fallback-model \
oxide --provider portkey --model account-model
```

`PORTKEY_API_KEY` can also be used for the default setup. `OXIDE_API_KEY` has
higher precedence; if neither is set and no stored or configured Portkey key
exists, Oxide accepts `OPENAI_API_KEY` as the generic OpenAI-compatible
fallback. `PORTKEY_BASE_URL` overrides `base_url`, `OXIDE_BASE_URL` overrides
both, `PORTKEY_CONFIG` overrides `portkey_config`, and `PORTKEY_MODELS`
overrides `model_catalog` with a comma-separated list.

For routing, fallbacks, retries, or other behavior through Portkey's standard
endpoint, set a saved Config ID without changing `base_url`:

```json
{
  "provider": "portkey",
  "model": "claude-sonnet-5",
  "portkey_config": "pc-example"
}
```

`/models` normally requests `<base_url>/models` and uses the ids the provider
reports, so the picker never offers a model the provider does not expose. The
active model is added only when the provider returns an empty list, or for an
endpoint Oxide does not recognize, so a stale or hand-typed id does not linger
in the picker. If `model_catalog` is nonempty, Oxide uses it verbatim without
making that request. When a Portkey `/models` request returns HTTP 403, Oxide
falls back to its bundled Portkey list, again including the active model. Set
`model_catalog`, or a comma-separated `PORTKEY_MODELS` override, for restricted
or account-specific catalogs. Unknown model IDs are passed through unchanged.

With several providers logged in, the picker lists the catalogs of all of them
at once, each row tagged with its provider (`gpt-4o-mini [openai]`), and the same
model id can appear once per provider. Selecting a model from another provider
switches to it and remembers the choice for that provider. Providers whose
catalog cannot be fetched are reported in the transcript while the others stay
usable; custom endpoints are skipped because their base URL is not stored per
provider.

Refreshing the credential with `/login portkey` preserves an existing Portkey
model, custom base URL, and Config ID when Portkey is already active. See
Portkey's documentation for the current [gateway headers](https://portkey.ai/docs/api-reference/inference-api/headers)
and [OpenAI-compatible setup](https://portkey.ai/docs/integrations/libraries/openai-compatible).

## Portkey usage bar

When your models are routed through Portkey, oxide can show a spend bar on the
last line of the TUI:

```text
→ firstname.lastname | Session: $0.00 | Today: $20.61 | Month: $220.69 / $600.00
```

`Session` is the cumulative cost of the current session (the same figure as the
footer's `$cost`). `Today` and `Month` come from the Portkey analytics API, so
they cover every request your workspace attributed to that user from any
client, not just this session, and they are the USD amounts Portkey reports. The
optional ` / $600.00` after `Month` is the budget configured with
`/usage budget`, formatted in the currency set with `/usage currency`.

The bar only runs while a Portkey provider is logged in, because the spend it
shows belongs to that account: `/login portkey` (or `PORTKEY_API_KEY`) has to be
in place before `/usage on` and before the bar appears again at startup.

### Configure and toggle

Run `/usage` to open the Portkey spend-bar dialog. It is a small form over the
bar's settings: move with `↑`/`↓`, press `Enter` to toggle `Enabled` and
`Currency` or to edit a text field in place (the cursor sits at the end of the
value; an API key is masked), and `Esc` to save and close. The rows are:

```text
Enabled    show or hide the bar
User       the user whose spend to show (firstname.lastname)
Metadata   metadata key holding the user (default `_user`)
Budget     monthly budget shown after the month spend
Currency   budget currency (`usd`/`cny`)
API key    usage API key (empty uses the provider key)
Endpoint   analytics base URL (default https://api.portkey.ai/v1)
```

The same settings can still be changed one at a time from the command line:

```text
/usage status                 show the status and syntax
/usage on | off               show or hide the bar
/usage user <firstname.lastname>   the user whose spend to show
/usage key <pk-...>           usage API key (or `off` to use the provider key)
/usage budget <amount|off>    monthly budget shown after the month spend
/usage currency <usd|cny>     budget currency (`$`/`¥` are accepted too)
/usage metadata <key>         metadata key holding the user (default `_user`)
```

`on` requires a Portkey login, a username, and an API key. The key comes from
`/usage key` when set, and otherwise from the active Portkey credential, so
`/login portkey` followed by `/usage user firstname.lastname` and `/usage on` is
enough. Use `/usage key` when usage has to be queried with a different (for
example, organization-scoped) key than the one that serves models.

`/usage budget 600` sets a monthly budget of `$600.00`; set the currency first
or afterwards with `/usage currency cny` to show it as `¥600.00`. Both the
amount and the symbol are accepted, so `/usage budget ¥600` works too. The spend
columns stay in USD either way.

By default the bar filters on the `_user` metadata field. If your gateway
attributes users with a different key (`email`, `user_id`, ...), set it with
`/usage metadata <key>` so the query matches your traffic.

The bar refreshes every 60 seconds and once after each turn. Its settings and
key live in `portkey-usage.json` in the oxide config directory (mode `0600`,
override the path with `OXIDE_USAGE_FILE`); they are never written to a project
scope. When a request fails, the bar keeps the last known amounts and appends
the error message after a `|`.

## Data locations and reset

Runtime state lives under the platform oxide config directory:

- `config.json` — provider and behavior settings
- `auth.json` — stored API keys (mode `0600`)
- `model-cache.json` — provider model lists (refreshed after 24 hours)
- `mcp-oauth/<server>.json` — OAuth tokens for remote MCP servers (mode `0600`)
- `sessions/<project>/<timestamp>_<id>.jsonl` — Pi-style session entry trees
- `snapshots/<project>/` — shadow-git snapshots for `/undo` and `/redo`; only created when the working directory is inside a git work tree (never the home directory, which would index the whole folder)
- `memory/` — persistent memory entries
- `trust.json` — saved project trust decisions
- `settings.json` — global settings such as `defaultProjectTrust`, `compaction`, `modelPrices`, `hideThinkingBlock`, `notifyOnComplete`, and `notifySound`
- `themes/<name>.json` — custom TUI themes
- `plugins/` — installed plugin packages, marketplaces, and plugin state
- `portkey-usage.json` — Portkey spend bar settings and API key (mode `0600`, override the path with `OXIDE_USAGE_FILE`)
- `truncated/` — full text of tool outputs that exceeded the line/byte cap, retained 7 days (override with `OXIDE_TRUNCATION_DIR`)

Global ecosystem resources can additionally live under `~/.oxide/` and
`~/.claude/`; global Claude-compatible MCP configuration is read from
`~/.claude.json`.

Deleting a session file removes that conversation; `/resume` can also delete
(Ctrl+D) or rename (Ctrl+R) sessions from the picker. `oxide sessions` offers
non-interactive management: `list` (with `--all` or `--older-than`), `delete`
(id, `--all`, or `--older-than`), `compact` (summarize older history and keep
the recent tail), and `merge` (concatenate two sessions, optionally summarizing
the second first). Deleting `snapshots/` removes undo history; deleting
`auth.json` logs you out.
