---
description: Stage changes and create a conventional commit with a generated message.
agent: build
---

Create a git commit for the `oxide` project.

1. Inspect `git status` and `git diff` to understand what changed.
2. Stage only the files relevant to the change (never secrets or build output).
3. Draft a concise conventional-commit message (`type(scope): summary`) that
   matches the repository's style. Use `git log --oneline -10` as a reference.
4. Commit, then show the resulting `git log -1 --stat`.

Hints or scope from the user: $ARGUMENTS
