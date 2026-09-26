---
description: Build the project and summarize compiler errors with suggested fixes.
agent: build
---

Build the `oxide` workspace.

1. Run `cargo build` (add `--release` if requested).
2. If it succeeds, report the artifact path and any warnings.
3. If it fails, group the errors by root cause and propose minimal fixes.

Extra arguments: $ARGUMENTS
