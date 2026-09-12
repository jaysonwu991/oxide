---
description: Run the test suite and summarize failures.
agent: build
---

Run the `oxide` test suite.

1. Run `cargo test $ARGUMENTS`.
2. Summarize passed/failed counts.
3. For each failure, show the assertion and the smallest likely cause, then
   propose a fix. Do not edit production code without confirmation.

Extra arguments: $ARGUMENTS
