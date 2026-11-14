# Jira

## Read and search

Resolve the site and work item once, then reuse them. Use a narrow JQL query and
request only the fields needed. Paginate large searches instead of raising the
page size. Read the work item before editing it, including the fields and
metadata for the operation.

## Create and edit

For creation, read the create metadata for the selected project and work-item
type so required fields and allowed values come from Jira. Use account IDs from
`lookupJiraAccountId`; do not guess a user from a display name.

For an edit, call `editJiraIssue` with only the requested field changes. Do not
send the complete issue back: omitted fields must remain untouched, and
descriptions must not be regenerated unless requested. Read the editable
metadata when a field ID or allowed value is uncertain.

For comments, use `addOrEditJiraIssueComment`; when editing, include the existing
comment ID and change only the requested text. Read the comment back.

For status changes, read `listJiraIssueTransitions` and call
`transitionJiraIssue` with the transition ID. Do not edit status through a
generic field update. Preserve unrelated fields during the transition.

After a mutation, read the work item or comment back and verify the key, changed
fields, comment ID, and status. Report failures per item rather than treating an
HTTP 200 response as proof that a field changed.
