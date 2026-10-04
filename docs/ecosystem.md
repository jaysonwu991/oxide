# Ecosystem layout

What Oxide discovers from a project and from your global scope, and the Claude
Code layout it also reads. For step-by-step instructions on adding each kind of
resource, see [cli.md](cli.md).

Oxide discovers configuration from the project root (the nearest ancestor
containing `.git`, `.oxide`, or `.claude`) and the user's global scope. Project
entries override global entries with the same name, and the Oxide layout overrides
the Claude Code layout.

## Oxide layout

- `AGENTS.md` — project memory and instructions
- `.oxide/AGENTS.md` — additional layout-scoped instructions
- `.oxide/agents/*.md` — subagents (frontmatter: `name`, `description`, `mode`,
  `tools`, `permission`); subagents may spawn subagents one level deep
- `.oxide/commands/*.md` — slash commands (`$ARGUMENTS`, `$1`, `$2`, …; optional
  `agent` and `subtask` frontmatter)
- `.oxide/prompts/*.md` — prompt templates (Pi-style; frontmatter `description`
  and `argument-hint`, arguments `$1`, `$@`, `${1:-default}`, `${@:2:3}`)
- `.oxide/skills/*/SKILL.md` — on-demand skills
- `.oxide/themes/*.json` — TUI color themes (built-in `dark`/`light` plus custom)
- `.oxide/plugins/` — JS/TS plugin hooks
- `.oxide/SYSTEM.md`, `.oxide/APPEND_SYSTEM.md` — replace or extend the system
  prompt
- `.oxide/mcp.json` — MCP servers (same schema as `.mcp.json`; manage with
  `oxide mcp`)
- Global scope: `~/.oxide/` and the platform Oxide config directory (the latter
  has higher precedence)

This repository keeps its own agents, commands, prompts, skills, and plugins in
`.oxide/`.

## Command routing

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

## Claude Code compatibility

Oxide also reads the Claude Code layout, so existing configurations work as-is:

- `CLAUDE.md` — project memory and instructions
- `.claude/CLAUDE.md` — additional layout-scoped instructions
- `.claude/agents/`, `.claude/commands/`, `.claude/skills/`, `.claude/plugins/`
- `.mcp.json` — MCP servers
- Global scope: `~/.claude/`, `~/.claude.json`

## Slash commands

Slash commands are expanded from the ecosystem and also include built-ins:
`/help`, `/hotkeys`, `/exit`, `/new`, `/session`, `/tree`, `/fork`, `/clone`,
`/name`, `/model`, `/reasoning`, `/theme`, `/trust`, `/export`, `/reload`,
`/init`, `/connect`, `/logout`, `/models`, `/mcp`, `/plugins`, `/marketplaces`,
`/notify`, `/permissions`, `/usage`, `/spend`, `/undo`, `/redo`, `/compact`,
`/copy`, `/copy all`, and `/skill:<name>`. Each command has exactly one
spelling: a name the catalog does not declare is a plain message rather than a
second way to reach a command.
Discovered
commands and prompt templates can also be invoked by the agent through the
`command` tool, and skills load on demand with `skill` or via `/skill:<name>`.

## Plugin packages

Installed via `oxide plugin` (or `/plugins`), plugin packages bundle commands,
agents, skills, MCP servers, and command hooks behind a `.oxide/plugin.json` or
`.claude-plugin/plugin.json` manifest, discovered from marketplaces declared by
`.oxide/marketplace.json` or `.claude-plugin/marketplace.json`. They load at
startup before project resources, so project entries still override plugins with
the same name. Use `/marketplaces` to browse marketplaces and install, enable, or
remove their plugins interactively. See
[Plugins and hooks](cli.md#plugins-and-hooks).
