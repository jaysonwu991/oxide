# CLI and configuration

This guide covers the `oxide` CLI: where files live, and how to add or remove
MCP servers, subagents, slash commands, prompt templates, skills, plugins,
permissions, modes, reasoning, memory, project trust, themes, and context
compaction. The desktop app reads the same configuration and session files — see
[desktop.md](desktop.md).

For the reference tables rather than the task guides, see
[configuration.md](configuration.md) (`config.json` keys, CLI flags, environment
variables, providers, data locations), [ecosystem.md](ecosystem.md) (the `.oxide/`
layout), [tools.md](tools.md) (tool parameters and output caps),
[tui.md](tui.md) (keyboard shortcuts) and [modes.md](modes.md)
(non-interactive runs and `oxide sessions`).

## Scopes and precedence

Oxide merges two scopes:

- **Global** — native resources can live in either `~/.oxide/` or the platform
  Oxide config directory (`~/.config/Oxide` on Linux,
  `~/Library/Application Support/Oxide` on macOS, and `%APPDATA%\Oxide` on
  Windows). Claude Code-compatible resources come from `~/.claude/` and
  `~/.claude.json`.
- **Project** — the nearest ancestor of the working directory containing `.git`,
  `.oxide`, or `.claude`.

Project entries override global entries with the same name (for agents,
commands, prompt templates, skills, and MCP servers). Oxide reads its native
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
`notifyOnComplete`, `notifySound`, and `checkForUpdates`.

Installed plugin packages (see [Plugins and hooks](#plugins-and-hooks)) load
after global resources and before project resources, so project entries still
override plugins with the same name.

## MCP servers

MCP servers are declared under `mcpServers` in a JSON file. Oxide reads, in
increasing precedence:

1. Global `~/.claude.json`
2. Global `~/.oxide/mcp.json`
3. Global `<platform-config>/Oxide/mcp.json`
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
`oxide mcp auth <name>` for a server that needs OAuth. The TUI `/mcp` command
performs the same check.

```sh
oxide mcp list --json      # the same listing for a front-end: name, transport,
                           # detail, source, scope, enabled, state, status
oxide mcp disable filesystem --scope project   # keep the config, stop loading it
oxide mcp enable filesystem                    # back on, in whichever file defines it
```

`--json` is what the desktop app's **MCP servers** dialog and the VS Code
panel's `/mcp` picker read: the `state` field is one of `connected`,
`needs-auth`, `needs-trust`, `disabled`, `error`, so a client colors and groups
without parsing the display string, and `scope` is the `project` / `global`
value `--scope` accepts for a toggle that lands in the file the listing came
from. A server a project defines and `enabled: false` (or Claude Code's
`disabled: true`) is left out of the runtime and reported as `Disabled` —
turning it off never deletes its configuration. A toggle writes both spellings,
`enabled` and `disabled`, so a file that a Claude Code reader also loads says the
same thing to either harness; a name whose entry is not a server object, or that
is nowhere while one of the searched files cannot be parsed, is reported instead
of being passed over or written into a different file.

Options may appear before or after the server name. `oxide mcp auth` and
`oxide mcp remove` treat `--scope` as a boundary rather than just a file: a
pinned `--scope project` searches `<root>/.oxide/mcp.json` then
`<root>/.mcp.json`, `--scope global` searches the three global files
highest-precedence first, and neither touches the other scope — so a name
defined in both project and global resolves to the requested one. Without
`--scope`, both search every configured source in precedence order, so
`oxide mcp remove <name>` still deletes the entry the runtime actually uses.

At startup, Oxide adds only the enabled server names and configured URLs or
commands to the model context. It does not start a local process, make a remote
request, or check OAuth until a matching server is needed. A server is selected
three ways:

- **URL routing** — when a user message contains a URL whose host matches a
  server's routing domains, Oxide loads that server before the next model call
  so its tools are ready on the first turn. `webfetch` also redirects to the
  matching server instead of making an unauthenticated request.
- **Service names** — a message that names a service loads the server configured
  for it, so "create a Confluence doc" loads a server named `atlassian` (or
  `my-atlassian`) before the first model call, with no URL to paste. A server is
  matched by the words of its own name, by the aliases of the well-known service
  its name belongs to, and by the service its endpoint points at —
  `mcp.atlassian.com` is Atlassian whatever the server is called, so a server
  registered as `company-tools` answers to "Confluence" and "Jira" too. Words
  too short or too generic to name a service (`mcp`, `server`, `local`) are
  ignored.
- **`mcp_load`** — the model loads a server on demand through the built-in
  `mcp_load` tool, whose description lists each server's domains and the other
  names it answers to. Any of those names loads it — `confluence` loads a server
  named `atlassian` — and a name more than one server answers to is reported
  instead of guessed at.

The server's tools are discovered when loaded and become available on the next
agent step for the rest of the session. A server's own instructions — the
`instructions` string from the MCP initialize handshake, where a service says
which of its tools to call first — are passed to the model too: in the `mcp_load`
result when the model loads the server, and in the system prompt for the rest of
the run so an auto-loaded server's guidance is not lost. That section is headed
as untrusted: it states that the text came from the servers themselves rather
than from you or from the policy above it, and that it cannot change those
instructions, grant a permission, or redirect the task.

Routing domains come from the optional `domains` array in a server's config,
falling back to built-in presets for well-known services (Atlassian/Jira/Confluence,
New Relic, Context7, Contentful, Figma, GitHub, GitLab, Notion, Linear, and
Sentry) matched by the server's name or by the service its URL or command points
at. Exact hosts match exactly; a `*.` prefix (or leading `.`) matches the
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

On a `401 Unauthorized` response, Oxide reads the `WWW-Authenticate` challenge
and falls back to the standard `/.well-known/oauth-protected-resource` URLs. It
then discovers the authorization server, dynamically registers a client when
supported, runs the authorization-code flow with PKCE (`S256`) on a loopback
callback, stores the token under
`mcp-oauth/<server>.json` in the Oxide config directory (mode `0600`), and
refreshes it automatically. Run the flow up front with:

```sh
oxide mcp auth <name>
```

`oxide mcp auth` is the only command that opens a browser: authorization waits on
the loopback callback, so a running agent turn never starts it — a server without
a usable token reports that it needs authorization and names this command, in the
tool result and in `/mcp`. A stored token is still refreshed silently mid-turn.
An optional Claude Code-compatible `oauth` block can provide a pre-registered `clientId`,
`clientSecret`, `callbackPort`, `scopes`, or `redirectUri` when the authorization
server does not support dynamic client registration or needs overrides.

#### Connect to the Atlassian Rovo MCP server

Atlassian's remote MCP server exposes Jira, Confluence, and Compass tools over
Streamable HTTP. Adding the server by URL is enough:

```sh
oxide mcp add --transport http atlassian https://mcp.atlassian.com/v1/mcp
```

Then authorize it once with `oxide mcp auth atlassian`, which follows the OAuth
discovery metadata, opens the consent screen, and dynamically registers the
client. Afterwards a prompt that needs Atlassian loads the server with the
stored token; a prompt that arrives before it is authorized is told to run that
command rather than being interrupted by a browser.

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
recognize Oxide, since it only writes config files for editors it knows about.
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

On the first prompt that needs Context7, Oxide loads the server, follows its
OAuth discovery metadata, and opens the consent screen. To authorize before
starting Oxide, run:

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
- Enabled servers start lazily when first needed (a URL, a service name, or
  `mcp_load`). A server that fails to start or list tools is logged to stderr and
  skipped.
- Restart Oxide after editing.

## Subagents

Create `.oxide/agents/<name>.md` (or `.claude/agents/<name>.md`):

```markdown
---
name: rust-reviewer
description: Reviews Rust changes for correctness and style.
mode: subagent
tools: read, grep, find, ls, bash
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
- `tools` (optional) is a comma-separated list or YAML list of the only tools
  exposed to that agent. It is enforced again at dispatch, so omitted or
  fabricated tool calls cannot bypass the allowlist. Omit it for all tools.
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
- Built-in commands: `/help`, `/hotkeys`, `/exit`, `/new`, `/session`,
  `/tree`, `/fork`, `/clone`, `/name`, `/model`, `/reasoning`, `/theme`,
  `/trust`, `/export`, `/reload`, `/init`, `/connect`, `/logout`, `/models`,
  `/mcp`, `/plugins`, `/marketplaces`, `/notify`, `/permissions`, `/usage`,
  `/spend`, `/updates`, `/undo`, `/redo`, `/compact`, `/copy`,
  `/copy all`, and a skill by its own name (`/rust-conventions`, whose other
  spelling is `/skill:<name>`). The catalog holds one spelling per command, so
  `/help` and the autocomplete list each name once and a word that is not one —
  `/mcps` for `/mcp`, `/login` for `/connect` — is an ordinary message rather
  than a second way to reach a command. After a space,
  the built-in commands autocomplete their fixed arguments too
  (`/notify sound`, `/permissions on`, `/spend currency usd`, `/updates off`,
  `/plugins marketplace update`, and providers for `/connect`); Tab accepts the
  highlighted suggestion.
- **Remove** a command by deleting its file.
- `oxide models [--json] [--active]` reads the same normalized provider catalogs as the
  TUI's `/models` picker. The JSON form is for clients such as the VS Code
  extension; it includes the active provider, current model, each connected
  provider's models, and any provider-specific catalog error. `--active` queries
  only the active provider, so a latency-sensitive client is not held up by a
  slow inactive provider it will not display.
- `oxide commands [--json]` prints the catalog a client offers: the built-in
  names, each with its argument hint, their `kind` (`client` for a
  command a front-end answers itself, like `/mcp`; `prompt` for one it runs by
  sending `/name args`; `skill` for a skill, which is listed under its own name —
  sending `/name` loads it, and `/skill:<name>` is the terminal's other
  spelling) and the `front_ends` that perform it (`terminal`, `desktop`,
  `panel`), then the commands, prompt templates and skills the project and its
  plugins contribute, each with the `source` it was found in (`builtin`,
  `project` or `global`). A command whose frontmatter routes it to an agent or a
  subtask is one of those `prompt` entries: the CLI applies the routing when it
  runs the `/name` prompt, so a client only has to send it. An agent is not a slash command — it is selected
  with a picker of the front-end's own (`/agent` in the terminal, `--agent` for
  a run) — so agent files contribute no entry. The desktop app's `/` palette is
  built from the catalog, so its menu and the terminal's autocomplete agree on
  what exists, and the VS Code panel draws the same catalog in its composer — a
  skill's own row in either one sends `/name`, which is what loads it; the panel
  answers the client commands it has an action for from its own code.
  `front_ends` is what keeps a menu honest: each client offers the built-ins it
  is named in and leaves out the rest (`/permissions` is the terminal's and the
  desktop app's; `/agent` is the panel's), so a row that appears is one the
  client performs rather than one it answers with a "not available here" note.
  `desktop_only` is printed beside it for a client too old to read the new field:
  true when the panel is not among the front-ends that perform the command, which
  is how such a client hid the desktop app's own rows.

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
- Force-load a skill with `/<name> [args]` — its own name, which is what the `/`
  autocomplete, the desktop app's palette and the VS Code panel all list it
  under; `/skill:<name>` is the same skill and resolves too. Extra arguments are
  appended as `User: <args>`.
- **Remove** a skill by deleting its directory.

### Questions a skill asks the user

A skill that needs a decision — which database, which name, whether to keep
something — has the model call the `ask` tool, which puts the question to the
user in a front-end and returns the answer as the tool's result:

- One call carries 1–4 questions; each is a short `question`, an optional
  `header` (the dialog's title for it), up to 8 `options`, and `multiSelect` for
  a list where more than one label may be picked. A question with no options is
  answered in the user's own words. A `multiSelect` written out as a string
  (`"true"`) is read as the flag it is rather than failing the call.
- A question the user dismisses, or one that is never answered, comes back as
  the tool answering that nobody answered, so the model carries on without it —
  the request times out after five minutes and the turn never hangs. A front-end
  that painted the questions is told when the request goes (a `question_closed`
  event in rpc mode), so a dialog stops offering an answer nothing is waiting
  for.
- The tool is only offered when a front-end can answer it: the desktop app
  (`crates/desktop/src/ask.rs`) and the VS Code panel (which passes
  `--ask-questions` to `--mode rpc`, answering with a `question` frame). The TUI
  and `-p`/`--mode json` runs have no dialog for one, so `ask` is not in their
  tool list and the model asks its question in the reply instead. `--mode rpc`
  without `--ask-questions` is the same: a client that does not understand the
  frame is never sent one it cannot answer.

## Plugins and hooks

### Hook plugins (single files)

Place a `.ts` or `.js` file in `.oxide/plugins/` (or `.claude/plugins/`). Oxide
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

Oxide also supports Claude Code-style plugin packages: directories with a
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
under `<config>/Oxide/plugins/marketplaces/<name>/`; a local directory is
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
`<config>/Oxide/plugins/<marketplace>/<plugin>/`, so uninstalling a plugin or
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

Installed plugins live under `<config>/Oxide/plugins/` (next to `auth.json` and
`trust.json`), with their state in `plugins/config.json`. Their commands,
agents, skills, and MCP servers load at startup before project resources, so
project-local entries still override plugins with the same name, and each
loaded plugin is named to the model with what it brought (see *Plugins in the
model's context* below), as is one that is installed but disabled. After
installing, `/reload` picks up new commands, agents, and skills; hooks and MCP
servers require a restart.

### Plugins in the model's context

A run tells the model which plugins are already installed, so a request that
belongs to one is met with what it provides instead of the model building the
same thing by hand or telling you to install something you already installed.
The system prompt carries a `# Plugins` section naming every loaded plugin with
the description from its manifest and the capabilities it brought (`2 skills,
1 command, 1 MCP server`), one marked `(disabled)` when it is installed but
switched off — switched on by the user, not the model, since enabling a plugin
runs its hooks on every tool call, which the reply names as
`/plugins` in the terminal or `oxide plugin enable <name>`, so the guidance
works in the desktop app and the VS Code panel too — and a line naming how many
hook plugins are active — a hook can
rewrite a tool call's arguments before it runs and a tool's output before the
model sees it. The section appears whenever a plugin was loaded or a hook
plugin is active (including a single file in `.oxide/plugins/`), so a plugin
that only ships hooks is visible even though it contributes no commands or
skills.

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
- `auto_approve: true` in `config.json` (the default) skips prompts for `ask`
  rules. When it is `false`, the TUI asks: the question appears in the transcript
  and the composer becomes the answer field, where `y` runs the tool once, `a`
  allows it for this project from now on, and `n` — optionally with a reason,
  which the agent reads as guidance — refuses it. `Esc` refuses without a reason,
  and an empty line leaves the request open.
- `--ask-approvals` asks for one run (it switches `auto_approve` off for it) and
  `--no-ask-approvals` runs gated tools without asking; either overrides the
  stored value.
- In the TUI, `/permissions [on|off]` toggles that
  same `auto_approve` key for later runs, `/permissions list` shows the state and
  the tools this project allows from now on, and `/permissions clear` forgets
  them.
- The VS Code panel asks on its own: `oxide.askApprovals` (default on) starts the
  turn with `--ask-approvals`, so the shared `config.json` value does not decide
  there.
- Without an interactive host a gated call is denied rather than run: `-p`,
  `--mode json`, and an `--mode rpc` client that did not pass `--ask-approvals`
  never prompt. Those two non-interactive modes have no answer channel at all, so
  they reject the approval flags instead of silently running the tool they were
  meant to gate.
- The desktop app shows the same question as a card in the transcript (`Deny` /
  `Allow once` / `Always allow`) and holds the tool until it is answered.
- `Always allow` is remembered per project in `<config>/Oxide/approvals.json`,
  which every front-end reads — the TUI included — so the question does not come
  back for that tool in that repository. An answer that never arrives is denied
  after five minutes so a turn cannot hang.

Questions are the other side of that: the model can ask *you* something through
the `ask` tool, which the desktop app and the VS Code panel answer with a dialog
(see [Questions a skill asks the user](#questions-a-skill-asks-the-user)).

There is no permission mode: the rules and `auto_approve` decide every call.
For a read-only run, allowlist the read tools with `--tools` (e.g.
`oxide -t read,grep,find,ls -p "review this"`).

## Reasoning

Reasoning effort controls how much internal reasoning Oxide asks the model to
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
In the TUI, press Shift+Tab to cycle auto → off → low → medium → high; the current
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
`hideThinkingBlock` in the global `settings.json` makes Oxide start collapsed,
matching Pi. Reasoning is stored in the session as thinking blocks: Anthropic
replays them on later turns, and a DeepSeek thinking-mode request puts them back
as `reasoning_content` on every earlier assistant message — the API rejects a
request that drops an earlier turn's reasoning, whether it made a tool call or
gave the final answer, and `deepseek-flash` skips reasoning on quick tool calls
and short answers, so that field is sent empty rather than left out; every other
OpenAI-compatible request strips them. While a
turn is in flight the status row names the phase (`thinking...`,
`running tool...`, `compacting...`, `summarizing branch...`) and reports
transient stream failures as `retrying (1/3) in 1s...` before the client backs
off and tries again. A response that arrives with neither text nor tool calls is
retried on the same schedule, so an occasional empty reply does not end the
turn, and a stream that drops after it has already streamed part of the answer
is retried too: the partial reply disappears when the retry starts and the new
attempt streams from the beginning. Only after the retries are exhausted is the
failure reported as an error, and the text the last attempt streamed is kept in
the session so the next message can continue from it.

### Status tips and queued messages

That status row only exists while the agent is busy, so anything you do at rest
reports itself as a dim line in the transcript instead — `copied 243 chars` after
you drag over text, `tool output collapsed` after Ctrl+O, `restored 2 queued
messages to the editor` after a dequeue. Only the newest tip is kept, so repeated
actions update one line rather than growing the transcript, matching Pi's
`showStatus`.

While the agent is busy, Enter safely queues the next turn after the current
response and Alt+Enter deliberately steers before the agent's next model step.
Both show up as your turns, and
the status row adds `2 queued · Option+Up to edit`. Pressing that key empties the
queues back into the message box — queued text first, whatever you were already
typing after it — so you can extend a message before it is sent. Their entries are
also removed from the transcript, since they were never sent. The key is `Alt+Up`
(`Option+Up` on macOS, where `Alt` is the Option key) and `Alt+Q` on Windows and
WSL, where the terminal claims `Alt+Up` for scrollback.

## Attachments

A message can carry images, PDFs and text files, which the selected LLM reads as media
or as text. `Ctrl+V`
pastes a clipboard image — or the file the clipboard holds, so a screenshot
copied from the Finder attaches the picture itself rather than the pasteboard's
icon of the file; a file copy that arrived from another machine leaves its URL
behind without the file, and what the pasteboard itself carries is attached
then — `@path` names one on disk, and `/attach
[list|remove <id|n>|clear]` lists and edits what is pending; a message queued
while the agent is busy keeps the attachments it was queued with. In a
non-interactive run the same parts come from `--image <path>` and from `@path`
references in the prompt, so `oxide -p "what changed here? @shot.png"` works
without a terminal. The terminal completes a reference as it is typed: the
project's own files and folders are offered above the composer (`↑`/`↓` walk the
rows, Tab or Enter takes one, Escape closes the list), a folder keeps its token
open so the query goes on narrowing inside it, a file closes it with a space, and
a folder the reference already spells is left out so taking a row walks into it.
The rules are `oxide_core::at`, the same ones the desktop app's composer
completes from.

An image is downscaled to a 1568px long edge — the same bound the desktop app
and the VS Code panel paint their thumbnails at — so a retina screenshot is not
re-encoded at full resolution into every request and every session entry. An
image in a format no provider takes but this machine's image tools can convert
(a TIFF, a HEIC, a HEIF, an AVIF) is converted to PNG rather than refused, and
one neither can decode is reported by name.

Anything that is neither an image nor a PDF is attached as its own text, wrapped
the way Pi wraps it (`<file name="…">…</file>`), so a `.csv`, a `.json` or a
source file can ride along without a format list deciding what may travel. The
only size gate is 20 MB (`oxide_core::media::MAX_ATTACHMENT_BYTES`), checked
before the file is read, and a binary payload holding a NUL byte — neither
media nor text — is refused with a message naming the file rather than being
sent as mojibake.

The selected LLM is the source of truth for rich-media support. Oxide serializes
and forwards media without inferring support from provider or endpoint names.
If the LLM rejects a media request, the error points to the model's capabilities
instead of silently rewriting or discarding the attachment.

## Desktop notifications

When an agent turn finishes, Oxide raises a system toast (Notification Center on
macOS, `notify-send` on Linux, a Windows toast) whose body names the turn, so you
can switch windows while a long task runs. The desktop app and the VS Code panel
name the thread by its summarized title — its session name, else the first
message sent — which is the same line the panel's header shows; the TUI's body
is a short snippet of the reply. Only real agent turns notify — internal work
such as `/compact` and branch summaries stays silent — and a turn you stopped
yourself is not announced by the desktop app or the panel.

All three front-ends honor the same switch: the desktop app and the VS Code
panel read `notifyOnComplete` (the panel next to its own
`oxide.notifyOnFinish`), so turning the toast off in the terminal silences them
too — only the alert sound is the terminal's and the desktop app's, since a VS
Code notification has none of its own. Both the toast and its alert sound are on
by default. In the TUI, `/notify`
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

## Update check

A launch looks for a newer release of the CLI in the background and, when it
finds one, prints it in the transcript the way Pi announces one:

```
───
Update Available
New version 0.35.0 is available. Run `oxide update`
Changelog: https://github.com/jaysonwu991/oxide/releases/tag/v0.35.0
───
```

The lookup never holds up the launch: the release the last launch found is read
from `updates.json` and shown at once, and a fresh lookup only runs once that
answer is more than six hours old (one request a day for a daily launch —
`oxide update` is still the way to look right now). What is remembered is one
release per component, so the CLI's own notice never overwrites the desktop
app's or the extension's answer.

Nothing is offered where `oxide update` could not install a release anyway: a
`target/debug` build, a distribution package, or a binary someone moved has no
install method oxide recognizes, so it is told nothing rather than offered an
install that would fail. A Homebrew Cellar is offered `brew upgrade oxide`,
since that is what `oxide update` hands a Homebrew install to.

- `/updates` reports the state and the newest release seen; `/updates on|off`
turns the launch check on or off (asking right now when it turns it on).
- `checkForUpdates` in the global `settings.json` — or the project's
  `.oxide/settings.json`, which wins — turns it off by hand; the default is on.
  `OXIDE_CHECK_FOR_UPDATES=0|1` overrides both for one launch.
- The check runs only in the TUI. A `-p`/`--mode json`/`--mode rpc` run prints
  nothing of its own, so a scripted run's output stays the frames it was asked
  for; the desktop app and the VS Code panel keep their own **Check for
  Updates** surfaces over the same `oxide update --check --json`.

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
`memory/` in the Oxide config dir; the 8 most recent entries are injected into
the system prompt automatically. Use the `memory` tool to add, search, or forget
entries.

The prompt also carries a `# Workspaces` section naming the working directory
and the other folders added to Oxide — the desktop's project list, read from
`desktop/projects.json` — with their paths, so a question about a repository
elsewhere on the machine ("can you reach the `api-service` repo?") is answered
from the list instead of a `find ~` that reads every unrelated project before
the command times out. It says the file tools take absolute paths, so a file
in a sibling project can be read, searched, and edited from the run, while
`bash` still starts in the current project's root.

## Project trust

Project-local resources that can change behavior or execute code (agents,
commands, prompts, skills, plugins, `SYSTEM.md`) load only after the project is
trusted. On interactive startup Oxide asks when a project requires trust and no
decision is saved; non-interactive runs use `defaultProjectTrust` (in
`settings.json`) without prompting.

- `defaultProjectTrust`: `ask` (default), `always`, or `never`.
- `--approve`/`-a` and `--no-approve` override for one run.
- `/trust [show|off]` saves a decision for the current directory to `trust.json`
  (the closest saved decision on the current or a parent path applies).
- Context files always load regardless of trust.

## Themes

Oxide ships `dark` and `light`. Add custom themes as JSON under
`.oxide/themes/<name>.json` or `<config>/Oxide/themes/<name>.json`, then select
one with `--use-theme <name>` or `/theme <name>`. The built-in palettes come
from `oxide_core::theme_view`, shared with the desktop app, so the CLI and
desktop render identical colors and both read the same custom theme files.
Colors accept names or `#rrggbb`; unset slots fall back to the built-in `dark`
theme.

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
`/theme` lists available themes; after a switch, Oxide immediately rebuilds the
styled transcript.

Selections use reverse video and outcome rows retain words or symbols, so color
is not the only state cue. When authoring a custom theme, choose foregrounds
with strong contrast against the terminal background and keep `success`,
`error`, and `tool` visually distinct.

For the complete keyboard guide, see
[Keyboard shortcuts](tui.md#keyboard-shortcuts).

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

The model window is derived from the model when it is known, falling back to
1000000; an explicit `context_window` in `config.json` overrides it.
`OXIDE_COMPACTION_ENABLED`, `OXIDE_COMPACTION_RESERVE_TOKENS`, and
`OXIDE_COMPACTION_KEEP_RECENT_TOKENS` override the file settings, and
`OXIDE_CONTEXT_LIMIT` overrides `context_window`. Manual compaction is available
with `/compact [focus]` in the TUI or `oxide sessions compact`.

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

Start `oxide` even without a key, then run `/connect`:

- `/connect` lists the providers — enter a number or name, then paste the API key
  (Enter with the field empty reuses the stored key when there is one). Providers
  with a stored key are marked `connected`. After the key, an optional settings
  step pre-fills the model and endpoint (and, for Portkey, the Config ID) so a
  custom gateway can be configured without editing `config.json` by hand; press
  Enter through the rows to keep the pre-filled values, or replace one before
  saving — a row left blank keeps the provider's default.
- `/connect deepseek` or `/connect portkey` skips the picker. When that provider is
  already connected the command switches to it; otherwise it asks for the key.
- Bedrock and Vertex ask for no key: they authorize with a credential this
  machine already has — an AWS signing identity, Google's application-default
  file — so `/connect bedrock` is a login in itself. A machine without one says
  what to set instead of asking for a key that would never be read.
- A provider that authenticates by a browser flow instead of a pasted key asks
  for that instead: `/connect github-copilot` shows a GitHub URL and a one-time
  code (also copied to the clipboard), waits for the approval, and stores the
  token it mints before moving on to the same settings step. Nothing is typed.
  `/connect gitlab` is not one of those: the token to paste is a GitLab personal
  access token carrying the `ai_features` scope.
- `/logout` removes the active provider's stored credential and switches to
  another logged-in provider when one is left; `/logout <provider>` removes a
  specific one without disturbing the active session.

Press Enter to confirm and Esc to cancel. The key is stored in `auth.json`
(mode `0600`) and the active provider is written to `config.json`, so it applies
to the running session and the next launch.

### From a front-end

The desktop app's **Connect** dialog and the VS Code panel's Connect Provider
command draw the same table the TUI's picker does, from one definition:

```console
oxide providers --json          # every provider, and the state of each
oxide login openai --json --key-stdin <key
oxide login ollama              # a stored credential, or one that needs none
oxide login bedrock             # a credential this machine already has
```

`oxide providers --json` prints `{ "active": <name|null>, "providers": [...] }`,
where each row carries the `name` a login takes, the `label` and `description` a
picker paints, the `keyUrl` a key is issued at, and `local`, `stored` and `active`
— the state a row shows beside it. Its `credential` says where a credential comes
from, which is what decides whether a picker asks for a key at all:

- `key` — a key typed here and kept in `auth.json`.
- `external` — one this machine already holds, read where it lives and never
  copied into the store: Bedrock's AWS signing identity (`AWS_ACCESS_KEY_ID` and
  `AWS_SECRET_ACCESS_KEY`, or `~/.aws/credentials`) and Vertex's
  application-default file (`GOOGLE_APPLICATION_CREDENTIALS`, or
  `gcloud auth application-default login`). No key is asked for, and a login with
  none stores nothing.
- `none` — nothing to present; a model server on this machine.

The human-readable form of the same listing is one tab-separated line per
provider, marked `[in use]`, `[stored]`, `[no key needed]` or
`[machine credential]`.

`oxide login <provider> [--key-stdin] [--model <m>] [--base-url <url>] [--json]`
is what a front-end runs when a row is taken. The key is read from stdin rather
than an argument — an argument is visible in the process listing and kept in a
shell's history — and `--key-stdin` is what says to read one, so a stored
provider is connected by name alone, and so is one whose credential is the
machine's own (Enter with an empty key does the same in the TUI). A provider
whose own credential is missing is refused with what to set rather than asked for
a key it would never read. `--model` and `--base-url` are the settings step's
values, and `--json` prints `{ provider, label, model, local }` for the caller to
report. Either front-end writes `auth.json` and `config.json` through the same
core, so a login in one is a login in the others; neither needs a project open.

### Several providers at once

Credentials for different providers live side by side in `auth.json`, so you can
log in to OpenAI, Anthropic, and Portkey and move between them without re-entering
a key. Switching happens by:

- `/connect <provider>` for a provider that is already connected.
- Enter in the login dialog's key step, which reuses the stored key — type a key
  first to replace it instead.
- Picking a model of another provider in `/models` (see below).
- `--provider <name>` on the command line, for one run.

Each provider keeps the model it was last used with in the `provider_models` map
in `config.json`, and a custom endpoint it was last used with in the
`provider_base_urls` map, so switching back restores that provider's model and
gateway instead of carrying the other provider's values:

```json
{
  "provider": "anthropic",
  "model": "claude-sonnet-5",
  "provider_models": { "openai": "gpt-4o-mini", "anthropic": "claude-sonnet-5" },
  "provider_base_urls": { "portkey": "https://gateway.example.com/v1" }
}
```

The active provider's endpoint is also written to `base_url` for convenience and
hand editing; it is removed when it matches the preset default. A custom
(non-preset) provider keeps the last endpoint it was given, so switching away
and back restores it instead of the previous provider's URL. A custom provider
with no remembered endpoint uses the previous provider's URL, or the endpoint
from `OXIDE_BASE_URL`.

You can also provide a key without the login flow, through the environment
variable the provider's own documentation uses (`OPENAI_API_KEY`,
`DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY`, `PORTKEY_API_KEY`, `ZAI_API_KEY`,
`GEMINI_API_KEY`, `GROQ_API_KEY`, `GITHUB_TOKEN`, … — each is listed with its
provider in [Configuration reference](configuration.md#providers)) or an
`api_key` entry in `config.json`; environment variables take precedence over
`auth.json`. A provider that is not in the table takes its key from
`<NAME>_API_KEY` (`oxider` → `OXIDER_API_KEY`).

See [Configuration reference](configuration.md#configjson) and
[Providers](configuration.md#providers) for the full list.

### Z.AI (GLM)

Z.AI serves the GLM models over the OpenAI-compatible Chat Completions API, so
the provider only differs in its endpoint and defaults:

| Setting | Value |
| --- | --- |
| Names | `zai`, `glm`, `z.ai`, `z-ai`, `zhipu`, `bigmodel` |
| Base URL | `https://api.z.ai/api/paas/v4` (`ZAI_BASE_URL`) |
| Default model | `glm-5.3` (`glm-5.3-flash` is cheaper and faster) |
| Key | `ZAI_API_KEY`, or `/connect glm` |

```text
/connect glm
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
[Reasoning](#reasoning) for how `/reasoning` maps onto GLM's levels.

### Portkey

Portkey uses the OpenAI-compatible Chat Completions API. In both setups below,
keep the API key out of `config.json`: `/connect portkey` stores it in
`auth.json` with mode `0600` and selects Portkey as the active provider.

#### Login without a custom gateway

Start Oxide and run:

```text
/connect portkey
```

Paste your Portkey API key when prompted. For a new setup, no other
configuration is required: Oxide uses `https://api.portkey.ai/v1` and
`claude-sonnet-5` by default. If you previously configured a custom gateway,
remove its `base_url` and its `provider_base_urls.portkey` entry before using
the default endpoint (switching away from a custom gateway remembers it so that
logging in to another provider does not leak the gateway URL). To use a model
from the Portkey Model Catalog, set its identifier in `config.json`:

```json
{
  "provider": "portkey",
  "model": "@provider-slug/model-name"
}
```

The global file is `~/Library/Application Support/Oxide/config.json` on macOS,
`~/.config/Oxide/config.json` on Linux, and `%APPDATA%\Oxide\config.json` on
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

After saving the file, start Oxide and run `/connect portkey`. If Oxide is already
running, save the file, run `/connect portkey`, then run `/reload`. Paste the API
key for that gateway when prompted. Oxide sends the key as
`x-portkey-api-key` and the Config ID as `x-portkey-config`; it does not send
the key as a bearer token.

`portkey_config` is optional when the gateway does not require a saved Portkey
Config. A Config ID typed in the `/connect portkey` settings step is written to
`portkey_config` in the global `config.json`, so it is still in force on the
next launch and is not lost when you log in to another provider. `model_catalog`
is also optional, but is useful when the gateway blocks
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
usable; a custom endpoint is listed through the URL remembered for it in
`provider_base_urls`, and is left out when it has none, since its catalog would
otherwise be queried against the active provider's endpoint.

Refreshing the credential with `/connect portkey` preserves an existing Portkey
model, custom base URL, and Config ID when Portkey is already active. See
Portkey's documentation for the current [gateway headers](https://portkey.ai/docs/api-reference/inference-api/headers)
and [OpenAI-compatible setup](https://portkey.ai/docs/integrations/libraries/openai-compatible).

## Portkey usage bar

When your models are routed through Portkey, Oxide can show a spend bar on the
last line of the TUI:

```text
→ firstname.lastname | Session: $0.00 | Today: $20.61 | Month: $220.69 / $600.00
```

`Session` is the cumulative cost of the current session (the same figure as the
footer's `$cost`). `Today` and `Month` come from the Portkey analytics API, so
they cover every request your workspace attributed to that user from any
client, not just this session, and they are the USD amounts Portkey reports. The
optional ` / $600.00` after `Month` is the budget configured with
`/spend budget`, formatted in the currency set with `/spend currency`.

The bar only runs while a Portkey provider is logged in, because the spend it
shows belongs to that account: `/connect portkey` (or `PORTKEY_API_KEY`) has to be
in place before `/spend on` and before the bar appears again at startup.

### Configure and toggle

Run `/spend` to open the Portkey spend-bar dialog. (The shared `/usage` is not
this bar: it reports the current chat's tokens, cost and context window and takes
no arguments, so the bar's own settings are all behind `/spend`.) The dialog is
a small form over the bar's settings: move with `↑`/`↓`, press `Enter` to toggle
`Enabled` and `Currency` or to edit a text field in place (the cursor sits at the
end of the value; an API key is masked), and `Esc` to save and close. The rows
are:

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
/spend status                 show the status and syntax
/spend on | off               show or hide the bar
/spend user <firstname.lastname>   the user whose spend to show
/spend key <pk-...>           usage API key (or `off` to use the provider key)
/spend budget <amount|off>    monthly budget shown after the month spend
/spend currency <usd|cny>     budget currency (`$`/`¥` are accepted too)
/spend metadata <key>         metadata key holding the user (default `_user`)
```

`on` requires a Portkey login, a username, and an API key. The key comes from
`/spend key` when set, and otherwise from the active Portkey credential, so
`/connect portkey` followed by `/spend user firstname.lastname` and `/spend on` is
enough. Use `/spend key` when usage has to be queried with a different (for
example, organization-scoped) key than the one that serves models.

`/spend budget 600` sets a monthly budget of `$600.00`; set the currency first
or afterwards with `/spend currency cny` to show it as `¥600.00`. Both the
amount and the symbol are accepted, so `/spend budget ¥600` works too. The spend
columns stay in USD either way.

By default the bar filters on the `_user` metadata field. If your gateway
attributes users with a different key (`email`, `user_id`, ...), set it with
`/spend metadata <key>` so the query matches your traffic.

The bar refreshes every 60 seconds and once after each turn. Its settings and
key live in `portkey-usage.json` in the Oxide config directory (mode `0600`,
override the path with `OXIDE_USAGE_FILE`); they are never written to a project
scope. When a request fails, the bar keeps the last known amounts and appends
the error message after a `|`.

## A turn's changes

A finished turn can say what it changed, from one listing: the project's shadow
snapshot (the bare git repo under `snapshots/` in the config directory) records
the state a run starts from, and when the run finishes its work tree is diffed
against that baseline — so a file a shell command, a formatter or an MCP server
wrote is listed beside the ones a tool call named. Each entry carries the status
(`A`/`M`/`D`), the added and removed line counts, and the same compact
line-numbered preview the `write`/`edit` cards paint; a binary file is named
rather than counted, and a mode-only change is listed with nothing to count.

Two front-ends draw it: the desktop app paints a card per finished turn (with
**Review** and **Undo**) and the VS Code panel opens a file in VS Code's own diff
editor. A client on the RPC channel gets the same listing as a `turn_changes`
frame — `{project, baseline, changes}`, written after the turn's own last event
— and draws whatever it likes from it. The terminal is not one of them: its
`/undo` and `/redo` restore the shadow snapshot, but a finished turn leaves no
change card in the transcript.

  ```sh
  oxide changes show src/main.rs --baseline <rev> --project <root>
  ```

- A front-end that draws its own diff reads the left side through the CLI, since
  the file as the run found it exists only in the snapshot. `--baseline` is the
  revision the run reported when it finished, and `--project` is the project the
  listing named — its own frame carries it, so a client that has moved to another
  folder since still reads the snapshot the turn belongs to; it defaults to the
  current directory. The file is printed as the baseline recorded it — alongside
  whatever the work tree holds now — so a front-end can diff the two.
- A turn that changed nothing has nothing to show and no revision to read.
- A project does not have to be a git clone: the snapshot is oxide's own repo, so
  a plain folder is recorded too. The shadow snapshot is refused for a directory
  that must not be walked — the home directory or an ancestor of it, anything
  holding the config directory, and a directory that is neither inside a git work
  tree nor project-sized (over 20,000 files or 512 MB, counted without the build
  directories the snapshot excludes). Where it is refused there is simply no
  listing for the turn, and the run itself is unaffected.

## Data locations and reset

Runtime state lives under the platform Oxide config directory:

- `config.json` — provider and behavior settings
- `auth.json` — stored API keys (mode `0600`)
- `model-cache.json` — provider model lists (refreshed after 24 hours)
- `mcp-oauth/<server>.json` — OAuth tokens for remote MCP servers (mode `0600`)
- `sessions/<project>/<timestamp>_<id>.jsonl` — Pi-style session entry trees
- `snapshots/<project>/` — shadow-git snapshots for `/undo` and `/redo`, and for
  the change listing a turn leaves; created for any project directory — a git
  work tree or a plain folder — but never the home directory or an ancestor of it
  (which would index the whole folder), a directory holding the config directory,
  or a directory that is neither a git work tree nor project-sized
- `memory/` — persistent memory entries
- `trust.json` — saved project trust decisions
- `settings.json` — global settings such as `defaultProjectTrust`, `compaction`, `modelPrices`, `hideThinkingBlock`, `notifyOnComplete`, `notifySound`, and `checkForUpdates`
- `updates.json` — the newest release of each component the last launch found, so the "update available" notice needs no network wait (override with `OXIDE_UPDATES_FILE`)
- `themes/<name>.json` — custom TUI themes
- `plugins/` — installed plugin packages, marketplaces, and plugin state
- `portkey-usage.json` — Portkey spend bar settings and API key (mode `0600`, override the path with `OXIDE_USAGE_FILE`)
- `truncated/` — full text of tool outputs that exceeded the line/byte cap, retained 7 days (override with `OXIDE_TRUNCATION_DIR`)

Global ecosystem resources can additionally live under `~/.oxide/` and
`~/.claude/`; global Claude-compatible MCP configuration is read from
`~/.claude.json`.

Deleting a session file removes that conversation; `/session` can also delete
(Ctrl+D) or rename (Ctrl+R) sessions from the picker. `oxide sessions` offers
non-interactive management: `list` (with `--all` or `--older-than`), `show
<id>` (print one saved conversation — `--tail <n>` for just its newest
messages, `--json` for the object a front-end draws: id, name, cwd, path, how
many messages the thread holds and how many were returned, the messages, and
the thread's usage totals), `delete` (id, `--all`, or `--older-than`), `compact`
(summarize older history and keep the recent tail), and `merge` (concatenate two
sessions, optionally summarizing the second first). An id can be given to `show`
as the short id a listing prints or as a session file's path, and a project with
many sessions resolves it from the file name (`<timestamp>_<id>.jsonl`) without
reading the other threads. Deleting `snapshots/` removes undo history; deleting
`auth.json` logs you out.
