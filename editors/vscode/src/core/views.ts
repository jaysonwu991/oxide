// Where the chat lives in the workbench. These ids must match the views the
// manifest contributes in `package.json`: VS Code only resolves a webview view
// whose provider is registered under the id of a contributed view, so a rename
// on one side silently leaves an empty panel. `test/views.test.ts` holds the
// two files together.

/// The view contributed by the activity-bar container.
export const CHAT_VIEW = "oxide.chat";

/// A second copy of the same chat in the secondary side bar, the strip Copilot
/// Chat lives in. VS Code older than the `secondarySidebar` contribution point
/// ignores the container and with it this view, leaving `CHAT_VIEW` as the only
/// pane.
export const CHAT_VIEW_SECONDARY = "oxide.chatSecondary";
