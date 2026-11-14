---
name: atlassian
description: Use for Confluence pages or Jira work items, especially safe incremental edits, full-width pages, ticket transitions, and Mermaid or draw.io diagrams.
---

# Atlassian

Complete the requested Confluence or Jira operation through the configured
Atlassian MCP server. Read the current state before every mutation.

- Load [references/confluence.md](references/confluence.md) for page reads,
  creates, edits, layout, or diagrams.
- Load [references/jira.md](references/jira.md) for work-item search, creation,
  field edits, comments, or transitions.

Use only tools and arguments present in the live schema. Atlassian MCP v2
advertises primary tools such as `getConfluenceContent`,
`createConfluenceContent`, `updateConfluenceContent`, `searchConfluence`,
`getJiraIssue`, `createJiraIssue`, `editJiraIssue`, and
`addOrEditJiraIssueComment`; deferred capabilities use `discover` followed by
an `executeRead`, `executeWrite`, or `executeDestructive` call. Older or
community servers use different names. Inspect the available schema instead of
guessing an argument or mixing tool generations.

## Mutation safety

- Change only what the user requested. Preserve every unspecified field, page
  node, macro, attachment, comment, label, restriction, and layout setting.
- Prefer a granular edit or a partial field update. Do not replace a whole Jira
  issue or Confluence page merely to change one field, section, or diagram.
- Read a target immediately before editing it and include the current
  version/revision when the tool requires optimistic concurrency.
- If no safe granular operation exists, retain the complete current body, replace
  the smallest identifiable block, and send back all untouched content exactly.
- Never fall back to an unauthenticated REST request or a dummy write to probe
  the schema.
- Read back the changed target. Compare the intended fields or nodes and the new
  version, and report any unchanged content that could not be preserved.

## Token use

Use the directly declared primary tool when it already covers the task. Run one
capability search only when a needed operation is absent, then use the returned
schema without repeating discovery. Search before listing broad corpora, request
only the fields or expansion needed, and read bounded sections. Do not paste
whole API responses into the answer; summarize the resulting link, key, version,
and verified changes.

Creating or editing an Atlassian artifact does not by itself authorize sharing
it or notifying other people.
