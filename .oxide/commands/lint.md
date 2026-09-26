---
description: Run formatting and clippy with warnings denied, then fix issues.
agent: build
---

Lint the `oxide` workspace.

1. Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings`.
2. Fix every clippy warning with the smallest idiomatic change.
3. Re-run both until clean, then run `cargo test`.

Scope: $ARGUMENTS
