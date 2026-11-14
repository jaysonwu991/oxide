import { translate, type LanguageInfo } from "./i18n";
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
  /// How its tools reach the model: `deferred`, `direct`, or `hidden`.
  exposure: string;
  /// What the server says it offers, flattened to one line by its author.
  description: string;
  /// Tools the server listed when it was probed; 0 when it did not answer.
  tools: number;
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
      exposure: exposureOf(record.exposure),
      description: stringOf(record.description),
      tools: typeof record.tools === "number" && record.tools > 0 ? Math.floor(record.tools) : 0,
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

/// The three ways a server's tools reach the model, in the order the row's
/// exposure button cycles them.
export const MCP_EXPOSURES = ["deferred", "direct", "hidden"] as const;

export type McpExposure = (typeof MCP_EXPOSURES)[number];

function exposureOf(value: unknown): McpExposure {
  return (MCP_EXPOSURES as readonly string[]).includes(value as string)
    ? (value as McpExposure)
    : "deferred";
}

/// The one an exposure button asks the CLI for next.
export function nextMcpExposure(exposure: string): McpExposure {
  const current = MCP_EXPOSURES.indexOf(exposureOf(exposure));
  return MCP_EXPOSURES[(current + 1) % MCP_EXPOSURES.length];
}

/// The word an exposure is painted with, sharing the keys the terminal and the
/// desktop app use so all three name the setting the same way.
export function mcpExposureLabel(exposure: string, info: LanguageInfo | null = null): string {
  switch (exposureOf(exposure)) {
    case "direct":
      return translate(info, "ui.exposureDirect", "direct");
    case "hidden":
      return translate(info, "ui.exposureHidden", "hidden");
    default:
      return translate(info, "ui.exposureDeferred", "deferred");
  }
}

/// The word a state is painted with, so the dialog reads at a glance the way the
/// terminal's colored status does. The webview cannot draw a codicon, so the
/// state is a label colored by `dialogTone` in the dialog itself.
export function mcpStateLabel(state: string, info: LanguageInfo | null = null): string {
  switch (state) {
    case "connected":
      return translate(info, "ui.connected", "Connected");
    case "needs-auth":
      return translate(info, "state.auth", "Needs auth");
    case "needs-trust":
      return translate(info, "state.trust", "Needs trust");
    case "disabled":
      return translate(info, "ui.disabled", "Disabled");
    case "error":
      return translate(info, "ui.error", "Error");
    default:
      return state || "Unknown";
  }
}

export function mcpStatusLabel(server: McpServerView, info: LanguageInfo | null = null): string {
  if (!server.enabled) return mcpStateLabel("disabled", info);
  switch (server.state) {
    case "connected":
    case "needs-auth":
    case "needs-trust":
    case "disabled":
      return mcpStateLabel(server.state, info);
    case "error": {
      const detail = server.status.replace(/^Error(?::\s*|$)/, "");
      return detail
        ? translate(info, "common.errorDetail", "Error: {value0}", { value0: detail })
        : mcpStateLabel("error", info);
    }
    default:
      return server.status || mcpStateLabel(server.state, info);
  }
}

/// The arguments that turn a server on or off. The scope pins the change to the
/// file that defines the server, so a name in both scopes toggles the one that
/// was listed rather than whichever the lookup finds first.
export function mcpToggleArgs(server: McpServerView, enabled: boolean): string[] {
  return ["mcp", enabled ? "enable" : "disable", server.name, "--scope", server.scope];
}

/// The arguments that open a server's OAuth page in the browser and wait for
/// its loopback callback. The scope pins the lookup to the server the dialog
/// listed when the same name exists in both scopes.
export function mcpAuthArgs(server: McpServerView): string[] {
  return ["mcp", "auth", server.name, "--scope", server.scope];
}

/// The arguments that set how a server's tools reach the model. The scope pins
/// the change to the file that defines the server, like the toggle beside it.
export function mcpExposureArgs(server: McpServerView, exposure: string): string[] {
  return ["mcp", "exposure", server.name, exposure, "--scope", server.scope];
}

/// The arguments that list the servers as JSON, for the project the command
/// runs in.
export function mcpListArgs(): string[] {
  return ["mcp", "list", "--json"];
}
