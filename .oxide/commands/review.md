---
description: Review the current diff for correctness, regressions, and security issues.
---

Review the current working-tree changes for the `oxide` project.

Use `git diff` (and `git diff --staged` when relevant) to gather the changes,
then inspect the affected code and tests. Report only actionable findings with
file and line references, ordered by severity. If there are no findings, say so
and identify any validation gaps. Scope or focus: $ARGUMENTS
