# How Oxide compares

Oxide is a small, native terminal agent that deliberately borrows the Claude
Code configuration layout so existing `.claude/` setups keep working. The table
below compares the high-level shape of the five tools; feature sets move fast,
so check each project's documentation for the current details.

| Capability | Oxide | [Claude Code](https://docs.claude.com/en/docs/claude-code/overview) | [Codex](https://github.com/openai/codex) | [Pi Coding Agent](https://github.com/earendil-works/pi) | [OpenCode](https://opencode.ai) |
| --- | --- | --- | --- | --- | --- |
| Distribution | Native Rust core + Electron desktop app (TypeScript window over the Rust engine) + VS Code extension | Proprietary CLI + apps | Open-source CLI (Rust) + IDE extension | Open-source TypeScript CLI (npm, install script, Nix) with RPC mode and a TypeScript SDK | Open-source CLI (Node/Bun) |
| License | MIT | Proprietary | Apache-2.0 | MIT | Open source |
| Model providers | 35 built-in providers (OpenAI-compatible incl. the Portkey gateway, Anthropic Messages, Gemini/Vertex, Bedrock, Azure, GitHub Copilot, GitLab Duo) + any custom endpoint | Claude (Anthropic API, Bedrock, Vertex, third-party) | OpenAI models (GPT-5-Codex family) + custom providers | Many hosted providers by OAuth or API key (Anthropic, OpenAI, Gemini, Bedrock and Vertex with ambient credentials, and more) + custom endpoints and llama.cpp | Any provider (bring your own keys) |
| Interfaces | Terminal TUI, `-p` print, JSON/RPC modes, desktop app, VS Code extension | Terminal, IDE, desktop, web | Terminal CLI, IDE (VS Code, Cursor) | Interactive TUI, print, JSON event stream, RPC, TypeScript SDK | Terminal, desktop, IDE, web |
| Project config | `.oxide/` + `AGENTS.md` (also reads `.claude/`) | `CLAUDE.md` + `.claude/` | `AGENTS.md` + `~/.codex/config.toml` | `.pi/` + `AGENTS.md` (also reads `CLAUDE.md`) | `opencode.json` + `AGENTS.md` |
| Subagents | `--agent`, `task`, command routing | Subagents, background agents | Subagents | — (left to extensions and packages) | Agents |
| Reasoning effort | `auto` / `off` / `minimal` / `low` / `medium` / `high` / `xhigh` / `max`, narrowed to the levels the model's own listing or the pi.dev catalog advertises (Shift+Tab, `--reasoning`) | Extended thinking | `--reasoning-effort` (model-dependent) | `off` / `minimal` / `low` / `medium` / `high` / `xhigh` / `max`, cycled with Shift+Tab and settable per model | Model-dependent |
| Slash commands | `.oxide/commands` + `.oxide/prompts`, `agent`/`subtask` routing | Commands | Commands (`~/.codex/prompts`, `prompts/`) | Slash commands + prompt templates (`.pi/prompts`), commands from extensions | Commands |
| Skills | `SKILL.md` | Skills | `SKILL.md` | `SKILL.md` (`.pi/skills`, `.agents/skills`) | Agent Skills |
| MCP servers | stdio + HTTP + OAuth, managed with `oxide mcp` | MCP servers | MCP servers (`codex mcp`, config.toml) | stdio + streamable HTTP + OAuth, managed with `pi mcp` and `/mcp` | MCP servers |
| Plugins / hooks | Hooks + plugin packages & marketplaces | Hooks, plugins, Agent SDK | — | Extensions (in-process TypeScript hooks) + packages from npm or git | Plugins |
| LSP diagnostics | Built in (rust-analyzer, TS, pyright, gopls) | — | — | — | Built in (LSP servers) |
| Undo file changes | Shadow-git `/undo`, `/redo` | Git / checkpoints | Git checkpoints (`codex checkpoint`) | — (session branches undo conversation, not files) | `/undo`, `/redo` |
| Sessions | Pi-style JSONL trees, `-c` / `-r`, `/session` / `/tree` / `/fork` / `/clone`, pin/archive in the shared store | Sessions across surfaces | Sessions (`codex --resume`) | JSONL trees, `--continue` / `--resume`, `/tree` / `/fork` / `/clone`, `/export` / `/share` | Sessions, share links |
| Project trust | `trust.json`, `--approve` / `/trust` | — | Sandbox + approval modes | `trust.json`, `--approve` / `/trust`, `defaultProjectTrust` | — |
| Themes | Built-in `dark` / `light`, custom `.oxide/themes` | — | Built-in themes (`codex themes`) | Themes (`.pi/themes`, packaged) | Themes |
| Context management | Auto-compaction + branch summarization | Auto-compaction | Auto-compaction | Auto-compaction + branch summarization, `/compact` with instructions | Auto-compaction + DCP plugin |
| Multimodal input | Images and PDFs (`--image`, `@path`) | Images | Images | Images (paste or drag) and files | Images |

A dash indicates no first-class built-in equivalent. Where Oxide differs most:
it is a dependency-light Rust core with a terminal binary, an Electron desktop
app whose window drives that same core, and a VS Code extension, it speaks the
OpenAI-compatible, Anthropic, Gemini, Bedrock and Azure APIs directly, its
plugin packages reuse the same on-disk commands, agents, skills, and MCP servers
the ecosystem already reads, and it is compatible with the Claude Code on-disk
layout while using its own `.oxide/` format.

Pi Coding Agent is the one Oxide was modelled on: the JSONL session trees,
tool names, context-window rules, compaction format and thinking levels all
follow it, and the table above is where the two stop agreeing — Oxide adds
the desktop app, the VS Code panel, subagents, LSP diagnostics, shadow-git
undo, and a provider table with its own logins, while Pi keeps the harness
minimal and leaves subagents, plan mode and file checkpoints to extensions
and packages.
