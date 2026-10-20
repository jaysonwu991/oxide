// The dialogs the panel paints itself: settings, the MCP server list and the
// session history.
//
// They are composed here as data — a title, a note and a list of rows — so the
// webview stays a dumb renderer and never decides what a click means: a row
// carries the action it posts back, exactly like a footer chip carries its
// control id. The listing itself comes from the CLI (`oxide mcp list --json`,
// `oxide sessions list`), so the rows here say what the terminal's `/mcp` and
// `/session` say about the same project.
//
// Rendering them in the panel instead of a `showQuickPick` keeps the flow in
// Oxide's own UI: a native picker takes over the window, hides the transcript
// the listing is about, and cannot be answered while a turn streams.

import { mcpStatusLabel, type McpServerView } from "./mcps";
import {
  DEFAULT_LANGUAGE,
  FALLBACK_LANGUAGES,
  translate,
  type LanguageInfo,
} from "./i18n";
import { filterProviders, providerState, type ProviderView } from "./providers";
import { modelChoiceId } from "./models";
import { filterSessions, type SessionEntry } from "./sessions";
import { worktreeAt, type Worktree } from "./worktrees";
import { updateVsix, type UpdateCheck } from "./updates";

/// How a row's status is colored: the green/amber/red the terminal's `/mcp`
/// uses, `muted` for a server that is off or a session's age.
export type DialogTone = "ok" | "warn" | "error" | "muted" | "";

/// What a row is, so the renderer can paint the shapes a listing is made of
/// without reading its labels: a way out of the listing (`action` — the session
/// history's two rows), one of the things it lists (`thread`), or an ordinary
/// row (""), which is what a server and a confirmation's rows are.
export type DialogRowKind = "action" | "thread" | "";

/// The glyphs a row's own button can be painted with. The webview cannot load
/// VS Code's codicon font, so the few it needs are named here and inlined as
/// SVG by the renderer — the words the button carries stay in `button`.
export type DialogIcon = "power" | "pin" | "archive" | "plus" | "close" | "";

export interface DialogRow {
  /// What the row's action posts back — a server name, a session id.
  value: string;
  label: string;
  /// The dim second line: what the row is, and where it comes from.
  detail: string;
  /// A short trailing word (`Connected`, `2h ago`), painted in `tone`.
  status: string;
  tone: DialogTone;
  /// The action a click on the row itself posts, or `""` for a row that only
  /// carries a button.
  action: string;
  /// A button at the row's end: what it does, in words — its tooltip and its
  /// label for a screen reader, and its text when `icon` is empty.
  button: string;
  buttonAction: string;
  buttons?: { label: string; action: string; icon: DialogIcon }[];
  /// The glyph the button is painted as, so a narrow pane gets a switch rather
  /// than a word that would not fit.
  icon: DialogIcon;
  /// What the row is, for the listing's own styling.
  kind: DialogRowKind;
  /// Whether this is the thread the panel has open. Carried rather than read off
  /// `status`, which is a word to paint: the mark is state, and a listing whose
  /// open thread is not in it marks nothing.
  current: boolean;
}

/// Which end of the panel a dialog card sits near. Session history, settings
/// and MCP servers belong near the composer where they are opened, matching the
/// desktop panels; confirmations can still replace a header-side listing.
export type DialogPin = "header" | "footer";

/// Which dialog is open. Carried rather than inferred from the pin and the rows,
/// because the controller has to know what it is looking at: the session listing
/// is painted again from a fresh read when a turn ends, and a confirmation must
/// not be swapped for a listing the moment it appears.
export type DialogKind =
  | "shortcuts"
  | "update"
  | "provider"
  | "logout"
  | "model"
  | "agent"
  | "reasoning"
  | "trust"
  | "permissions"
  | "mcp"
  | "sessions"
  | "worktrees"
  | "undo"
  | "language";

export interface DialogState {
  kind: DialogKind;
  /// The end of the panel that positions the complete standalone card.
  pin: DialogPin;
  title: string;
  subtitle: string;
  /// A line above the rows: why the list is empty, that it is still loading, or
  /// that the last listing failed.
  note: string;
  rows: DialogRow[];
  /// How many of the rows are the things being listed — threads, servers — for
  /// the count the head carries beside the title. 0 for a dialog whose rows are
  /// answers rather than a listing, and 0 hides it.
  count: number;
  /// Whether the head carries a search box, and the filter that was applied to
  /// the rows, echoed back so a redraw keeps what the reader typed rather than
  /// clearing it under them. The panel filters what the store already answered,
  /// which is why the query is only ever applied to rows that are here.
  search: boolean;
  query: string;
  /// The prompt inside a searchable/custom-value listing.
  searchPlaceholder?: string;
  /// A trailing action beside Close (`Recheck`), and the action it posts.
  refreshLabel: string;
  refreshAction: string;
}

/// The actions a row posts back. The controller routes them (`dialogAction`),
/// so a rename here is a compile error there rather than a dead button.
export const MCP_TOGGLE = "mcpToggle";
export const MCP_REFRESH = "mcpRefresh";
export const OPEN_SESSION = "openSession";
/// A checkout picked in the worktree listing: the folder the next chat runs in.
/// Its value is the checkout's path, or the empty string for the project's own
/// folder.
export const WORKTREE_SELECT = "worktreeSelect";
/// The row that makes a worktree: the name is typed into VS Code's own input
/// box, since a listing row is a button rather than a form.
export const WORKTREE_CREATE = "worktreeCreate";
export const WORKTREE_REMOVE = "worktreeRemove";
/// Reading the listing again, which is the head's own trailing action.
export const WORKTREE_REFRESH = "worktreeRefresh";
export const WORKTREE_BRANCH = "worktreeBranch";
export const WORKTREE_HANDOFF = "worktreeHandoff";
export const SESSION_PIN = "sessionPin";
export const SESSION_ARCHIVE = "sessionArchive";
/// The confirmation a change card's Undo opens, and the row that does the
/// restore. The Undo under the card itself posts its own view message, since it
/// is a transcript control rather than a row of a dialog.
export const CHANGES_UNDO_CONFIRM = "changesUndoConfirm";
/// The language listing's own control: a row switches to the language it names.
export const LANGUAGE_SELECT = "languageSelect";
/// Close or Escape: the view dismisses it and tells the controller, so the next
/// pane to attach does not paint it again.
export const CLOSE_DIALOG = "dialogClose";
/// A row of the provider table: signing in to the provider it names, which is
/// how the panel connects one without the terminal.
export const PROVIDER_SELECT = "providerSelect";
/// The row that forgets a provider's credential (`/logout`), which is the other
/// half of connecting one here.
export const PROVIDER_LOGOUT = "providerLogout";
/// The row that forgets every saved "Always allow" rule for this project.
export const APPROVALS_CLEAR = "approvalsClear";
export const APPLY_MODEL = "applyModel";
export const APPLY_AGENT = "applyAgent";
export const APPLY_REASONING = "applyReasoning";
export const APPLY_TRUST = "applyTrust";
/// The update dialog's own two rows: installing the release the check resolved,
/// and opening its release page. The check itself is run by the command, not by
/// a row, so a dialog that is only reporting has nothing to press.
export const UPDATE_INSTALL = "updateInstall";
export const UPDATE_NOTES = "updateNotes";
/// The row that updates the CLI rather than this extension: `oxide update`,
/// which works on any version and is the command the terminal uses.
export const UPDATE_CLI = "updateCli";
/// Restarts the window, which is what puts the freshly installed extension in
/// charge: the code in this window is the one that was running when it was
/// replaced.
export const UPDATE_RELOAD = "updateReload";

/// The values the session dialog's own two rows carry, so the controller can
/// tell a session id from "start over".
export const NEW_SESSION = "new";
export const CONTINUE_SESSION = "continue";

function row(
  value: string,
  label: string,
  rest: Partial<Omit<DialogRow, "value" | "label">> = {},
): DialogRow {
  return {
    value,
    label,
    detail: "",
    status: "",
    tone: "",
    action: "",
    button: "",
    buttonAction: "",
    icon: "",
    kind: "",
    current: false,
    ...rest,
  };
}

const MCP_TONES: Record<string, DialogTone> = {
  connected: "ok",
  "needs-auth": "warn",
  "needs-trust": "warn",
  disabled: "muted",
  error: "error",
};

export interface ModelChoice {
  model: string;
  provider: string;
}

/// The provider table, in the panel: every provider a client can connect, with
/// the state a login would change, searched rather than listed because the table
/// holds all of them. A row signs in to the provider it names — the same
/// `auth.json` and `config.json` the terminal's `/connect` writes, so what the
/// panel connects is what the next turn runs on.
///
/// The rows come from the CLI (`oxide providers --json`), so which providers
/// exist, which are stored and which one is in use is the core's answer rather
/// than a copy kept here.
export function providerDialog(
  providers: readonly ProviderView[],
  query = "",
  note = "",
  info: LanguageInfo | null = null,
): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  const rows = filterProviders(providers, query);
  const empty = providers.length
    ? query.trim()
      ? t("ui.noProviderMatches", "No provider matches “{value0}”.", { value0: String(query.trim()) })
      : ""
    : t("ui.noProvidersAreConfiguredForThisInstallation", "No providers are configured for this installation.");
  return {
    kind: "provider",
    // The composer's own column, like the MCP list: `/connect` is typed there,
    // and the model chip beside it is where the model is chosen afterwards.
    pin: "footer",
    title: t("ui.providers", "Providers"),
    subtitle:
      t("ui.signInToAProviderTheCredentialIsStoredWhereTheTerminalSConnectStoresItAndTheNext", "Sign in to a provider. The credential is stored where the terminal's /connect stores it, and the next turn uses it."),
    note: note || empty,
    rows: rows.map((provider) =>
      row(provider.name, provider.label, {
        detail: [provider.name, provider.description].filter(Boolean).join(" · "),
        status: providerState(provider, info),
        // Only the provider in use is called out in the accent: a stored one is
        // ordinary news, and a row with nothing to say is not a disabled one.
        tone: provider.active ? "ok" : "",
        action: PROVIDER_SELECT,
      }),
    ),
    count: rows.length,
    search: true,
    query,
    searchPlaceholder: t("dialog.connect.search", "Search providers…"),
    refreshLabel: "",
    refreshAction: "",
  };
}

/// Model selection stays attached to the composer. The search field doubles as
/// custom input: a value not in the provider catalog becomes a row the user
/// can deliberately choose.
///
/// Every logged-in provider's catalog is listed, grouped by the provider's name
/// in each row's detail, with the one in use first — the desktop app's picker
/// draws the same table. `provider` is the one `config.json` selects: a row from
/// another one is a switch as well as a choice, which the controller performs
/// through `oxide login` when it is taken.
export function modelDialog(
  configured: string,
  provider: string,
  current: string,
  models: readonly ModelChoice[],
  query = "",
  note = "",
  info: LanguageInfo | null = null,
): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  const needle = query.trim().toLowerCase();
  const choices =
    current && !models.some((entry) => entry.model === current)
      ? [...models, { provider, model: current }]
      : [...models];
  const known = choices.filter((entry) => !needle || entry.model.toLowerCase().includes(needle));
  const exact = choices.some((entry) => entry.model.toLowerCase() === needle);
  const configLabel = t("ui.oxideConfigDefault", "Oxide config default");
  const configMatches =
    !needle ||
    configLabel.toLowerCase().includes(needle) ||
    configured.toLowerCase().includes(needle);
  const elsewhere = known.some((entry) => entry.provider && entry.provider !== provider);
  return {
    kind: "model",
    pin: "footer",
    title: t("dialog.models.title", "Model"),
    subtitle: elsewhere
      ? t("ui.chooseAModelFromAnyProviderYouHaveConnectedPickingOneFromAnotherProviderSwitches", "Choose a model from any provider you have connected. Picking one from another provider switches to it, which the terminal and the desktop app read too.")
      : t("ui.chooseAModelAvailableFromTheActiveProviderOrEnterAnyModelID", "Choose a model available from the active provider, or enter any model ID."),
    note,
    rows: [
      ...(configMatches
        ? [
            row("", configLabel, {
              detail: configured
                ? t("model.useConfigured", "Use {value0}, the model stored in the Oxide config", { value0: configured })
                : t("ui.useTheModelStoredInTheOxideConfig", "Use the model stored in the Oxide config"),
              status: current ? "" : t("ui.current", "Current"),
              tone: "muted",
              action: APPLY_MODEL,
            }),
          ]
        : []),
      ...known.map((entry) =>
        // The value is the provider-qualified choice rather than the bare id:
        // two providers can offer the same id, and the row has to say which one
        // it is so the action switches to that provider rather than to the first
        // entry that happens to match.
        row(modelChoiceId(entry.provider, entry.model), entry.model, {
          detail: entry.provider
            ? t("ui.availableFrom", "Available from {value0}{value1}", { value0: entry.provider, value1: entry.provider === provider ? " (active)" : "" })
            : t("ui.currentModelOverride", "Current model override"),
          status:
            entry.model === current
              ? t("ui.current", "Current")
              : entry.provider && entry.provider !== provider
                ? entry.provider
                : entry.provider === provider
                  ? t("ui.inUse", "in use")
                  : "",
          tone: "muted",
          action: APPLY_MODEL,
        }),
      ),
      ...(needle && !exact
        ? [
            row(query.trim(), t("ui.use", "Use “{value0}”", { value0: query.trim() }), {
              detail: t("ui.passThisModelIDWithModel", "Pass this model ID with --model"),
              action: APPLY_MODEL,
              kind: "action",
            }),
          ]
        : []),
    ],
    count: known.length,
    search: true,
    query,
    searchPlaceholder: t("ui.filterOrEnterAModelID", "Filter or enter a model ID…"),
    refreshLabel: "",
    refreshAction: "",
  };
}

export interface AgentChoice {
  name: string;
  description: string;
}

export function agentDialog(
  agents: readonly AgentChoice[],
  current: string,
  query = "",
  info: LanguageInfo | null = null,
): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  const needle = query.trim().toLowerCase();
  const known = agents.filter(
    (agent) =>
      !needle ||
      agent.name.toLowerCase().includes(needle) ||
      agent.description.toLowerCase().includes(needle),
  );
  const exact = agents.some((agent) => agent.name.toLowerCase() === needle);
  const defaultMatches = !needle || "default no agent".includes(needle);
  return {
    kind: "agent",
    pin: "footer",
    title: t("ui.agent", "Agent"),
    subtitle: t("ui.chooseADiscoveredAgentOrTypeAnAgentNameBelow", "Choose a discovered agent, or type an agent name below."),
    note: agents.length ? "" : t("ui.noConfiguredAgentsWereDiscoveredYouCanStillEnterAName", "No configured agents were discovered; you can still enter a name."),
    rows: [
      ...(defaultMatches
        ? [
            row("", t("ui.defaultAgent", "Default agent"), {
              detail: t("ui.runWithoutASubagentPrompt", "Run without a subagent prompt"),
              status: current ? "" : t("ui.current", "Current"),
              tone: "muted",
              action: APPLY_AGENT,
            }),
          ]
        : []),
      ...known.map((agent) =>
        row(agent.name, agent.name, {
          detail: agent.description,
          status: agent.name === current ? t("ui.current", "Current") : "",
          tone: "muted",
          action: APPLY_AGENT,
        }),
      ),
      ...(needle && !exact
        ? [
            row(query.trim(), t("ui.use", "Use “{value0}”", { value0: query.trim() }), {
              detail: t("ui.passThisNameWithAgent", "Pass this name with --agent"),
              action: APPLY_AGENT,
              kind: "action",
            }),
          ]
        : []),
    ],
    count: 0,
    search: true,
    query,
    searchPlaceholder: t("ui.filterOrEnterAnAgentName", "Filter or enter an agent name…"),
    refreshLabel: "",
    refreshAction: "",
  };
}

/// The language listing: the languages the CLI offers, each named in itself
/// with the one in use marked. A row switches the language every front-end
/// draws — the same `settings.json` key the terminal and the desktop app read —
/// because the choice is written by the CLI rather than kept in a VS Code
/// setting this panel alone would follow.
export function languageDialog(info: LanguageInfo | null): DialogState {
  const current = info?.language ?? DEFAULT_LANGUAGE;
  const languages = info?.languages?.length ? info.languages : FALLBACK_LANGUAGES;
  return {
    kind: "language",
    pin: "footer",
    title: translate(info, "language.name", "Language"),
    subtitle: translate(
      info,
      "dialog.language.subtitle",
      "The language this window, the terminal and the editor panel share. Switching it rewrites the same settings.json key every front-end reads.",
    ),
    note: "",
    rows: languages.map((language) =>
      row(language.tag, language.nativeLabel || language.label, {
        detail: language.tag,
        action: LANGUAGE_SELECT,
        status: language.tag === current ? translate(info, "panel.current", "Current") : "",
        tone: language.tag === current ? "ok" : "",
        current: language.tag === current,
      }),
    ),
    count: 0,
    search: false,
    query: "",
    refreshLabel: "",
    refreshAction: "",
  };
}

export function reasoningDialog(current: string, levels: readonly string[], info: LanguageInfo | null = null): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  return choiceDialog(
    "reasoning",
    t("dialog.reasoning.title", "Reasoning effort"),
    t("ui.theEffortPassedToTheNextTurnWithReasoning", "The effort passed to the next turn with --reasoning."),
    APPLY_REASONING,
    levels.map((level) => ({ value: level, label: t(`reasoning.${level}`, level), detail: "" })),
    current, info
  );
}

/// The `/permissions` dialog: the tools this project allows without prompting,
/// read from the shared `approvals.json` through `oxide approvals`. The rows are
/// the rules an `Always allow` saved, wherever it was answered — the terminal,
/// the desktop app or this panel — so a rule given here is visible here and a
/// rule given elsewhere can be taken back here.
///
/// `project` is the folder the rules belong to, since the store is per project:
/// a listing for another one would be a different set of rules under the same
/// heading.
export function permissionsDialog(
  project: string,
  tools: readonly string[],
  note = "",
  info: LanguageInfo | null = null,
): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  const empty = tools.length
    ? ""
    : project
      ? t("ui.noToolIsAllowedWithoutPromptingInAnAlwaysAllowAnswerAddsOne", "No tool is allowed without prompting in {value0}. An “Always allow” answer adds one.", { value0: project })
      : t("ui.openAFolderFirst", "Open a folder first.");
  return {
    kind: "permissions",
    // Opened from the composer's row, where `/permissions` is typed.
    pin: "footer",
    title: t("ui.savedApprovals", "Saved approvals"),
    subtitle: t("ui.toolsThisProjectRunsWithoutAskingClearingForgetsEveryRuleHere", "Tools this project runs without asking. Clearing forgets every rule here."),
    note: note || empty,
    rows: [
      ...tools.map((tool) =>
        row(tool, tool, {
          detail: t("ui.runsWithoutAskingIn", "Runs without asking in {value0}", { value0: project }),
          status: "allowed",
          tone: "ok",
          action: "",
        }),
      ),
      ...(tools.length
        ? [
            row("clear", t("dialog.permissions.clear", "Clear all"), {
              detail: t(tools.length === 1 ? "approvals.forgetRule" : "approvals.forgetRules", tools.length === 1 ? "Forget {count} rule for this project" : "Forget {count} rules for this project", { count: tools.length }),
              tone: "muted",
              action: APPROVALS_CLEAR,
              kind: "action" as DialogRowKind,
            }),
          ]
        : []),
    ],
    count: tools.length,
    search: false,
    query: "",
    refreshLabel: "",
    refreshAction: "",
  };
}

/// The `/logout` dialog: the providers this machine holds a credential for. A
/// provider is signed out by the row that names it, so the panel can take back
/// what it — or the terminal, or the desktop app — connected.
export function logoutDialog(
  providers: readonly ProviderView[],
  query = "",
  note = "",
  info: LanguageInfo | null = null,
): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  const stored = filterProviders(providers, query).filter((provider) => provider.stored);
  const empty = providers.some((provider) => provider.stored)
    ? t("ui.noStoredCredentialMatches", "No stored credential matches “{value0}”.", { value0: query.trim() })
    : t("ui.noProviderHasAStoredCredentialConnectAddsOne", "No provider has a stored credential. /connect adds one.");
  return {
    kind: "logout",
    pin: "footer",
    title: t("ui.storedProviders", "Stored providers"),
    subtitle:
      t("ui.signOutOfAProviderTheCredentialIsRemovedWhereItIsStoredAndATurnThatUsedItSwitche", "Sign out of a provider. The credential is removed where it is stored, and a turn that used it switches to another logged-in provider."),
    note: note || (stored.length ? "" : empty),
    rows: stored.map((provider) =>
      row(provider.name, provider.label, {
        detail: [provider.name, provider.description].filter(Boolean).join(" · "),
        status: provider.active ? t("ui.inUse", "in use") : "stored",
        tone: provider.active ? "ok" : "muted",
        action: PROVIDER_LOGOUT,
      }),
    ),
    count: stored.length,
    search: true,
    query,
    searchPlaceholder: t("ui.searchStoredProviders", "Search stored providers…"),
    refreshLabel: "",
    refreshAction: "",
  };
}

export function trustDialog(state: { setting: string; saved: boolean | undefined }, info: LanguageInfo | null = null): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  // The row that describes what is in force: the panel's own override first,
  // then the decision saved for this folder, then the default. The rows write
  // the shared `trust.json` — the store the terminal's `/trust` and the desktop
  // app read — so a decision made here is a decision everywhere, and the
  // override is left unset rather than holding a second answer.
  const current =
    state.setting === "always"
      ? "trusted"
      : state.setting === "never"
        ? "untrusted"
        : state.saved === true
          ? "trusted"
          : state.saved === false
            ? "untrusted"
            : "default";
  return choiceDialog(
    "trust",
    t("composer.trust", "Project access"),
    t("ui.trustingThisFolderLoadsItsLocalResourcesWhichCanRunCommandsOrChangeHowTheAgentBe", "Trusting this folder loads its local resources, which can run commands or change how the agent behaves. The decision is saved in trust.json and shared with the CLI and desktop."),
    APPLY_TRUST,
    [
      {
        value: "default",
        label: t("ui.useTheSavedDecision", "Use the saved decision"),
        detail: t("ui.followTrustJsonOrDefaultProjectTrust", "Follow trust.json or defaultProjectTrust"),
      },
      {
        value: "trusted",
        label: t("ui.trustThisFolder", "Trust this folder"),
        detail: t("ui.writeTrustJsonEveryFrontEndReadsTheSameDecision", "Write trust.json — every front-end reads the same decision"),
      },
      {
        value: "untrusted",
        label: t("ui.neverTrustThisFolder", "Never trust this folder"),
        detail: t("ui.writeTrustJsonEveryFrontEndReadsTheSameDecision", "Write trust.json — every front-end reads the same decision"),
      },
    ],
    current, info
  );
}

function choiceDialog(
  kind: "reasoning" | "trust",
  title: string,
  subtitle: string,
  action: string,
  choices: readonly { value: string; label: string; detail: string }[],
  current: string,
  info: LanguageInfo | null = null,
): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  return {
    kind,
    pin: "footer",
    title,
    subtitle,
    note: "",
    rows: choices.map((choice) =>
      row(choice.value, choice.label, {
        detail: choice.detail,
        status: choice.value === current ? t("ui.current", "Current") : "",
        current: choice.value === current,
        tone: "muted",
        action,
      }),
    ),
    count: 0,
    search: false,
    query: "",
    refreshLabel: "",
    refreshAction: "",
  };
}

/// The `/mcp` dialog: every server this project loads, the state the core
/// probed, and a switch that turns one off or back on in the file that defines
/// it. `note` overrides the empty-list message, so a listing that failed can say
/// so in place where a QuickPick would just vanish.
export function mcpDialog(servers: readonly McpServerView[], note = "", info: LanguageInfo | null = null): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  return {
    kind: "mcp",
    pin: "footer",
    title: t("dialog.mcps.title", "MCP servers"),
    subtitle:
      t("ui.theServersThisProjectLoadsAndWhetherOxideCanReachThemAToggleIsWrittenToTheFileTh", "The servers this project loads, and whether Oxide can reach them. A toggle is written to the file that defines the server — the same change the terminal's /mcp makes."),
    note:
      note ||
      (servers.length
        ? ""
        : t("ui.noMCPServersConfiguredForThisProjectAddOneWithOxideMcpAddOrAnMcpJsonInTheProject", "No MCP servers configured for this project. Add one with oxide mcp add, or an .mcp.json in the project.")),
    count: servers.length,
    search: false,
    query: "",
    rows: servers.map((server) =>
      row(server.name, server.name, {
        detail: [server.transport, server.detail, t("ui.source", "source: {value0}", { value0: server.source })]
          .filter(Boolean)
          .join(" · "),
        // A server that is switched off reports that rather than the state a
        // probe could not reach.
        status: mcpStatusLabel(server, info),
        tone: MCP_TONES[server.enabled ? server.state : "disabled"] ?? "",
        button: server.enabled ? t("ui.disable", "Disable {value0}", { value0: server.name }) : t("ui.enable", "Enable {value0}", { value0: server.name }),
        buttonAction: MCP_TOGGLE,
        icon: "power",
      }),
    ),
    refreshLabel: t("ui.recheck", "Recheck"),
    refreshAction: MCP_REFRESH,
  };
}

/// The worktrees this project holds, in the panel: the checkout a chat runs in,
/// beside the folder itself. A worktree is where a conversation's work stays out
/// of the reader's checkout, and its branch is what carries that work back.
///
/// Picking a row starts a chat there rather than moving the one on screen: the
/// checkout belongs to the task, which is what makes two threads of one project
/// readable apart. `active` is the checkout the panel is already in, marked the
/// way the session listing marks the thread it has open.
export function worktreeDialog(
  worktrees: readonly Worktree[],
  active: string | null,
  projectName: string,
  note = "",
  sessionId?: string | null,
  info: LanguageInfo | null = null,
): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  const inLocal = !worktreeAt(worktrees, active);
  const local = row("", t("ui.localCheckout", "Local checkout"), {
    detail: t("ui.sOwnFolderTheCheckoutYouHaveOpen", "{value0}'s own folder — the checkout you have open", { value0: projectName }),
    action: WORKTREE_SELECT,
    status: inLocal ? t("ui.inUse", "in use") : "",
    tone: inLocal ? "ok" : "",
    current: inLocal,
    kind: "thread",
  });
  const rows = worktrees.map((entry) =>
    row(entry.path, entry.name, {
      detail: [entry.branch, entry.path].filter(Boolean).join(" · "),
      action: WORKTREE_SELECT,
      status: entry.path === active ? t("ui.inUse", "in use") : "",
      tone: entry.path === active ? "ok" : "",
      current: entry.path === active,
      kind: "thread",
      buttons: [
        { label: t("ui.removeTheWorktree", "Remove the worktree {value0}", { value0: String(entry.name) }), action: WORKTREE_REMOVE, icon: "close" },
      ],
    }),
  );
  return {
    kind: "worktrees",
    // The composer's own column: the checkout is chosen where a message is
    // composed, and the transcript stays readable behind it.
    pin: "footer",
    title: t("dialog.worktrees.title", "Worktrees"),
    subtitle:
      t("ui.runAChatInAnIsolatedCheckoutStartFromABranchOrCommitThenCreateABranchOrHandTheCh", "Run a chat in an isolated checkout. Start from a branch or commit, then create a branch or hand the chat back to Local when ready."),
    note: note || (worktrees.length ? "" : t("ui.noWorktreesYetMakeOneFromTheNameOfTheTask", "No worktrees yet. Make one from the name of the task.")),
    rows: [
      local,
      ...rows,
      ...(sessionId ? [row("", active ? t("ui.handOffChatToLocal", "Hand off chat to Local") : t("dialog.worktrees.handoff", "Hand off chat to Worktree"), { action: WORKTREE_HANDOFF })] : []),
      ...(active ? [row(active, t("dialog.worktrees.createBranch", "Create branch here"), { action: WORKTREE_BRANCH })] : []),
      row("", t("ui.newWorktree", "New worktree…"), {
        detail: t("ui.namesACheckoutAndStartsAChatInIt", "Names a checkout and starts a chat in it"),
        action: WORKTREE_CREATE,
      }),
    ],
    count: worktrees.length,
    search: false,
    query: "",
    refreshLabel: t("ui.recheck", "Recheck"),
    refreshAction: WORKTREE_REFRESH,
  };
}

/// The thread the panel has open, as the listing needs it when the store has no
/// row (or no name) for it yet: a session file is written as its first turn
/// runs, so the listing can be read before the row it belongs to exists.
export interface LiveSession {
  /// The thread's id, the one the `session` header reported.
  id: string;
  /// The title the header shows it under, empty for a thread with nothing said
  /// in it yet.
  label: string;
}

/// The session history: the threads stored for this project, newest first as the
/// CLI lists them, with the thread the panel is showing marked and the two ways
/// out of it — a fresh chat, and the newest thread this project has.
///
/// `current` is the session the panel has open (its id, from the `session`
/// header the CLI reported or the id a row resumed), so the list can say which
/// row is the conversation on screen: closing it is what "New chat" does, and
/// without the mark there is nothing in the listing that says where you are. It
/// is required rather than defaulted, and it comes before the note, because
/// every redraw of the listing has to carry it: a rebuild that left it out —
/// after a failed delete, say — would drop the mark and turn the first row back
/// into one that promises a fresh thread rather than closing the open one.
///
/// `live` is that same thread when the panel knows more about it than the store
/// does — a run that has not been written yet, or a file whose thread is still
/// unnamed. It stands in for itself: a row of its own ahead of the store's when
/// there is no row there, and its title where the store's row has none.
///
/// `query` is the search box's own value — the listing is long enough to be
/// worth filtering — and the rows are narrowed by it before they are painted.
/// It is echoed back so a redraw under a typed-in filter keeps the filter, and
/// the count in the head is what the filter left, so narrowing the list is
/// visible as a number rather than only as a shorter list.
///
/// `running` is the thread a turn is writing into when that thread is not the
/// one on screen — the reader moved to another thread while it ran. Its row says
/// so rather than reading as any other past conversation, because a turn is
/// still adding to it: the row is a way back to the reply being written, not
/// only to what it already said. The open thread's own mark is `current`, and a
/// row is never both.
export function sessionDialog(
  sessions: readonly SessionEntry[],
  current: string | null,
  note = "",
  live: LiveSession | null = null,
  query = "",
  running: LiveSession | null = null,
  info: LanguageInfo | null = null,
): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  const open = live && live.id === current ? live : null;
  const file = open ? sessions.find((session) => session.id === open.id) : undefined;
  const title = open?.label.trim() || "";
  // The thread on screen is painted under the header's title where the store has
  // none for it yet, so the rows are named before anything reads them: the name
  // a search is matched against is the name the row shows, and a query for what
  // the listing displays cannot miss the row it displays it on.
  const named = open
    ? sessions.map((session) =>
        session.id === open.id && !session.label ? { ...session, label: title } : session,
      )
    : sessions;
  const filtered = filterSessions(named, query);
  // The turn's own thread, when the reader is not looking at it: its row says a
  // turn is in it rather than reading as a past conversation.
  const runningId = running && running.id !== current ? running.id : null;
  // The threads the store has no row for yet: the one on screen — a fresh thread
  // whose file it is still catching up to — and, when the reader is elsewhere,
  // the one a turn is writing into. A session file is written as its turn
  // starts, so the second is only the moment before the read lands; without a
  // stand-in the listing would omit the very thread it has the most to say
  // about.
  const stands: SessionEntry[] = [];
  if (open && !file) stands.push({ id: open.id, label: title, age: "", messages: 0 });
  if (runningId && !sessions.some((session) => session.id === runningId)) {
    stands.push({ id: runningId, label: running!.label.trim(), age: "", messages: 0 });
  }
  // The thread on screen is filtered like any other row, so a search never
  // leaves a row behind that the query does not match.
  const standing = filterSessions(stands, query);
  const threads = standing.length + filtered.length;
  // The note is the host's when it has one to give (the read failed, the store
  // is not there yet); otherwise a listing says why it is showing no threads,
  // which is either nothing stored yet or a filter that left none.
  const empty = sessions.length || stands.length
    ? query.trim() && !threads
      ? t("ui.noThreadMatches", "No thread matches “{value0}”.", { value0: query.trim() })
      : ""
    : t("ui.noSessionsForThisProjectYetTheNextMessageStartsOne", "No sessions for this project yet — the next message starts one.");
  return {
    kind: "sessions",
    // `/session` grows out of the composer in Desktop. Keeping the extension's
    // copy on that same edge makes the two front-ends read as one operation and
    // leaves the thread title visible above the transcript while the list is up.
    pin: "footer",
    title: t("dialog.sessions.title", "Sessions"),
    subtitle: t("ui.threadsStoredForThisProjectPinnedFirstThenMostRecentPickingOneOpensItHere", "Threads stored for this project, pinned first, then most recent. Picking one opens it here."),
    note: note || empty,
    rows: [
      row(NEW_SESSION, t("panel.newChat", "New chat"), {
        // On a fresh page there is nothing to close, and saying otherwise would
        // be a row describing a thread the project does not have.
        detail: current ? t("ui.closeThisThreadAndStartAFreshOne", "Close this thread and start a fresh one") : t("ui.startAFreshThread", "Start a fresh thread"),
        action: OPEN_SESSION,
        kind: "action",
      }),
      row(CONTINUE_SESSION, t("ui.continueMostRecentSession", "Continue most recent session"), {
        detail: t("ui.pickUpTheNewestSessionForThisProject", "Pick up the newest session for this project"),
        action: OPEN_SESSION,
        kind: "action",
      }),
      ...standing.map((entry) =>
        row(entry.id, entry.label || entry.id, {
          // Preferences are offered once the store has a file for the thread.
          detail: t("ui.openInThisPanelTheStoreHasNoFileForItYet", "Open in this panel — the store has no file for it yet"),
          status: entry.id === runningId ? t("ui.running", "Running") : t("ui.current", "Current"),
          tone: (entry.id === runningId ? "ok" : "muted") as DialogTone,
          action: OPEN_SESSION,
          kind: "thread",
          current: entry.id === current,
        }),
      ),
      ...filtered.map((session) => {
        const marked = session.id === current;
        const live = session.id === runningId;
        const name = session.label || session.id;
        return row(session.id, name, {
          detail: t(session.messages === 1 ? "session.messageCountOne" : "session.messageCount", session.messages === 1 ? "{count} message" : "{count} messages", { count: session.messages }),
          status: marked ? t("ui.current", "Current") : live ? t("ui.running", "Running") : session.pinned ? t("ui.pinned", "Pinned") : session.age,
          tone: live ? "ok" : "muted",
          action: OPEN_SESSION,
          buttons: [
            { label: session.pinned ? t("ui.unpinThread", "Unpin thread") : t("ui.pinThread", "Pin thread"), action: SESSION_PIN, icon: "pin" },
            { label: t("ui.archiveThread", "Archive thread"), action: SESSION_ARCHIVE, icon: "archive" },
          ],
          kind: "thread",
          current: marked,
        });
      }),
    ],
    count: threads,
    search: true,
    query,
    refreshLabel: "",
    refreshAction: "",
  };
}

/// The confirmation a change card's Undo opens: what the turn wrote, and the
/// restore as the row rather than a second button. It is a panel dialog for the
/// same reason the one above is, and the desktop app asks before it puts a turn
/// back the same way. What it puts back is the whole turn, so the note says what
/// a later edit would mean for it — the CLI reports that rather than folding it
/// into the restore.
export function undoChangesDialog(card: { id: number; detail: string }, info: LanguageInfo | null = null): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  return {
    kind: "undo",
    pin: "header",
    title: t("ui.undoTurn", "Undo turn"),
    subtitle: t("changes.restoreTurnFiles", "Put this turn's files back to how the run found them."),
    note: t("ui.anEditMadeSinceTheTurnIsRefusedRatherThanUndone", "An edit made since the turn is refused rather than undone."),
    rows: [
      row(String(card.id), t("ui.undoChanges", "Undo changes"), {
        detail: card.detail,
        tone: "warn",
        action: CHANGES_UNDO_CONFIRM,
      }),
      row("", t("common.cancel", "Cancel"), { detail: t("ui.keepTheTurnSFiles", "Keep the turn's files"), action: CLOSE_DIALOG }),
    ],
    count: 0,
    search: false,
    query: "",
    refreshLabel: "",
    refreshAction: "",
  };
}

/// What the update dialog is showing. The check runs through the CLI and takes
/// a round trip to GitHub, so the panel has something to say before there is an
/// answer, and the install takes minutes; each state carries the words rather
/// than the call sites composing a dialog of their own.
export type UpdateState =
  /// Asking the installed CLI, which is asking GitHub.
  | { k: "checking" }
  /// The install is running. `what` names what is being installed, phrased for
  /// the title: a CLI too old to be checked has no version to name, so it is
  /// `the newest CLI` there and `Oxide 0.34.0` when the check resolved one.
  | { k: "installing"; what: string }
  /// The install landed. `check` is the check that resolved it, so the note
  /// can name the release and the file that carries it, and VS Code holds the
  /// new version from the next window on.
  | { k: "installed"; check: UpdateCheck }
  /// The check or the install did not answer with a result. `stage` says which
  /// one, because the same failure means different things at each: a check that
  /// could not reach GitHub leaves the installation alone, while a failed
  /// install may have left a half-downloaded release behind.
  | { k: "failed"; stage: "check" | "install"; message: string }
  /// The check resolved a release. `cli` is the same check asked about the CLI's
  /// own train, which is the binary every turn runs through: the panel reaches
  /// GitHub through it, and a release of it that is newer than the one installed
  /// is worth saying here, where the reader is already looking at versions.
  | { k: "ready"; check: UpdateCheck; cli: UpdateCheck | null }
  /// The installed CLI is older than this panel: it does not know `--json`, so
  /// there is no release to report — but `oxide update` still updates it, which
  /// is what `text` (the CLI's own refusal) is shown under. `headline` replaces
  /// the title for the same state read after an install ran, when what it holds
  /// is that install's report rather than a refusal, and `subtitle` says what
  /// that report is instead of describing the refusal it is not.
  | { k: "legacy"; text: string; path: string; headline?: string; subtitle?: string };

/// The update dialog: the release the extension's own train resolved, what this
/// window runs, and the install — the `.vsix` — as a row.
///
/// It is the panel's own update and deliberately not the CLI's: the check is
/// asked about the `extension-v*` releases and the panel installs what it
/// resolved, so the row cannot offer a release this editor would not run. The
/// one exception is the row a CLI too old to answer the check is offered, which
/// updates that binary — the thing standing between the user and a check at
/// all — and says so in as many words.
export function updateDialog(state: UpdateState, info: LanguageInfo | null = null): DialogState {
  const t = (key: string, fallback: string, vars?: Record<string, unknown>) =>
    translate(info, key, fallback, vars);
  const empty = {
    kind: "update" as const,
    pin: "footer" as const,
    count: 0,
    search: false,
    query: "",
    refreshLabel: "",
    refreshAction: "",
  };
  if (state.k === "checking") {
    return {
      ...empty,
      title: t("ui.checkingForUpdates", "Checking for updates"),
      subtitle:
        t("ui.oxideAsksTheInstalledCLIWhichReleaseOfThisExtensionIsNewestTheSameCheckThePanelR", "Oxide asks the installed CLI which release of this extension is newest — the same check the panel runs as `oxide update --check --component extension`."),
      note: t("ui.askingGitHubForTheNewestExtensionRelease", "Asking GitHub for the newest extension release…"),
      rows: [],
    };
  }
  if (state.k === "installing") {
    return {
      ...empty,
      title: t("ui.installing", "Installing {value0}", { value0: state.what }),
      subtitle:
        t("ui.theReleaseSVsixIsDownloadedCheckedAgainstTheChecksumItsReleasePublishedAndHanded", "The release's .vsix is downloaded, checked against the checksum its release published, and handed to VS Code to install."),
      note: t("ui.thisTakesAsLongAsTheDownloadTheDialogReportsWhatHappenedWhenItIsDone", "This takes as long as the download; the dialog reports what happened when it is done."),
      rows: [],
    };
  }
  if (state.k === "installed") {
    const { check } = state;
    return {
      ...empty,
      title: t("ui.oxideIsInstalled", "Oxide {value0} is installed", { value0: check.latest }),
      subtitle:
        t("ui.vSCodeHoldsTheNewVersionTheExtensionRunningInThisWindowIsTheOneThatWasThereWhenI", "VS Code holds the new version; the extension running in this window is the one that was there when it was installed."),
      note: [
        check.current ? t("ui.was", "Was {value0}", { value0: check.current }) : "",
        t("ui.installed", "Installed {value0}", { value0: check.latest }),
        check.asset?.name ?? "",
        t("ui.restartThisWindowToRunIt", "Restart this window to run it"),
      ]
        .filter(Boolean)
        .join(" · "),
      rows: [
        row("", t("ui.restartWindow", "Restart Window"), {
          detail: t("ui.runTheVersionThatWasJustInstalled", "Run the version that was just installed"),
          action: UPDATE_RELOAD,
        }),
        row("", t("common.close", "Close"), {
          detail: t("ui.restartLaterFromTheCommandPalette", "Restart later from the command palette"),
          action: CLOSE_DIALOG,
        }),
      ],
    };
  }
  if (state.k === "failed") {
    return {
      ...empty,
      title: state.stage === "check" ? t("ui.couldNotCheckForUpdates", "Could not check for updates") : t("ui.couldNotInstallTheUpdate", "Could not install the update"),
      subtitle:
        state.stage === "check"
          ? t("ui.theCheckAsksGitHubWhichReleaseOfThisExtensionIsNewestSoAMachineWithNoNetworkOrAR", "The check asks GitHub which release of this extension is newest, so a machine with no network — or a request GitHub refuses — reports it here.")
          : t("ui.theInstallDownloadsTheReleaseSVsixChecksItAgainstTheChecksumItsReleasePublishedA", "The install downloads the release's .vsix, checks it against the checksum its release published and hands it to VS Code."),
      note: state.message,
      rows: [row("", t("common.close", "Close"), { detail: t("ui.leaveThisExtensionAsItIs", "Leave this extension as it is"), action: CLOSE_DIALOG })],
    };
  }

  if (state.k === "legacy") {
    // The release is unknown here — the CLI that would have resolved it cannot
    // be asked — so the dialog offers the one command that works on any version
    // rather than a version to install. That command updates the CLI, which is
    // what this panel reads its own releases through; the sentence says so
    // rather than leaving the row looking like the extension's own install.
    return {
      ...empty,
      title: state.headline ?? t("ui.theOxideCLIIsOlderThanThisPanel", "The oxide CLI is older than this panel"),
      subtitle:
        state.subtitle ??
        t("ui.checkForUpdatesReadsTheExtensionSOwnReleasesThroughTheInstalledOxideAndThisCLIPr", "Check for Updates reads the extension's own releases through the installed oxide, and this CLI predates the report it reads — so updating the oxide command line is what makes both the check and the extension's next update reachable from here. It works on any version."),
      note: state.text,
      rows: [
        row("", t("ui.installTheNewestCLI", "Install the newest CLI"), {
          detail: state.path ? t("ui.replaces", "Replaces {value0}", { value0: state.path }) : t("ui.runsOxideUpdate", "Runs oxide update"),
          action: UPDATE_INSTALL,
        }),
        row("", t("common.close", "Close"), {
          detail: t("ui.leaveThisInstallationAsItIs", "Leave this installation as it is"),
          action: CLOSE_DIALOG,
        }),
      ],
    };
  }

  const { check, cli } = state;
  const vsix = updateVsix(check);
  const notes = [
    check.current ? t("update.currentVersion", "Current {value0}", { value0: check.current }) : "",
    t("ui.latest", "Latest {value0}", { value0: check.latest }),
    vsix?.name ?? "",
    check.pinned ? "pinned" : "",
  ].filter(Boolean);
  // The CLI is the other half of this installation, and the one this panel reads
  // GitHub through: a release of it that is newer than the binary on the machine
  // is said here rather than left for a turn to notice. Its own advice names what
  // to do where this panel cannot write it (a Homebrew installation).
  const cliRow = cli && cli.updateAvailable ? cli : null;
  if (cliRow) {
    notes.push(
      cliRow.installable
        ? `CLI ${cliRow.current || "older"} → ${cliRow.latest}`
        : cliRow.advice || `CLI ${cliRow.current || "older"} → ${cliRow.latest}`,
    );
  }
  // A release this panel cannot install itself — one with no build for this
  // platform, or an artifact that is not a VSIX — is reported with the check's
  // own sentence, which names what to install by hand: the row it would belong
  // to is the one thing missing.
  if (check.updateAvailable && !vsix && check.advice) notes.push(check.advice);
  const rows: DialogRow[] = [];
  if (vsix) {
    rows.push(
      row(vsix.name, t("ui.install", "Install {value0}", { value0: check.latest }), {
        detail: t("ui.downloadsAndInstalls", "Downloads and installs {value0}", { value0: vsix.name }),
        action: UPDATE_INSTALL,
      }),
    );
  }
  if (cliRow?.installable) {
    rows.push(
      row(cliRow.latest, t("ui.updateTheOxideCLITo", "Update the oxide CLI to {value0}", { value0: cliRow.latest }), {
        detail: t("ui.runsOxideUpdateReplacesTheBinaryEveryTurnRunsThrough", "Runs oxide update — replaces the {value0} binary every turn runs through", { value0: cliRow.current || "installed" }),
        action: UPDATE_CLI,
      }),
    );
  }
  if (check.releaseUrl) {
    rows.push(
      row(check.releaseUrl, t("dialog.update.notes", "Release notes"), {
        detail: t("ui.openTheReleasePageInYourBrowser", "Open the release page in your browser"),
        action: UPDATE_NOTES,
      }),
    );
  }
  rows.push(
    row("", t("common.close", "Close"), {
      detail: check.updateAvailable ? t("ui.keepTheVersionInstalledNow", "Keep the version installed now") : t("ui.nothingToInstall", "Nothing to install"),
      action: CLOSE_DIALOG,
    }),
  );
  const title = check.updateAvailable
    ? t("ui.oxideIsAvailable", "Oxide {value0} is available", { value0: check.latest })
    : cliRow?.installable
      ? t("ui.theOxideCLIIsAvailable", "The oxide CLI {value0} is available", { value0: cliRow!.latest })
      : t("ui.oxideIsUpToDate", "Oxide {value0} is up to date", { value0: check.current || check.latest });
  return {
    ...empty,
    title,
    subtitle: check.updateAvailable
      ? t("ui.installingDownloadsThisReleaseOfTheExtensionTheVsixPublishedOnItsOwnReleaseTrain", "Installing downloads this release of the extension — the .vsix published on its own release train — and hands it to VS Code, which runs it from the next window on.")
      : cliRow?.installable
        ? t("ui.thisExtensionIsCurrentTheOxideCLIThisPanelRunsEveryTurnThroughIsAtUpdatingItIsOn", "This extension is current. The oxide CLI this panel runs every turn through is at {value0}: updating it is one row, and it is the binary the terminal and the desktop app share.", { value0: cliRow!.current || "an older release" })
        : t("ui.oxideIsTheNewestReleasedVersionOfThisExtension", "Oxide {value0} is the newest released version of this extension.", { value0: check.current || check.latest }),
    note: notes.join(" · "),
    rows,
  };
}
