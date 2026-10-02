# How Oxide compares

Oxide is a small, native terminal agent that deliberately borrows the Claude
Code configuration layout so existing `.claude/` setups keep working. The table
below compares the high-level shape of the four tools; feature sets move fast,
so check each project's documentation for the current details.

| Capability | Oxide | [Codex](https://github.com/openai/codex) | [OpenCode](https://opencode.ai) | [Claude Code](https://docs.claude.com/en/docs/claude-code/overview) |
| --- | --- | --- | --- | --- |
| Distribution | Native Rust core + Tauri desktop app + VS Code extension | Open-source CLI (Rust) + IDE extension | Open-source CLI (Node/Bun) | Proprietary CLI + apps |
| License | MIT | Apache-2.0 | Open source | Proprietary |
| Model providers | OpenAI-compatible (OpenAI, DeepSeek, Portkey, Z.AI/GLM, custom) + Anthropic Messages API | OpenAI models (GPT-5-Codex family) + custom providers | Any provider (bring your own keys) | Claude (Anthropic API, Bedrock, Vertex, third-party) |
| Interfaces | Terminal TUI, `-p` print, JSON/RPC modes, desktop app, VS Code extension | Terminal CLI, IDE (VS Code, Cursor) | Terminal, desktop, IDE, web | Terminal, IDE, desktop, web |
| Project config | `.oxide/` + `AGENTS.md` (also reads `.claude/`) | `AGENTS.md` + `~/.codex/config.toml` | `opencode.json` + `AGENTS.md` | `CLAUDE.md` + `.claude/` |
| Subagents | `--agent`, `task`, command routing | Subagents | Agents | Subagents, background agents |
| Reasoning effort | `auto` / `off` / `low` / `medium` / `high` (Shift+Tab, `--reasoning`) | `--reasoning-effort` (model-dependent) | Model-dependent | Extended thinking |
| Slash commands | `.oxide/commands` + `.oxide/prompts`, `agent`/`subtask` routing | Commands (`~/.codex/prompts`, `prompts/`) | Commands | Commands |
| Skills | `SKILL.md` | `SKILL.md` | Agent Skills | Skills |
| MCP servers | stdio + HTTP + OAuth, managed with `oxide mcp` | MCP servers (`codex mcp`, config.toml) | MCP servers | MCP servers |
| Plugins / hooks | Hooks + plugin packages & marketplaces | — | Plugins | Hooks, plugins, Agent SDK |
| LSP diagnostics | Built in (rust-analyzer, TS, pyright, gopls) | — | Built in (LSP servers) | — |
| Undo file changes | Shadow-git `/undo`, `/redo` | Git checkpoints (`codex checkpoint`) | `/undo`, `/redo` | Git / checkpoints |
| Sessions | Pi-style JSONL trees, `-c` / `-r`, `/resume` / `/tree` / `/fork` / `/clone` | Sessions (`codex --resume`) | Sessions, share links | Sessions across surfaces |
| Project trust | `trust.json`, `--approve` / `/trust` | Sandbox + approval modes | — | — |
| Themes | Built-in `dark` / `light`, custom `.oxide/themes` | Built-in themes (`codex themes`) | Themes | — |
| Context management | Auto-compaction + branch summarization | Auto-compaction | Auto-compaction + DCP plugin | Auto-compaction |
| Multimodal input | Images and PDFs (`--image`, `@path`) | Images | Images | Images |

A dash indicates no first-class built-in equivalent. Where Oxide differs most:
it is a dependency-light Rust core with a terminal binary and a Tauri desktop
app, it speaks both the OpenAI-compatible and Anthropic APIs directly, its plugin
packages reuse the same on-disk commands, agents, skills, and MCP servers the
ecosystem already reads, and it is compatible with the Claude Code on-disk layout
while using its own `.oxide/` format.
