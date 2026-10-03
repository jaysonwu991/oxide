# Features

The full feature list. The [README](../README.md) keeps a short summary; this
page is the detail behind it, and the task-by-task guides live in
[cli.md](cli.md), [desktop.md](desktop.md), and [vscode.md](vscode.md).

## Interfaces

- Interactive TUI (ratatui) plus non-interactive `-p/--print`, `--mode json`
  (JSONL event stream), and `--mode rpc` (JSONL over stdin/stdout) modes. In the
  TUI, `/login` (`/connect`) and `/logout` manage provider credentials.
- Desktop app (`oxide-desktop`, Tauri + Rust) that manages multiple projects
  and shows the shared session store, using the same configuration as the CLI
  (see [desktop.md](desktop.md)).
- VS Code extension (`editors/vscode`) that drives the same `oxide` binary from
  a chat panel in the activity bar, with editor actions for the selection and
  the CLI's own sessions and configuration (see [vscode.md](vscode.md)).

## Models and providers

- OpenAI-compatible (OpenAI, DeepSeek, Portkey, Z.AI/GLM, custom) and Anthropic
  Messages API clients.
- Reasoning effort: `auto` (default), `off`, `low`, `medium`, or `high`, cycled
  in the TUI with Shift+Tab or set with `--reasoning` / `OXIDE_REASONING`.
  `auto` uses the provider/model's native behavior; explicit levels map to
  OpenAI-compatible effort, Anthropic adaptive thinking, or legacy extended
  thinking as appropriate.
- Resilient streaming: transient failures (network errors, truncated streams,
  429, and 5xx responses) and a turn that comes back with neither text nor tool
  calls are retried with backoff. A stream that drops after it has already
  emitted part of the reply is retried too — the failed attempt is discarded so
  the retry streams fresh instead of extending the partial text — and only
  after that budget is exhausted do empty or truncated responses and in-band
  stream errors surface as errors instead of silently ending the turn. Text the
  last attempt streamed is kept in the session, so the next message can continue
  from what you already saw.
- Multimodal prompts: attach images/PDFs with `--image` or `@path` references,
  and pass prompt files as `oxide @file "message"`.

## Tools and the agent loop

- Built-in tools under Pi-style names: `read`, `write`, `edit`, `bash`, `grep`,
  `find`, `ls`, `webfetch`. Compatibility names (`read_file`, `write_file`,
  `list_dir`, `glob`) and the unified-diff `patch` tool are accepted everywhere,
  including in permission rules. See [tools.md](tools.md).
- Agent-level tools: `task` (subagents), `skill` (on-demand skill loading),
  `command` (model-invoked commands), `memory` (cross-session notes),
  `diagnostics` (LSP diagnostics), and `ask` (a question put to the user by a
  front-end that can answer it).
- MCP servers over stdio or Streamable HTTP, loaded on demand with automatic
  tool selection, OAuth discovery, and session handling, exposed as
  `<server>__<tool>`.
- Agent-harness niceties: read-only tool calls in a batch run in parallel while
  preserving model order, `bash` output streams into the UI as it arrives, and
  typing while the agent works can queue or steer it between steps. Enter safely
  queues a follow-up while busy; Alt+Enter deliberately steers the active
  response.
- Read-only runs via the tool allowlist, e.g.
  `oxide -t read,grep,find,ls -p "review this"`.
- Tool selection: `--tools`/`-t` allowlists and `--exclude-tools`/`-x` disables
  tools (accepting both Pi and legacy names); disabled tools are hidden from the
  model and refused if requested.
- LSP diagnostics via rust-analyzer, typescript-language-server, pyright, gopls;
  a language server that crashes or closes its pipe is evicted and respawned on
  the next edit instead of poisoning every later call with a broken pipe.

## Evidence-based completion

The system prompt's Definition of Done requires the model to confirm the outcome
of any state-changing action before claiming success — edits on disk and
builds/tests, a pull request's CI and mergeability, posted comments and reviews,
releases, and deployments. A code change made in response to a pull request or
review is not delivered until it is committed and pushed to the branch under
review, so a review reply is held until the fix is pushed and an uncommitted fix
is never reported as an addressed review. A reply goes into the review comment's
own thread (`gh api -X POST
repos/{owner}/{repo}/pulls/<n>/comments/<comment_id>/replies`) rather than as one
general comment on the pull request; `gh pr comment` is for a new top-level
comment. A summary about a pull or merge request names it with a link the user
can click — `[#123](https://github.com/owner/repo/pull/123)`, the URL the forge
CLI printed, not a bare `#123` — beside its state, since the desktop app and the
VS Code panel open a Markdown link in the browser and a terminal links the URL
printed after the label. A commit is named that way too — `[<short sha>](<url>)`
in a report, a reply to a review comment or any other summary — with the URL
the forge reports for that commit rather than a template filled in by hand:
`gh api repos/{owner}/{repo}/commits/<sha> --jq .html_url` answers GitHub's
`https://github.com/owner/repo/commit/<sha>`, and a GitLab project answers its
own `https://gitlab.com/owner/repo/-/commit/<sha>` on whatever domain it lives
on, since a hash on its own is not turned into a link by the forge: a reply
that says which commit addressed a comment carries the link rather than the
hash alone. A run that tries to finish
with unconfirmed edits or unchecked side effects gets one hidden reminder to verify
before it can summarize; a reminder the provider answers with nothing ends the
run with the summary the model already wrote, rather than reporting an empty
response on top of a finished answer. An explicit instruction from the user
outranks those checks: ask for a pull request with "no need to check the PR's
status" and none of it is run, and the reply says plainly what was left
unverified instead.

A separate Scope rule keeps commits limited to the task: blanket staging
(`git add -A`, `git commit -a`) is held until the model reviews the staged files,
so local-only files like `.claude/settings.local.json` stay out of the PR.

## Project configuration and ecosystem

- Project + global ecosystem discovery: instructions, commands, prompt
  templates, agents, skills, MCP servers, and plugins from `.oxide/` (plus the
  Claude Code layout).
- Plugin hooks (`tool.execute.before` / `tool.execute.after`, plus `status` for
  a footer status row) run under bun/node, and an `after` hook can end the turn
  by setting `output.terminate = true`.
- Claude Code-style plugin packages and marketplaces: install plugins that
  bundle commands, agents, skills, MCP servers, and command hooks behind a
  `.oxide/plugin.json` (or `.claude-plugin/plugin.json`) manifest, from a
  marketplace declared by `.oxide/marketplace.json` (or
  `.claude-plugin/marketplace.json`). MCP servers can come from the manifest's
  `mcpServers` or a plugin-root `.mcp.json`. Manage them with `oxide plugin` and
  `/plugins`, or browse marketplaces and their plugins interactively with
  `/marketplaces`.
- Project trust: project-local resources (agents, commands, prompts, skills,
  plugins, `SYSTEM.md`) load only after the project is trusted; decisions are
  saved per directory in `trust.json`, `defaultProjectTrust` sets the fallback,
  `--approve`/`-a` and `--no-approve` override for one run, and `/trust` saves a
  decision.
- Themes: built-in `dark` and `light` plus custom `.oxide/themes/<name>.json`,
  selected with `--use-theme` or `/theme`. The built-in palettes are shared with
  the desktop app, so the CLI and desktop render the same colors.

## Sessions and context

- Pi-compatible sessions stored as JSONL trees (`id`/`parentId` entries with
  in-file compaction and branch summaries), shadow-git snapshots (`/undo`,
  `/redo`), Pi-style context compaction that runs automatically near the model
  window (`/compact`), and branch summarization when branching (`/tree <n>`,
  `/fork <n>`).
- Pi-style session flags: `--session <path|id>`, `--no-session`, `--name`,
  `-c`/`--continue`, `-r`/`--resume` (browse past sessions), and
  `--fork <path|id>`, plus TUI commands `/new`, `/session`, `/resume`, `/name`,
  `/model`, `/thinking`, `/export`, `/reload`, and `/hotkeys`.
- Session branching: `/tree` lists user messages and `/tree <n>` branches the
  current session in place (summarizing the abandoned path), `/fork <n>`
  branches a new session from one, and `/clone` duplicates the current session.

## Terminal experience

- Compact, bounded output: tool bodies render as background-filled panels with
  a short, readable preview by default (compact JSON is expanded, and long
  output is cut to a per-tool budget: shell tail 5 lines, `read` 10, `grep` 15,
  `find`/`ls` 20), colored by state (pending, success, or error), with long
  action lines and wrapped output continuations aligned so the full text stays
  readable, and tool results are capped by lines and bytes before they enter the
  model's context. Capped output is saved to disk with a pointer so it stays
  recoverable.
- Compact agent transcript: shell calls render as `→ Run <command>` and finish
  as `→ Ran <command> · exit <code>` (`→ Run failed …` on a non-zero exit), with
  a `(timeout Ns)` hint when the call sets one, a live `Elapsed Ns` while it
  runs, and a `Took Nms` duration afterwards (any other tool that runs for at
  least 500 ms is timed too). Long output is previewed with a
  `⋯ <lines> more/earlier lines · Ctrl+O to expand` affordance, `read` results
  preview the file contents, file edits show a colored line-numbered diff, user
  and assistant turns render their label inline with the message text, and the
  system prompt nudges the model to batch reads instead of re-reading the same
  paths.
- Visible work in progress: reasoning streams into the transcript as a muted
  italic `✦ Thinking` block that closes with `✦ Thought for 1.4s`, so work done
  before the answer is no longer invisible. Ctrl+T collapses reasoning blocks to
  their label (`· Ctrl+T to expand`); set `hideThinkingBlock` in `settings.json`
  to start collapsed, as in Pi. A running `task` subagent reports its own
  progress — a live `Elapsed` on any tool panel, a
  `↳ <agent> · <activity> · <n> call(s)` line, and a status row naming the
  subagent and its current tool. The status row also reports the phase of the
  current step (`thinking...`, `running tool...`, `compacting...`,
  `summarizing branch...`) and surfaces stream retries as
  `retrying (n/3) in Ns...`.
- Focused terminal layout: the welcome banner stacks the block-letter `OXIDE`
  wordmark above a short summary of the loaded ecosystem, context files, and
  MCP/plugin/memory state; current activity and elapsed time live in the status
  row, and the footer shows the abbreviated working directory with the git
  branch and session name, cumulative usage (`↑`/`↓`, `R`/`W` cache tokens and
  `CH` hit rate when reported, `$cost` from the model price table, including
  summary generation), context usage as `%`/window with an `(auto)` marker, and
  the right-aligned model and thinking level; plugins can add a third status
  row, and the Portkey spend bar adds a final one when it is enabled. The editor
  matches Pi: full-width top and bottom rules colored by the thinking level that
  grow to 12 rows, and semantic colors keep dark, light, and custom themes
  consistent. See [tui.md](tui.md).
- Portkey spend bar: with a Portkey login, `/usage` opens a settings dialog that
  adds a full-width bar at the bottom of the screen showing the user, this
  session's cost, and today's and the month's spend from the Portkey analytics
  API against an optional monthly budget in `$` or `¥`.
- Desktop notifications: a finished agent turn raises a system toast with a
  short snippet of the reply and plays the platform alert sound (Notification
  Center on macOS, `notify-send` plus the freedesktop `complete` sound on Linux,
  a Windows toast), so long runs can finish while you are in another window;
  internal work like `/compact` stays silent. Enabled by default and tuned with
  `/notify [on|off]` and `/notify sound [on|off]` in the TUI, the
  `notifyOnComplete` / `notifySound` keys in `settings.json`, or
  `OXIDE_NOTIFY_ON_COMPLETE` / `OXIDE_NOTIFY_SOUND`.
