# Configuration reference

Where Oxide's files live and every key, flag, and environment variable they
accept. For task-by-task instructions (adding MCP servers, agents, commands,
skills, plugins, permission rules) see [cli.md](cli.md).

## config.json

Oxide reads `config.json` from the platform config directory:

- Linux: `~/.config/Oxide/config.json`
- macOS: `~/Library/Application Support/Oxide/config.json`
- Windows: `%APPDATA%\Oxide\config.json`

```json
{
  "provider": "deepseek",
  "model": "deepseek-chat",
  "base_url": "https://api.deepseek.com/v1",
  "api_key": "",
  "system_prompt": "You are Oxide...",
  "max_tokens": 8192,
  "context_window": 1000000,
  "auto_approve": true,
  "reasoning": "auto",
  "theme": "dark"
}
```

`api_key` may be left empty when a key is available via `/login` or the
environment.

### auto_approve

`auto_approve` controls whether tool calls run without prompting; with it off,
permission rules that resolve to `ask` are asked about by the TUI and by the
desktop app (an approval card), and are denied in a non-interactive run. The TUI
asks in the transcript and answers in the composer: `y` runs the tool once, `a`
allows it for this project from now on, and `n` — optionally with a reason the
agent reads as guidance — refuses it (`Esc` refuses too). `--ask-approvals` asks
for one run, `/approvals [on|off]` toggles the stored value, `/approvals list`
shows the state and the tools this project allows, and `/approvals clear` forgets
them. The VS Code panel asks on its own setting (`oxide.askApprovals`) rather
than this one. See [Permissions](cli.md#permissions).

### reasoning

`reasoning` controls how much reasoning effort Oxide requests. `auto` (the
default) leaves reasoning behavior and effort to the provider/model. Newer Claude
models use adaptive thinking without a forced effort; other APIs receive no
effort override. `off`, `low`, `medium`, and `high` force a level using
OpenAI-compatible `reasoning_effort`, Anthropic adaptive thinking with
`output_config.effort`, or a legacy Anthropic thinking budget as appropriate. In
the TUI press Shift+Tab to cycle levels; `--reasoning` and `OXIDE_REASONING` set
the starting level. See [Reasoning](cli.md#reasoning).

### max_tokens and context_window

`max_tokens` caps the output of a single model turn, reasoning included. When a
reasoning model exhausts that budget on hidden reasoning and returns nothing,
Oxide retries with a doubled budget (up to 32768) before reporting the failure,
so a long-thinking turn recovers instead of ending in an empty response.

`context_window` is the model's full context window in tokens, and controls both
the context gauge and the automatic compaction threshold. When it is unset (or
`0`), Oxide uses the documented window of a known model from a small built-in
table and falls back to `1000000` for an unknown one, so a fresh configuration
compacts before a model with a smaller window rejects the request. Set it
explicitly to override the derived value — a larger window retains more
conversation before compaction. `OXIDE_CONTEXT_LIMIT` overrides it for one
environment.

## CLI flags

```
oxide [OPTIONS] [@files...] [PROMPT...]
oxide mcp <COMMAND>
oxide plugin <COMMAND>
oxide sessions <COMMAND>
oxide uninstall [--keep-config] [--keep-data] [--dry-run] [--force]
oxide update [--check] [--version <VERSION>] [--force]
```

| Flag | Description |
| --- | --- |
| `[PROMPT]...` | Prompt words. `@path` reads a file into the prompt (images/PDFs become attachments). Providing one implies non-interactive mode. |
| `-m, --model <MODEL>` | Model to use (overrides config). |
| `--provider <PROVIDER>` | Provider name (overrides config). |
| `--agent <AGENT>` | Agent to run, from `.oxide/agents` (or `.claude/agents`). |
| `--mode <MODE>` | Output mode: `print`, `json`, or `rpc` (defaults to print for a prompt). |
| `--ask-approvals` | Ask before a permission-gated tool runs: in the TUI the question is answered in the composer, and in `--mode rpc` it goes to the client as an `approval_request`. |
| `--no-ask-approvals` | Run gated tools without asking, overriding `auto_approve` for this run. |
| `--ask-questions` | Send `question_request` frames to `--mode rpc` clients (skill `ask` tool questions); without it the `ask` tool is not offered. |
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

## Environment variables

| Variable | Purpose |
| --- | --- |
| `OXIDE_PROVIDER` | Provider name. |
| `OXIDE_MODEL` | Model name. |
| `OXIDE_BASE_URL` | API base URL. |
| `OXIDE_API_KEY` | API key. |
| `OXIDE_REASONING` | Reasoning effort (`auto`, `off`, `low`, `medium`, `high`). |
| `OXIDE_CONTEXT_LIMIT` | Override `context_window`, the model context window used for the footer's context percentage and compaction threshold. |
| `OXIDE_COMPACTION_ENABLED` | Enable/disable automatic context compaction. |
| `OXIDE_COMPACTION_RESERVE_TOKENS` | Tokens reserved for the response before compaction triggers. |
| `OXIDE_COMPACTION_KEEP_RECENT_TOKENS` | Recent tokens kept verbatim when compacting. |
| `OXIDE_TRUNCATION_DIR` | Directory for saved truncated tool output (default `truncated/` in the config dir). |
| `OXIDE_NOTIFY_ON_COMPLETE` / `OXIDE_NOTIFY_SOUND` | Override the desktop-notification flags (`/notify`). |
| `OXIDE_SETTINGS_FILE` | Override the global `settings.json` path the TUI writes. |
| `OXIDE_USAGE_FILE` | Override the `portkey-usage.json` path for the Portkey spend bar. |
| `OPENAI_API_KEY` / `OPENAI_BASE_URL` | OpenAI credentials. |
| `DEEPSEEK_API_KEY` / `DEEPSEEK_BASE_URL` | DeepSeek credentials. |
| `ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` | Anthropic credentials. |
| `PORTKEY_API_KEY` / `PORTKEY_BASE_URL` | Portkey AI Gateway credentials. |
| `PORTKEY_CONFIG` | Optional Portkey config ID sent as `x-portkey-config`. |
| `PORTKEY_MODELS` | Optional comma-separated model catalog for keys that cannot call `/models`. |
| `ZAI_API_KEY` / `ZAI_BASE_URL` | Z.AI (GLM) credentials. |

## Providers

| Name | API | Default model | Base URL | Key env |
| --- | --- | --- | --- | --- |
| `openai`, `gpt`, `gpt-4`, `gpt-4o` | OpenAI-compatible | `gpt-4o-mini` | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
| `deepseek` | OpenAI-compatible | `deepseek-chat` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` |
| `anthropic` | Anthropic Messages | `claude-3-5-sonnet-latest` | `https://api.anthropic.com/v1` | `ANTHROPIC_API_KEY` |
| `portkey`, `port-key` | OpenAI-compatible gateway | `claude-sonnet-5` | `https://api.portkey.ai/v1` | `PORTKEY_API_KEY` |
| `zai`, `glm`, `z.ai`, `z-ai`, `zhipu`, `bigmodel` | OpenAI-compatible | `glm-5.3` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` |

Any OpenAI-compatible endpoint can be used by setting `provider`, `base_url`,
`model`, and a key.

### Credentials

Credential management happens inside the TUI with the Pi-style commands:

```text
/login [provider]    connect a provider and store its API key
/logout [provider]   remove stored credentials
```

`/login` opens a provider picker (`/login <provider>` skips straight to the key;
for a provider that is already connected it switches to it instead of asking for
the key again, and pressing Enter on the key step reuses the stored key). After
the key, an optional settings step lets you set the model, base URL, and (for
Portkey) Config ID, pre-filled with the provider's defaults so Enter keeps them.
Keys are stored in `auth.json` in the Oxide config directory (mode `0600`) and
resolved after environment variables and before the config file. Any number of
providers can be stored at once, and the active provider is written to
`config.json` so the next launch uses it. `/models` lists the catalogs of every
logged-in provider, and picking a model from another one switches to it. Each
provider remembers the model and custom endpoint it was last used with in the
`provider_models` and `provider_base_urls` maps of `config.json`, so a gateway
configured for one provider does not leak into the others. See
[Providers and credentials](cli.md#providers-and-credentials).

### Portkey

Run `/login portkey` in the TUI, or set `PORTKEY_API_KEY`, then select a model
with `/models` or `"model"` in `config.json`. The preset uses
`https://api.portkey.ai/v1`, sends the key as `x-portkey-api-key`, and defaults
to `claude-sonnet-5`. Run `/usage` to open the spend-bar dialog and enable a
full-width bar (session, today, and month cost against an optional monthly budget
in `$` or `¥`) in the TUI.

For custom gateways, Config IDs, environment precedence, and model-catalog
fallbacks, see the full [Portkey configuration](cli.md#portkey) section. The
[Portkey usage bar](cli.md#portkey-usage-bar) documents the `/usage` dialog and
the `portkey-usage.json` file.

### Z.AI (GLM)

Run `/login glm` in the TUI (or `/login zai`), or set `ZAI_API_KEY`, then pick a
model with `/models`. The preset talks to Z.AI's OpenAI-compatible endpoint
`https://api.z.ai/api/paas/v4` and defaults to `glm-5.3`; `glm-5.3-flash` is the
cheaper, faster option. Z.AI documents no model listing endpoint, so the picker
falls back to a bundled list of current GLM models.

GLM selects thinking with `thinking.type` rather than `reasoning_effort`, and the
GLM-5.3 series only accepts `low`, `high`, or `max`. `/thinking` therefore maps
`low` to `low`, `medium` to `high`, and `high` to `max`; `off` disables thinking
where the model allows it and asks for the lowest effort on GLM-5.3, which always
thinks. GLM prices ship in the built-in table, so the footer's `$cost` works out
of the box.

For the mainland-China BigModel endpoint
(`https://open.bigmodel.cn/api/paas/v4`), set `ZAI_BASE_URL` or `base_url`.

## Context files and the system prompt

Oxide loads `AGENTS.md` (or `CLAUDE.md`) as project instructions by walking every
ancestor directory from the filesystem root down to the working directory, so
nested projects layer their instructions. If a directory contains
`AGENTS.override.md`, it replaces `AGENTS.md`/`CLAUDE.md` for that directory
only. The global `~/.oxide/AGENTS.md` is loaded first (lowest precedence).

- Disable ancestor context-file discovery with `--no-context-files`.
- Replace the default system prompt with `.oxide/SYSTEM.md` (project),
  `~/.oxide/SYSTEM.md`, or the corresponding platform config-directory file.
  Append without replacing with `APPEND_SYSTEM.md` in the same locations.
- `--system-prompt <text>` replaces the configured base prompt for one run, and
  `--append-system-prompt <text>` appends to that base (repeatable). A loaded
  `SYSTEM.md` remains the higher-precedence replacement.
- The startup welcome area lists loaded context files, and `/reload` re-reads
  them.

The composed prompt carries a `# Workspaces` section naming the working directory
and the other folders added to Oxide (the desktop's project list,
`desktop/projects.json`), each with its path, so a repository elsewhere on the
machine is found from that list instead of a `find ~` that reads every unrelated
file before the command times out. It says the file tools take absolute paths, so
a file in a sibling project can be read, searched, and edited from the run, while
`bash` still starts in the current project's root. The section is present in every
front-end — the terminal, the desktop app, and the VS Code panel — since it is
composed by the shared core.

## Project trust

Projects may contain local resources that change how the agent behaves or execute
code — agents, commands, prompts, skills, plugins, `SYSTEM.md`, and
`APPEND_SYSTEM.md`. Oxide treats the presence of any of these as requiring trust.
When a project requires trust and no decision has been saved for it (or a parent
directory), the TUI asks before loading them.

- `defaultProjectTrust` in `settings.json` controls the fallback: `ask`
  (default), `always`, or `never`.
- `--approve`/`-a` trusts project resources for one run; `--no-approve` ignores
  them.
- `/trust [show|off]` saves a decision for the current directory to `trust.json`.
- Non-interactive modes (`-p`, `--mode json`, `--mode rpc`) never prompt: with
  the `ask` or `never` setting they ignore project resources unless approved.
- Context files (`AGENTS.md`/`CLAUDE.md`) always load, trusted or not.

## Themes

Oxide ships `dark` and `light` themes. Add a custom theme as JSON under
`.oxide/themes/<name>.json` (project) or `<config>/Oxide/themes/<name>.json`
(global), then select it with `--use-theme <name>` or `/theme <name>`. The
built-in palettes come from `oxide_core::theme_view`, shared with the desktop
app, so the CLI and desktop use identical colors; custom theme files are read by
both. Colors accept names (`cyan`, `lightblue`) or `#rrggbb`; unspecified slots
fall back to the built-in `dark` theme:

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
`info`, `dim`, `border`, `tool_pending_bg`, `tool_success_bg`, `tool_error_bg`,
`usage_bar_bg`, `usage_bar_fg`, `usage_bar_label`, `thinking_off`,
`thinking_low`, `thinking_medium`, `thinking_high`, `thinking_text`.

Theme slots are semantic: `accent` marks focus and selections, `user` and
`assistant` label speakers, `success` and `error` communicate outcomes, `tool`
marks active tool work, and `info`/`dim` render supporting text. The `tool_*_bg`
slots fill the background behind a tool's header, output, and `Took`/`Elapsed`
footer (pending while running, success or error once it settles). Selection rows
also use reverse video and outcomes include text or symbols, so meaning does not
depend on color alone. For accessible custom themes, keep every foreground
readable against the terminal background and avoid assigning the same color to
`success`, `error`, and `tool`. The built-in themes use fixed `#rrggbb` colors
(not the terminal's own ANSI palette) so they match the desktop app exactly.

## Sessions and context

Sessions use Pi's on-disk format: an append-only JSONL tree whose first line is a
`session` header and whose remaining lines are `message`, `compaction`,
`branch_summary`, `session_info`, `model_change`, and `thinking_level_change`
entries linked by `id`/`parentId`. The last entry is the active leaf; the model
sees the leaf path with the latest compaction applied. Branching moves the leaf
back and appends a `branch_summary`, so in-file alternatives are preserved.

### Context compaction

Oxide compacts Pi-style once the outgoing context approaches the model window. It
walks back from the newest message until `keepRecentTokens` is reached and
summarizes the older span into a structured handoff (goal, progress, decisions,
next steps, critical context, plus cumulative read/modified file lists), keeping
the most recent tokens verbatim. A cut never separates a tool call from its
result.

Settings live under `compaction` in `settings.json` (global) or
`.oxide/settings.json` (project):

```json
{
  "compaction": {
    "enabled": true,
    "reserveTokens": 16384,
    "keepRecentTokens": 20000,
    "modelOverrides": { "openai/gpt-4o": { "reserveTokens": 400000 } }
  }
}
```

Compaction triggers above `contextWindow - reserveTokens`, where the window is
`OXIDE_CONTEXT_LIMIT` or the model default. `OXIDE_COMPACTION_ENABLED`,
`OXIDE_COMPACTION_RESERVE_TOKENS`, and `OXIDE_COMPACTION_KEEP_RECENT_TOKENS`
override the file settings. Each compaction is appended to the session log as a
`compaction` entry anchored at `firstKeptEntryId` and replayed on resume, so the
model sees the same compacted view. Manual compaction is `/compact [focus]` in
the TUI or `oxide sessions compact`.

Branching summarizes the path being abandoned with the same structured format and
appends it as a `branch_summary` entry. `/tree <n>` branches the current session
in place (alternatives stay in the file); `/fork <n>` creates a new session
seeded with the summary.

## Data locations

Runtime state lives under the platform Oxide config directory:

- Main configuration: `config.json`
- Credentials: `auth.json`
- Cached provider model lists: `model-cache.json` (refreshed after 24 hours)
- MCP OAuth tokens: `mcp-oauth/<server>.json` (mode `0600`)
- Sessions: `sessions/<project>/<timestamp>_<id>.jsonl` (Pi-style entry trees)
- Snapshots: `snapshots/<project>/` (bare git repo)
- Memory: `memory/`
- Project trust: `trust.json`
- Plugins: `plugins/` (installed plugin packages, marketplaces, and state)
- Portkey usage bar: `portkey-usage.json` (mode `0600`; see `OXIDE_USAGE_FILE`)
- Settings: `settings.json` (e.g. `defaultProjectTrust`, `compaction`,
  `modelPrices`, `hideThinkingBlock`)
- Themes: `themes/<name>.json`
- Desktop projects: `desktop/projects.json` (folders added to the desktop
  sidebar)
- Approvals: `approvals.json` (tools allowed without prompting, per project;
  shared by the terminal, the desktop app and the VS Code extension)
- Truncated tool output: `truncated/` (retained 7 days; see
  `OXIDE_TRUNCATION_DIR`)
- Context compaction config: `compaction` in `settings.json` /
  `.oxide/settings.json`

Global ecosystem resources such as agents, commands, prompts, skills, plugins,
and MCP definitions may also live under `~/.oxide/`; compatibility resources are
read from `~/.claude/` and `~/.claude.json`.
