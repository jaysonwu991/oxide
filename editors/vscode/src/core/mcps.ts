// The MCP server list, read from the CLI's own `oxide mcp list --json`.
//
// The listing is the one the terminal's `/mcp` draws and the desktop app's MCP
// dialog shows, so the picker here reports the same servers, the same states and
// the same source labels without the extension having to read `mcp.json` itself
// (or know where each scope keeps it).

/// One server as `oxide mcp list --json` serializes it.
export interface McpServerView {
  name: string;
  /// `http`, `stdio`, or `unknown`.
  transport: string;
  /// The URL (with `(oauth)`) or the command line.
  detail: string;
  /// The scope label of the file that defines it, e.g. `global` or `project`.
  source: string;
  /// `project` or `global`: what `--scope` accepts, for a pinned toggle.
  scope: string;
  /// Whether the runtime connects it.
  enabled: boolean;
  /// `connected`, `needs-auth`, `needs-trust`, `disabled`, or `error`.
  state: string;
  /// Display form: `Connected`, `Needs Auth`, or `Error: <detail>`.
  status: string;
}

export type McpState = "connected" | "needs-auth" | "needs-trust" | "disabled" | "error";

/// Parses the listing. A CLI that predates `--json` prints its human table, so
/// a body that is not an array of objects yields nothing rather than throwing
/// in the middle of a picker.
export function parseMcpList(output: string): McpServerView[] {
  const text = output.trim();
  if (!text) return [];
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return [];
  }
  if (!Array.isArray(value)) return [];
  const servers: McpServerView[] = [];
  for (const entry of value) {
    if (!entry || typeof entry !== "object") continue;
    const record = entry as Record<string, unknown>;
    const name = typeof record.name === "string" ? record.name : "";
    if (!name) continue;
    servers.push({
      name,
      transport: stringOf(record.transport),
      detail: stringOf(record.detail),
      source: stringOf(record.source),
      scope: stringOf(record.scope).toLowerCase() || "project",
      enabled: record.enabled !== false,
      state: stringOf(record.state) || (record.enabled === false ? "disabled" : "unknown"),
      status: stringOf(record.status),
    });
  }
  servers.sort((left, right) => left.name.localeCompare(right.name));
  return servers;
}

function stringOf(value: unknown): string {
  return typeof value === "string" ? value : "";
}

/// The word a state is painted with, so the dialog reads at a glance the way the
/// terminal's colored status does. The webview cannot draw a codicon, so the
/// state is a label colored by `dialogTone` in the dialog itself.
export function mcpStateLabel(state: string): string {
  switch (state) {
    case "connected":
      return "Connected";
    case "needs-auth":
      return "Needs auth";
    case "needs-trust":
      return "Needs trust";
    case "disabled":
      return "Disabled";
    case "error":
      return "Error";
    default:
      return state || "Unknown";
  }
}

/// The arguments that turn a server on or off. The scope pins the change to the
/// file that defines the server, so a name in both scopes toggles the one that
/// was listed rather than whichever the lookup finds first.
export function mcpToggleArgs(server: McpServerView, enabled: boolean): string[] {
  return ["mcp", enabled ? "enable" : "disable", server.name, "--scope", server.scope];
}

/// The arguments that list the servers as JSON, for the project the command
/// runs in.
export function mcpListArgs(): string[] {
  return ["mcp", "list", "--json"];
}
