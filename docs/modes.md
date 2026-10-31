# Non-interactive modes and session management

Running Oxide without the TUI, and managing the sessions it stores. Flags are
listed in full in [configuration.md](configuration.md#cli-flags).

## Print, JSON, and RPC modes

```sh
oxide -p "summarize this repository"
oxide @prompt.md "answer this"           # include a file in the prompt
cat README.md | oxide -p "summarize"     # merge piped stdin
echo "explain src/main.rs" | oxide -p
oxide -p "review the diff" --image screenshot.png
oxide --mode json "list files"           # JSONL events on stdout
oxide --mode rpc                         # JSONL prompts over stdin
oxide --mode rpc --ask-approvals         # ...asking before a gated tool runs
oxide -t read,grep,find -p "review"      # read-only tool allowlist
oxide --session <id> -p "continue"       # reuse a specific session
oxide --fork <id> -p "try another path"  # branch a saved session
```

`print` mode writes a step's text once that step commits — at a tool call or the
end of the turn — so a stream the client retries never prints a partial answer
twice; `--mode json` still emits each delta as it arrives.

`--mode rpc` reads one JSON request per line (`{"type":"prompt","message":…}`,
`{"type":"steer","message":…}` or `{"type":"follow_up",…}` to add to the turn
that is running, `{"type":"approval","id":…,"decision":"once"|"always"|"deny"}`,
`{"type":"question","id":…,"answers":[…]}` (an empty list dismisses it),
`{"type":"abort"}`, and `{"type":"quit"}`) and writes its events to stdout. The
process serves any number of prompts, so a client can keep one per thread: the
MCP servers, plugin host and language servers are started once and reused, and
`abort` stops the running turn without ending the process (the next prompt
continues the same session). A tool a permission rule gates
runs unless the client passed `--ask-approvals`, which instead holds it and emits
an `approval_request` event for the client to answer; `always` is remembered per
project in the shared `approvals.json`, so the terminal and the desktop app stop
asking for that tool too. `--ask-questions` is the same bargain for the `ask`
tool — see [Questions a skill asks the user](cli.md#questions-a-skill-asks-the-user).

Non-interactive modes never prompt for project trust. A saved decision applies
first; without one, `ask` and `never` skip project resources. `--approve` trusts
the project resources for this run. Tools use the operating-system user's
filesystem permissions regardless of trust. The extension asks through its own UI and saves
the decision before starting its RPC process.

## Session management

```sh
oxide session list                         # current project
oxide session list --all                   # every project
oxide session list --older-than 30         # stale active sessions only
oxide session list --archived              # archived sessions for this project
oxide session list --json                  # shared metadata, including pinned
oxide session pin <id>                     # pin in Desktop and the extension
oxide session unpin <id>
oxide session archive <id>                 # hide it; keep the log
oxide session unarchive <id>               # restore it
oxide session show <id>                    # print one saved conversation
oxide session delete <id>                  # delete one (prompts)
oxide session delete --older-than 30 --force
oxide session compact <id>                 # summarize older history, keep recent tail
oxide session compact --all                # refresh every session in this project
oxide session merge <a> <b>                # concatenate two sessions into a new one
oxide session merge <a> <b> --summarize    # summarize the second session first
```

`delete` is token-free. `compact` and `merge --summarize` make one LLM
summarization pass per session so a stale session resumes from a small summary
plus its most recent messages instead of replaying the full transcript. `show`
takes `--tail <n>` for just the newest messages and `--json` for the object a
front-end draws. Every listing leaves out a session nothing was ever sent in —
a log a front-end opened and left has no title and nothing to resume — and
`/new` in the terminal starts one with the next message rather than with the
command. See
[Data locations and reset](cli.md#data-locations-and-reset).

Desktop and the extension offer Pin/Unpin and Archive. Archiving preserves the
session log and shows the CLI deletion command. `sessions delete --older-than 30`
also includes archived logs, and asks for confirmation unless `--force` is given.
Archiving and deleting session logs leave persistent memory intact.

## A turn's changes

A finished run can report what it changed, and a front-end that draws its own
diff reads the side that is nowhere on disk through the CLI:

```sh
oxide change show src/main.rs --baseline <rev> --project <root>
oxide change undo --baseline <rev> --after <rev> --project <root>
```

See [A turn's changes](cli.md#a-turns-changes).

## Other one-shot commands

```sh
oxide mcp list --json       # the server listing a front-end draws
oxide command --json        # the slash-command catalog
oxide model --json          # normalized provider catalogs
oxide context --json        # the window a run resolves for the active model
oxide provider --json       # every provider, and which are stored or in use
oxide login openai --json --key-stdin <key   # connect one (key on stdin)
oxide language --json       # the interface language and the catalog it draws
```
