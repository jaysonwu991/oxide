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
`{"type":"approval","id":…,"decision":"once"|"always"|"deny"}`, and
`{"type":"quit"}`) and writes its events to stdout. A tool a permission rule gates
runs unless the client passed `--ask-approvals`, which instead holds it and emits
an `approval_request` event for the client to answer; `always` is remembered per
project in the shared `approvals.json`, so the terminal and the desktop app stop
asking for that tool too. `--ask-questions` is the same bargain for the `ask`
tool — see [Questions a skill asks the user](cli.md#questions-a-skill-asks-the-user).

Non-interactive modes never prompt for project trust: with the `ask` or `never`
setting they ignore project resources unless `--approve` is passed.

## Session management

```sh
oxide sessions list                        # current project
oxide sessions list --all                  # every project
oxide sessions list --older-than 30        # stale sessions only
oxide sessions show <id>                   # print one saved conversation
oxide sessions delete <id>                 # delete one (prompts)
oxide sessions delete --older-than 30 --force
oxide sessions compact <id>                # summarize older history, keep recent tail
oxide sessions compact --all               # refresh every session in this project
oxide sessions merge <a> <b>               # concatenate two sessions into a new one
oxide sessions merge <a> <b> --summarize   # summarize the second session first
```

`delete` is token-free. `compact` and `merge --summarize` make one LLM
summarization pass per session so a stale session resumes from a small summary
plus its most recent messages instead of replaying the full transcript. `show`
takes `--tail <n>` for just the newest messages and `--json` for the object a
front-end draws. See
[Data locations and reset](cli.md#data-locations-and-reset).

## A turn's changes

A finished run can report what it changed, and a front-end that draws its own
diff reads the side that is nowhere on disk through the CLI:

```sh
oxide changes show src/main.rs --baseline <rev> --project <root>
oxide changes undo --baseline <rev> --after <rev> --project <root>
```

See [A turn's changes](cli.md#a-turns-changes).

## Other one-shot commands

```sh
oxide mcp list --json       # the server listing a front-end draws
oxide commands --json       # the slash-command catalog
oxide models --json         # normalized provider catalogs
oxide providers --json      # every provider, and which are stored or in use
oxide login openai --json --key-stdin <key   # connect one (key on stdin)
```
