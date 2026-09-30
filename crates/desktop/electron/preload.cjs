const { contextBridge, ipcRenderer } = require("electron");

const COMMANDS = new Set([
  "list_projects",
  "add_project",
  "create_project",
  "pick_folder",
  "remove_project",
  "list_sessions",
  "all_sessions",
  "project_info",
  "set_project_trust",
  "mcp_servers",
  "set_mcp_server",
  "list_commands",
  "at_suggestions",
  "session_messages",
  "rename_session",
  "delete_session",
  "list_providers",
  "login",
  "logout",
  "send_prompt",
  "cancel_run",
  "steer_run",
  "undo_turn",
  "change_sides",
  "resolve_approval",
  "resolve_question",
  "list_approvals",
  "clear_approvals",
  "list_models",
  "set_model",
  "list_themes",
  "theme_colors",
  "set_theme",
  "open_url",
]);

const EVENTS = new Set([
  "agent-start",
  "agent-event",
  "agent-end",
  "approval-request",
  "question-request",
  "question-closed",
  "host-error",
]);

contextBridge.exposeInMainWorld(
  "__OXIDE__",
  Object.freeze({
    invoke(command, args = {}) {
      if (!COMMANDS.has(command)) return Promise.reject(new Error(`Unknown command: ${command}`));
      return ipcRenderer.invoke("oxide:invoke", { command, args });
    },
    async listen(eventName, handler) {
      if (!EVENTS.has(eventName)) throw new Error(`Unknown event: ${eventName}`);
      const listener = (_event, message) => {
        if (message?.event === eventName) handler({ payload: message.payload });
      };
      ipcRenderer.on("oxide:event", listener);
      return () => ipcRenderer.removeListener("oxide:event", listener);
    },
  }),
);
