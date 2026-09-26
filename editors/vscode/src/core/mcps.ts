// The MCP server list, read from the CLI's own `oxide mcp list --json`.
//
// The listing is the one the terminal's `/mcps` draws and the desktop app's MCP
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

/// The codicon and label a state is painted with, so the picker reads at a
/// glance the way the terminal's colored status does.
export function mcpAppearance(state: string): { icon: string; label: string } {
  switch (state) {
    case "connected":
      return { icon: "$(pass-filled)", label: "Connected" };
    case "needs-auth":
      return { icon: "$(key)", label: "Needs auth" };
    case "needs-trust":
      return { icon: "$(shield)", label: "Needs trust" };
    case "disabled":
      return { icon: "$(circle-slash)", label: "Disabled" };
    case "error":
      return { icon: "$(error)", label: "Error" };
    default:
      return { icon: "$(question)", label: state || "Unknown" };
  }
}

/// The one-line description under a server's name: what it is, which file
/// defined it, and how it answered.
export function mcpDescription(server: McpServerView): string {
  const status = server.enabled ? server.status || mcpAppearance(server.state).label : "Disabled";
  return [server.transport, server.source, status].filter(Boolean).join(" · ");
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

/// True for the slash command the picker answers, so a typed `/mcps` opens the
/// list instead of being sent to the model as a prompt. Only the bare command:
/// `/mcp list something` is left to the agent's own resolution.
export function isMcpCommand(text: string): boolean {
  const parts = text.trim().split(/\s+/);
  if (parts.length !== 1) return false;
  const name = parts[0].replace(/^\//, "").toLowerCase();
  return name === "mcps" || name === "mcp";
}
