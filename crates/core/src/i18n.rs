//! The language the interface is written in, shared by every front-end.
//!
//! One value decides it — `language` in `settings.json` (the project's
//! `.oxide/settings.json` winning per key), overridden for one process by
//! `OXIDE_LANG` — and one catalog says what each language's words are. The
//! terminal, the desktop app and the VS Code panel all read this module, so a
//! label is written down once and a front-end that cannot link the crate reads
//! it through `oxide language --json`.
//!
//! Only the interface is translated. What the model writes, what a tool prints
//! and everything a project configures (agents, commands, skills, prompts) is
//! the content of the work, not chrome, and stays exactly as authored.

use anyhow::Result;
use serde::Serialize;
use std::path::Path;

/// The languages the interface is written in.
///
/// `en-GB` is the default: it is the language the strings were written in, so a
/// machine that has never chosen one reads the app as it was authored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum Language {
    #[default]
    #[serde(rename = "en-GB")]
    EnGb,
    #[serde(rename = "zh-CN")]
    ZhCn,
}

impl Language {
    /// Every language, in the order a picker lists them.
    pub const ALL: &'static [Language] = &[Language::EnGb, Language::ZhCn];

    /// The one a machine that has never chosen reads.
    pub const DEFAULT: Language = Language::EnGb;

    /// The tag the setting holds and every front-end names it by.
    pub fn tag(self) -> &'static str {
        match self {
            Self::EnGb => "en-GB",
            Self::ZhCn => "zh-CN",
        }
    }

    /// The language's name in English, for a listing that is read in another
    /// language.
    pub fn label(self) -> &'static str {
        match self {
            Self::EnGb => "English (United Kingdom)",
            Self::ZhCn => "Simplified Chinese",
        }
    }

    /// The language's name in the language itself, which is what a picker
    /// shows a reader looking for their own.
    pub fn native_label(self) -> &'static str {
        match self {
            Self::EnGb => "English (United Kingdom)",
            Self::ZhCn => "简体中文",
        }
    }

    /// Resolves a tag the way a person or a settings file writes one:
    /// `zh`, `zh_CN`, `zh-Hans` and `zh-CN` are all the Chinese interface, and
    /// any English tag is the one the app was written in. A tag naming neither
    /// is not a language this build has.
    pub fn parse(tag: &str) -> Option<Self> {
        let tag = tag.trim().to_ascii_lowercase().replace('_', "-");
        if tag.is_empty() {
            return None;
        }
        let primary = tag.split('-').next().unwrap_or_default();
        match primary {
            "en" => Some(Self::EnGb),
            "zh" => Some(Self::ZhCn),
            _ => None,
        }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.tag())
    }
}

/// One word or sentence of the interface: its key, the British English the app
/// is authored in, and the simplified Chinese it is shown as.
///
/// The key is what a front-end asks for, so a translation is added by naming
/// the key rather than by matching a sentence, and two front-ends asking for
/// the same word cannot drift apart.
pub const CATALOG: &[(&str, &str, &str)] = &[
    ("dialog.approval.alwaysHint", "Run it, and stop asking about this tool in this project", "运行此工具，并在此项目中不再询问"),
    ("dialog.approval.onceHint", "Run the tool this one time", "仅此次运行此工具"),
    ("dialog.approval.denyHint", "Refuse the tool and tell the agent to stop", "拒绝运行此工具并告知代理停止"),
    ("provider.storedMark", "stored", "已保存"),
    ("changes.filesChanged", "{count} files changed", "{count} 个文件已更改"),
    ("changes.fileChanged", "{count} file changed", "{count} 个文件已更改"),
    ("desktop.mcpEmpty", "No MCP servers configured. Add one with {command}, or a {file} in the project.", "尚未配置 MCP 服务器。使用 {command} 或项目中的 {file} 添加。"),
    ("desktop.threadReadError", "Could not read this project's threads: {error}", "无法读取此项目的对话：{error}"),
    ("desktop.mcpReadError", "Could not list MCP servers: {error}", "无法列出 MCP 服务器：{error}"),
    ("desktop.worktreeReadError", "Could not read this project's worktrees: {error}", "无法读取此项目的工作树：{error}"),
    ("desktop.empty.sidebar", "No projects yet. Add a folder with the + above.", "暂无项目。使用上方的 + 添加文件夹。"),
    ("desktop.empty.commands", "No matching command.", "没有匹配的命令。"),
    ("desktop.empty.threads", "No threads for this project yet. Send a message to start one.", "此项目暂无对话。发送消息即可开始。"),
    ("desktop.empty.loadingThreads", "Loading threads…", "正在加载对话…"),
    ("desktop.empty.checkingServers", "Checking servers…", "正在检查服务器…"),
    ("desktop.empty.checkingRelease", "Checking the newest release…", "正在检查最新版本…"),
    ("desktop.empty.approvals", "No saved approvals.", "暂无已保存的批准规则。"),
    ("desktop.empty.selectApprovals", "Select a project to review its tool approvals.", "选择项目以查看其工具批准规则。"),
    ("desktop.empty.themes", "No themes available.", "暂无可用主题。"),
    ("desktop.empty.loadingThemes", "Loading themes…", "正在加载主题…"),
    ("desktop.empty.models", "No models found.", "未找到模型。"),
    ("desktop.empty.loadingModels", "Loading…", "正在加载…"),
    ("desktop.empty.providers", "No provider matches that search.", "没有匹配此搜索的服务商。"),
    ("desktop.empty.projects", "No projects yet. Add a folder to run in.", "暂无项目。添加文件夹以开始运行。"),
    ("changes.moreCompactOne", "+{count} more file", "还有 {count} 个文件"),
    ("changes.moreCompact", "+{count} more files", "还有 {count} 个文件"),
    ("changes.deleted", "deleted", "已删除"),
    ("changes.modified", "modified", "已修改"),
    ("changes.added", "added", "已添加"),
    ("changes.noLines", "no line changes", "无行更改"),
    ("changes.binary", "binary", "二进制"),
    ("changes.uncommitted", "Uncommitted", "未提交"),
    ("changes.lastTurn", "Last turn", "上一轮"),
    ("usage.portkeyHint", "the Portkey account's spend is /spend", "使用 /spend 查看 Portkey 账户费用"),
    ("usage.manual", "manual", "手动"),
    ("usage.automatic", "auto", "自动"),
    ("usage.contextEmpty", "context: ? of {limit} ({mode} compaction)", "上下文：尚无用量 / {limit}（{mode}压缩）"),
    ("usage.context", "context: {percent}% of {limit} ({mode} compaction)", "上下文：{limit} 的 {percent}%（{mode}压缩）"),
    ("usage.output", "↓{count} out", "↓{count} 输出"),
    ("usage.input", "↑{count} in", "↑{count} 输入"),
    ("usage.thisChat", "this chat: {usage}", "此对话：{usage}"),
    ("language.saveError", "Could not save language: {error}", "无法保存语言：{error}"),
    ("language.available", "{current}; available: {languages}", "{current}；可选：{languages}"),
    ("tui.hotkeys", "keyboard shortcuts:\n  Enter                 send (steers the response while busy)\n  Shift+Enter           insert a newline\n  Alt+Enter             queue a follow-up while busy\n  {dequeue}pull queued messages back into the editor\n  Esc                   clear the input, or deny a waiting tool\n  Shift+Tab / Ctrl+R    cycle reasoning/thinking level\n  Ctrl+O                toggle tool output\n  Ctrl+T                show or hide thinking blocks\n  Ctrl+V                attach the clipboard image or copied file\n  Tab                   complete the selected command or @path\n  Ctrl+A / Ctrl+E       jump to the start/end of the message (when it is not empty)\n  Ctrl+Y                scroll up one line\n  Ctrl+E                scroll down one line (when the message is empty)\n  Ctrl+U / Ctrl+D       scroll half a page\n  PgUp / PgDn / wheel   scroll the transcript\n  Ctrl+G / Home         scroll to the top\n  End                   return to the latest message\n  Up / Down             input history\n  drag (mouse)          select text; copies on release\n  Ctrl+C                copy the selection, or quit\n  /exit                 quit Oxide\n  /copy                 copy the last assistant message\n  /copy all             copy the whole transcript", "键盘快捷键：\n  Enter                 发送（运行时引导当前回复）\n  Shift+Enter           换行\n  Alt+Enter             运行时排队后续消息\n  {dequeue}将排队消息取回编辑器\n  Esc                   清空输入或拒绝等待中的工具\n  Shift+Tab / Ctrl+R    循环切换思考强度\n  Ctrl+O                切换工具输出\n  Ctrl+T                显示或隐藏思考过程\n  Ctrl+V                添加剪贴板图片或已复制文件\n  Tab                   补全所选命令或 @path\n  Ctrl+A / Ctrl+E       跳到消息首尾（输入非空时）\n  Ctrl+Y                向上滚动一行\n  Ctrl+E                向下滚动一行（输入为空时）\n  Ctrl+U / Ctrl+D       滚动半页\n  PgUp / PgDn / wheel   滚动对话记录\n  Ctrl+G / Home         滚动到顶部\n  End                   返回最新消息\n  Up / Down             输入历史\n  拖动鼠标              选择文本，松开时复制\n  Ctrl+C                复制选区或退出\n  /exit                 退出 Oxide\n  /copy                 复制上一条助手消息\n  /copy all             复制完整对话记录"),
    ("cli.compaction", "[compaction] summarized {count} messages (~{tokens} tokens)", "[压缩] 已总结 {count} 条消息（约 {tokens} 个词元）"),
    ("cli.bytes", "[result: {name}] {count} bytes", "[结果：{name}] {count} 字节"),
    ("cli.matches", "[result: {name}] 0 matches", "[结果：{name}] 0 个匹配项"),
    ("cli.tool", "\n[tool] {name} {subject}", "\n[工具] {name} {subject}"),
    ("cli.retry", "[retry {attempt}/{max} in {delay}ms]", "[重试 {attempt}/{max}，等待 {delay} 毫秒]"),
    ("terminal.command.redo", "Reapply file changes", "重做文件更改"),
    ("terminal.command.undo", "Revert file changes", "撤销文件更改"),
    ("terminal.command.compact", "Summarize the conversation", "总结对话"),
    ("terminal.command.updates", "Check for a newer release at launch", "启动时检查新版本"),
    ("terminal.command.spend", "Configure the Portkey spend bar", "配置 Portkey 费用栏"),
    ("terminal.command.notify", "Toggle completion notifications and sound", "切换完成通知和声音"),
    ("terminal.command.marketplaces", "Browse and manage plugin marketplaces", "浏览和管理插件市场"),
    ("terminal.command.plugins", "Manage plugins and marketplaces", "管理插件和市场"),
    ("terminal.command.models", "Choose a model from any connected provider", "从所有已连接的服务商中选择模型"),
    ("terminal.command.init", "Create or update AGENTS.md", "创建或更新 AGENTS.md"),
    ("terminal.command.reload", "Reload config and skills", "重新加载配置和技能"),
    ("terminal.command.export", "Export the session to HTML", "将会话导出为 HTML"),
    ("terminal.command.status", "Show this session's file and id", "显示此会话的文件和 ID"),
    ("terminal.command.name", "Name the session", "命名会话"),
    ("terminal.command.clone", "Duplicate the session", "复制会话"),
    ("terminal.command.fork", "Branch a new session from a message", "从消息创建分支会话"),
    ("terminal.command.tree", "List user messages for branching", "列出用户消息以创建分支"),
    ("terminal.command.copy", "Copy the transcript to the clipboard", "将对话记录复制到剪贴板"),
    ("terminal.command.exit", "Quit Oxide", "退出 Oxide"),
    ("terminal.command.hotkeys", "Show keyboard shortcuts", "显示键盘快捷键"),
    ("tui.helpKeys", "keys: Enter send/steer · Shift+Enter newline · Alt+Enter queue for after · {key} edit queued · Shift+Tab/Ctrl+R reasoning · Ctrl+O tool details · Ctrl+T thinking · Ctrl+V image · Ctrl+A/E message start/end · ↑/↓ history · PgUp/PgDn/wheel scroll · Ctrl+U/D half page · drag to select and copy · Ctrl+C copy selection/quit", "快捷键：Enter 发送/引导 · Shift+Enter 换行 · Alt+Enter 排队 · {key} 编辑排队消息 · Shift+Tab/Ctrl+R 思考强度 · Ctrl+O 工具详情 · Ctrl+T 思考过程 · Ctrl+V 图片 · Ctrl+A/E 消息首尾 · ↑/↓ 历史 · PgUp/PgDn/滚轮 滚动 · Ctrl+U/D 半页 · 拖动选择并复制 · Ctrl+C 复制选区/退出"),
    ("terminal.help.copy", "Copy the last assistant message, or the whole transcript", "复制上一条助手消息或完整对话记录"),
    ("terminal.help.compact", "Summarize older context, optionally with a focus", "总结较早的上下文，可指定重点"),
    ("terminal.help.undo", "Revert or reapply the agent's file changes", "撤销或重做代理的文件更改"),
    ("terminal.help.updates", "Check for a newer release at launch; show the last one seen", "启动时检查新版本并显示上次发现的版本"),
    ("terminal.help.spend", "Configure the Portkey spend bar (dialog)", "配置 Portkey 费用栏（对话框）"),
    ("terminal.help.notify", "Show or set completion notifications (sound too)", "显示或设置完成通知及声音"),
    ("terminal.help.marketplaces", "Browse, add, and remove plugin marketplaces", "浏览、添加和移除插件市场"),
    ("terminal.help.plugins", "Manage plugins and marketplaces", "管理插件和市场"),
    ("terminal.help.models", "List models from every logged-in provider", "列出所有已登录服务商的模型"),
    ("terminal.help.init", "Create or update AGENTS.md for this project", "为此项目创建或更新 AGENTS.md"),
    ("terminal.help.reload", "Reload config, commands, and skills", "重新加载配置、命令和技能"),
    ("terminal.help.export", "Export the session to HTML", "将会话导出为 HTML"),
    ("terminal.help.clone", "Duplicate the current session", "复制当前会话"),
    ("terminal.help.fork", "Branch a new session from message n", "从第 n 条消息开始分支会话"),
    ("terminal.help.tree", "List branch points, or branch at message n", "列出分支点，或在第 n 条消息处创建分支"),
    ("terminal.help.status", "Show this session's file, id, name, and stats", "显示此会话的文件、ID、名称和统计信息"),
    ("terminal.help.name", "Name the current session", "命名当前会话"),
    ("terminal.help.skill", "Load a skill by name (also written /skill:<name>)", "按名称加载技能（也可写为 /skill:<name>）"),
    ("terminal.help.exit", "Quit Oxide", "退出 Oxide"),
    ("terminal.help.hotkeys", "Show the keyboard shortcuts", "显示键盘快捷键"),
    ("tui.helpTitle", "built-in commands:", "内置命令："),
    ("tui.escDeny", " {spinner} {question} · Esc deny ", " {spinner} {question} · Esc 拒绝 "),
    ("tui.moreAttachments", " • {count} more", " • 还有 {count} 个"),
    ("tui.deleteSession", "Delete session `{label}`?", "删除会话 `{label}`？"),
    ("tui.sessionLabel", "session: {label}", "会话：{label}"),
    ("tui.sessionsFiltered", " sessions · {filter} ", " 会话 · {filter} "),
    ("tui.error", "error: {error}", "错误：{error}"),
    ("cli.noClipboardMedia", "the clipboard holds no image or PDF to attach", "剪贴板中没有可添加的图片或 PDF"),
    ("cli.clearedTools", "forgot {count} allowed tool(s) for {path}", "已移除 {path} 的 {count} 条工具允许规则"),
    ("cli.noTools", "{path}: no tools allowed without prompting", "{path}：没有无需询问即可运行的工具"),
    ("cli.deleted", "deleted {id}", "已删除 {id}"),
    ("cli.aborted", "aborted", "已取消"),
    ("cli.deleteSessions", "delete {count} session(s)?", "删除 {count} 个会话？"),
    ("cli.noDeleteSessions", "no sessions to delete", "没有可删除的会话"),
    ("cli.noProjectSessions", "no sessions for this project", "此项目暂无会话"),
    ("cli.noSessions", "no sessions", "暂无会话"),
    ("extension.view.chat", "Chat", "对话"),
    ("state.off", "off", "关闭"),
    ("state.on", "on", "开启"),
    ("state.ask", "ask", "询问"),
    ("state.declined", "declined", "已拒绝"),
    ("state.untrusted", "untrusted", "未信任"),
    ("state.trusted", "trusted", "已信任"),
    ("state.unknown", "Unknown", "未知"),
    ("state.trust", "Needs trust", "需要信任"),
    ("state.auth", "Needs auth", "需要授权"),
    ("state.machine", "Machine credential", "机器凭据"),
    ("state.noKey", "No key needed", "无需密钥"),
    ("state.inUse", "In use", "使用中"),
    ("extension.workspaceTrust", "Oxide runs a coding agent that reads and writes files in your workspace.", "Oxide 会运行可读写工作区文件的编码代理。"),
    ("extension.description", "The Oxide coding agent inside VS Code: a chat panel and editor actions backed by the oxide CLI.", "在 VS Code 中使用 Oxide 编码代理：由 oxide CLI 驱动的对话面板和编辑器操作。"),
    ("extension.setting.projectTrust.never", "Pass --no-approve: ignore this workspace's project-local resources.", "传递 --no-approve：忽略此工作区的项目本地资源。"),
    ("extension.setting.projectTrust.always", "Pass --approve: load this workspace's .oxide agents, commands, skills, plugins and hooks.", "传递 --approve：加载此工作区的 .oxide 代理、命令、技能、插件及钩子。"),
    ("extension.setting.projectTrust.default", "Use the decision saved in trust.json, or defaultProjectTrust.", "使用 trust.json 中保存的决定，或 defaultProjectTrust。"),
    ("extension.setting.notifyOnFinish", "Show a notification when a run finishes while the chat view is hidden.", "对话视图隐藏时，在运行结束后显示通知。"),
    ("extension.setting.askApprovals", "Ask before running a tool a permission rule gates (`--ask-approvals`). The question appears in the transcript and the turn waits for it; **Always allow** is remembered per project in the shared `approvals.json`, so the terminal and the desktop app honour it too. Turn this off to run gated tools without a prompt (`--no-ask-approvals`). Either way the flag is passed explicitly, so this setting — not `askApprovals` in the shared `settings.json` — decides for a run started from the panel.", "运行受权限规则限制的工具前询问（--ask-approvals）。问题显示在对话记录中，回合会等待回答。“始终允许”按项目保存在共享 approvals.json 中，终端和桌面端也会遵循。关闭后无需询问即可运行受限工具（--no-ask-approvals）。此标志始终显式传递，因此由此设置决定面板启动的运行，而非共享 settings.json 中的 askApprovals。"),
    ("extension.setting.autoContext", "Track the file you are editing as a chip in the composer, sent with the next message as context. The chip is read when the message goes, so unsaved edits are included; its ✕ or **Clear** takes it out for as long as that file is the one being edited, and another file brings it back. Turn this off to attach only what you add by hand.", "在输入框中将正在编辑的文件作为标签追踪，并随下一条消息发送为上下文。发送时读取文件，因此包含未保存的更改。点击标签的 ✕ 或“清空”后，此文件将暂时不再自动附加，直到切换文件。关闭此设置后仅附加手动添加的内容。"),
    ("extension.setting.showThinking", "Show the model's reasoning in the transcript as a collapsible block.", "在对话记录中将模型思考过程显示为可折叠内容。"),
    ("extension.setting.followUpBehavior", "How follow-up messages behave while Oxide is running. Press Cmd/Ctrl+Shift+Enter to do the opposite for one message.", "Oxide 运行期间后续消息的送达方式。按 Cmd/Ctrl+Shift+Enter 可为单条消息使用另一种方式。"),
    ("extension.setting.additionalArguments", "Extra arguments appended to every oxide invocation.", "每次调用 oxide 时附加的额外参数。"),
    ("extension.setting.excludeTools", "Comma-separated tool denylist passed with --exclude-tools.", "通过 --exclude-tools 传递的工具禁止列表，以逗号分隔。"),
    ("extension.setting.tools", "Comma-separated tool allowlist passed with --tools (e.g. read,grep,find,ls for a read-only run).", "通过 --tools 传递的工具允许列表，以逗号分隔（例如 read,grep,find,ls 用于只读运行）。"),
    ("extension.setting.projectTrust", "Project trust for the workspace's own resources (`.oxide/agents`, `.oxide/commands`, `.oxide/skills`, `.oxide/plugins`, `SYSTEM.md`, and the Claude Code equivalents). With the default policy, the panel asks automatically when no shared trust decision is saved. Trust controls resource loading; file and shell access follows the operating-system user permissions.", "工作区自身资源的信任策略（.oxide/agents、.oxide/commands、.oxide/skills、.oxide/plugins、SYSTEM.md 及 Claude Code 对应资源）。默认策略下，未保存共享信任决定时面板会自动询问。信任决定资源是否加载；文件和 shell 访问遵循操作系统用户权限。"),
    ("extension.setting.reasoning", "Legacy setting. Reasoning now uses the shared Oxide configuration.", "旧版设置。思考强度现在使用 Oxide 共享配置。"),
    ("extension.setting.agent", "Agent passed with --agent, loaded from the workspace's .oxide/agents (or .claude/agents).", "通过 --agent 传递的代理，加载自工作区的 .oxide/agents（或 .claude/agents）。"),
    ("extension.setting.model", "Legacy setting. Model selection now uses the shared Oxide configuration.", "旧版设置。模型选择现在使用 Oxide 共享配置。"),
    ("extension.setting.binaryPath", "Path to the oxide binary. A bare name is looked up on PATH, then in ~/.local/bin and ~/.cargo/bin.", "oxide 二进制文件的路径。未指定路径时，依次从 PATH、~/.local/bin 和 ~/.cargo/bin 查找。"),
    ("extension.command.checkForUpdates", "Check for Updates...", "检查更新…"),
    ("extension.command.language", "Change Language...", "切换语言…"),
    ("extension.command.worktrees", "Worktrees...", "工作树…"),
    ("extension.command.mcpServers", "MCP Servers...", "MCP 服务器…"),
    ("extension.command.connectProvider", "Connect Provider...", "连接服务商…"),
    ("extension.command.setProjectTrust", "Set Project Trust...", "设置项目信任…"),
    ("extension.command.setReasoning", "Set Reasoning Effort...", "设置思考强度…"),
    ("extension.command.setAgent", "Set Agent...", "设置代理…"),
    ("extension.command.setModel", "Set Model...", "设置模型…"),
    ("extension.command.showOutput", "Show Output Channel", "显示输出通道"),
    ("extension.command.openTerminal", "Open Terminal (TUI)", "打开终端（TUI）"),
    ("extension.command.reviewChanges", "Review Working Tree Changes", "审查工作区更改"),
    ("extension.command.fixSelection", "Fix Selection", "修复选中内容"),
    ("extension.command.explainSelection", "Explain Selection", "解释选中内容"),
    ("extension.command.askAboutSelection", "Ask About Selection", "询问选中内容"),
    ("extension.command.addToChat", "Add File or Selection to Chat", "将文件或选中内容添加到对话"),
    ("extension.command.stop", "Stop", "停止"),
    ("extension.command.continueSession", "Continue Last Session", "继续上次会话"),
    ("extension.command.resumeSession", "Session History", "会话历史"),
    ("extension.command.newSession", "New Chat", "新建对话"),
    ("extension.command.insertReference", "Insert File Reference", "插入文件引用"),
    ("extension.command.focusInput", "Focus Input", "聚焦输入框"),
    ("extension.command.openChat", "Open Chat", "打开对话"),
    ("approvals.forgetRule", "Forget {count} rule for this project", "移除此项目的 {count} 条规则"),
    ("approvals.forgetRules", "Forget {count} rules for this project", "移除此项目的 {count} 条规则"),
    ("changes.moreLine", "⋯ {count} more line · click to expand", "⋯ 还有 {count} 行 · 点击展开"),
    ("changes.moreLines", "⋯ {count} more lines · click to expand", "⋯ 还有 {count} 行 · 点击展开"),
    ("ui.modelATurnRunsWithModelClickToChangeIt", "Model a turn runs with (--model){value0}. Click to change it.", "本轮使用的模型（--model）{value0}。点击更改。"),
    ("ui.on", " on {value0}", "，服务商 {value0}"),
    ("ui.reasoningEffortReasoningClickToChooseALevel", "Reasoning effort (--reasoning): {value0}. Click to choose a level.", "思考强度（--reasoning）：{value0}。点击选择。"),
    ("ui.access", "access: {value0}", "访问权限：{value0}"),
    ("agent.label", "agent: {value0}", "代理：{value0}"),
    ("ui.subagentThisChatRunsWithAgentDiscoveredClickToPickOne", "Subagent this chat runs with (--agent). {value0} discovered. Click to pick one.", "此对话使用的子代理（--agent）。已发现 {value0} 个。点击选择。"),
    ("ui.subagentThisChatRunsWithAgentNoAgentsWereFoundInThisProjectOrTheGlobalConfigClic", "Subagent this chat runs with (--agent). No agents were found in this project or the global config. Click to type one.", "此对话使用的子代理（--agent）。项目和全局配置中未发现代理。点击输入名称。"),
    ("ui.projectResourcesOxideAgentsCommandsSkillsAndPluginsLoadForThisRun", "Project resources (.oxide agents, commands, skills and plugins) load for this run.", "本轮会加载项目资源（.oxide 代理、命令、技能和插件）。"),
    ("ui.projectResourcesAreSkippedForThisRun", "Project resources are skipped for this run.", "本轮跳过项目资源。"),
    ("ui.oxideProjectTrustIsAlwaysApprove", " oxide.projectTrust is always (--approve).", " oxide.projectTrust 为 always（--approve）。"),
    ("ui.oxideProjectTrustIsNeverNoApprove", " oxide.projectTrust is never (--no-approve).", " oxide.projectTrust 为 never（--no-approve）。"),
    ("ui.noSavedDecisionForThisFolderDefaultProjectTrust", " No saved decision for this folder (defaultProjectTrust: {value0}).", " 此文件夹尚未保存决定（defaultProjectTrust：{value0}）。"),
    ("ui.savedDecisionForThisFolder", " Saved decision for this folder: {value0}.", " 此文件夹已保存的决定：{value0}。"),
    ("ui.clickToChangeIt", "{value0}{value1}{value2} Click to change it.", "{value0}{value1}{value2} 点击更改。"),
    ("ui.moveThisChatAndItsChangesToTheDestinationMustHaveNoUncommittedChanges", "Move this chat and its changes to {value0}? The destination must have no uncommitted changes.", "将此对话及其更改移动到{value0}？目标位置不能有未提交的更改。"),
    ("ui.itsWorktree", "its worktree", "此对话的工作树"),
    ("ui.local", "Local", "本地"),
    ("ui.chatMovedTo", "Chat moved to {value0}.", "对话已移至{value0}。"),
    ("ui.removingAWorktree", "removing a worktree", "移除工作树"),
    ("ui.creatingABranch", "creating a branch", "创建分支"),
    ("ui.handingOffThisChat", "handing off this chat", "移交此对话"),
    ("ui.startingANewThread", "starting a new thread", "开始新对话"),
    ("ui.sendingAMessage", "sending a message", "发送消息"),
    ("ui.restartingOxide", "restarting Oxide", "重启 Oxide"),
    ("ui.waitForTheHandoffBefore", "Wait for the handoff before {value0}.", "请等待移交完成后再{value0}。"),
    ("ui.aTurnIsRunningStopItBefore", "A turn is running; stop it before {value0}.", "请先停止正在运行的回合，再{value0}。"),
    ("ui.searchFor", "Search for", "搜索"),
    ("ui.find", "Find", "查找"),
    ("ui.in", "{value0} {value1} in {value2}", "在 {value2} 中{value0} {value1}"),
    ("ui.0Matches", "0 matches{value0}", "0 个匹配项{value0}"),
    ("update.currentVersion", "Current {value0}", "当前 {value0}"),
    ("ui.latest", "Latest {value0}", "最新 {value0}"),
    ("ui.download", "Download {value0}", "下载 {value0}"),
    ("ui.downloading", "Downloading {value0}", "正在下载 {value0}"),
    ("ui.release", "Release {value0}", "版本 {value0}"),
    ("update.downloadedAsset", "Downloaded {value0}", "已下载 {value0}"),
    ("ui.disableTurnItOffInTheFileThatDefinesIt", "Disable {value0}: turn it off in the file that defines it", "禁用 {value0}：在定义此服务器的文件中关闭"),
    ("ui.enableTurnItBackOn", "Enable {value0}: turn it back on", "启用 {value0}：重新开启"),
    ("ui.turningOn", "Turning on", "正在启用"),
    ("ui.turningOff", "Turning off", "正在禁用"),
    ("state.enabledLower", "enabled", "已启用"),
    ("state.disabledLower", "disabled", "已禁用"),
    ("ui.couldNotChange", "Could not change {value0}: {value1}", "无法更改 {value0}：{value1}"),
    ("ui.aTurnIsRunning", "{value0}a turn is running", "{value0}有回合正在运行"),
    ("ui.nameAProviderLogout", "Name a provider: /logout {value0}", "请指定服务商：/logout {value0}"),
    ("ui.forgetTheStoredCredentialFor", "Forget the stored credential for {value0}?", "移除 {value0} 已保存的凭据？"),
    ("ui.signedOutOf", "Signed out of {value0}", "已退出 {value0} 的登录"),
    ("ui.reasoningMustBeOneOf", "Reasoning must be one of {value0}.", "思考强度必须为以下值之一：{value0}。"),
    ("ui.desktopApp", "desktop app", "桌面应用"),
    ("ui.vSCodePanel", "VS Code panel", "VS Code 面板"),
    ("ui.isTheSCommand", "/{value0} is the {value1}'s command.", "/{value0} 是 {value1} 的命令。"),
    ("ui.isNotAvailableInTheDesktopAppYet", "/{value0} is not available in the desktop app yet.", "桌面应用尚不支持 /{value0}。"),
    ("ui.pin", "pin", "置顶"),
    ("ui.unpin", "unpin", "取消置顶"),
    ("ui.couldNotProject", "Could not {value0} project: {value1}", "无法{value0}项目：{value1}"),
    ("ui.couldNotThread", "Could not {value0} thread: {value1}", "无法{value0}对话：{value1}"),
    ("model.useConfigured", "Use {value0}, the model stored in the Oxide config", "使用 Oxide 配置中保存的模型 {value0}"),
    ("ui.availableFrom", "Available from {value0}{value1}", "由 {value0} 提供{value1}"),
    ("ui.use", "Use “{value0}”", "使用“{value0}”"),
    ("ui.noToolIsAllowedWithoutPromptingInAnAlwaysAllowAnswerAddsOne", "No tool is allowed without prompting in {value0}. An “Always allow” answer adds one.", "{value0} 中没有无需询问即可运行的工具。选择“始终允许”可添加规则。"),
    ("ui.runsWithoutAskingIn", "Runs without asking in {value0}", "在 {value0} 中无需询问即可运行"),
    ("ui.noStoredCredentialMatches", "No stored credential matches “{value0}”.", "没有匹配“{value0}”的已保存凭据。"),
    ("ui.disable", "Disable {value0}", "禁用 {value0}"),
    ("ui.enable", "Enable {value0}", "启用 {value0}"),
    ("ui.sOwnFolderTheCheckoutYouHaveOpen", "{value0}'s own folder — the checkout you have open", "{value0} 的原始文件夹 — 当前打开的检出"),
    ("ui.installing", "Installing {value0}", "正在安装 {value0}"),
    ("ui.oxideIsInstalled", "Oxide {value0} is installed", "Oxide {value0} 已安装"),
    ("ui.was", "Was {value0}", "之前为 {value0}"),
    ("ui.installed", "Installed {value0}", "已安装 {value0}"),
    ("ui.replaces", "Replaces {value0}", "替换 {value0}"),
    ("ui.install", "Install {value0}", "安装 {value0}"),
    ("ui.downloadsAndInstalls", "Downloads and installs {value0}", "下载并安装 {value0}"),
    ("ui.updateTheOxideCLITo", "Update the oxide CLI to {value0}", "将 oxide CLI 更新至 {value0}"),
    ("ui.runsOxideUpdateReplacesTheBinaryEveryTurnRunsThrough", "Runs oxide update — replaces the {value0} binary every turn runs through", "运行 oxide update — 替换每一轮运行使用的 {value0} 二进制文件"),
    ("ui.oxideIsAvailable", "Oxide {value0} is available", "Oxide {value0} 已发布"),
    ("ui.theOxideCLIIsAvailable", "The oxide CLI {value0} is available", "oxide CLI {value0} 已发布"),
    ("ui.oxideIsUpToDate", "Oxide {value0} is up to date", "Oxide {value0} 已是最新版本"),
    ("ui.thisExtensionIsCurrentTheOxideCLIThisPanelRunsEveryTurnThroughIsAtUpdatingItIsOn", "This extension is current. The oxide CLI this panel runs every turn through is at {value0}: updating it is one row, and it is the binary the terminal and the desktop app share.", "此扩展已是最新版本。此面板使用的 oxide CLI 版本为 {value0}：可通过下方选项更新，终端和桌面应用也使用此二进制文件。"),
    ("ui.oxideIsTheNewestReleasedVersionOfThisExtension", "Oxide {value0} is the newest released version of this extension.", "Oxide {value0} 是此扩展的最新发布版本。"),
    ("tui.changelog", "Changelog: {url}", "更新日志：{url}"),
    ("tui.newVersion", "New version {version} is available. Run `{command}`", "新版本 {version} 已发布。运行 `{command}`"),
    ("tui.updateAvailable", "Update Available", "有可用更新"),
    ("time.daysAgo", "{count}d ago", "{count} 天前"),
    ("time.hoursAgo", "{count}h ago", "{count} 小时前"),
    ("time.minutesAgo", "{count}m ago", "{count} 分钟前"),
    ("tui.trustProject", " Trust project {path}?", " 信任项目 {path}？"),
    ("command.logout", "Forget a provider's stored credentials", "移除服务商保存的凭据"),
    ("command.connect", "Sign in to a provider", "登录服务商"),
    ("command.language", "Switch the interface language", "切换界面语言"),
    ("command.theme", "Choose the color theme", "选择配色主题"),
    ("command.attach", "Attach images, PDFs or files", "添加图片、PDF 或文件"),
    ("command.usage", "Show this chat's tokens, cost and context", "显示此对话的词元、费用和上下文用量"),
    ("command.new", "Start a new session", "开始新会话"),
    ("command.session", "Resume a session from this project", "恢复此项目的会话"),
    ("command.trust", "Decide whether this project's own resources load", "决定是否加载此项目的资源"),
    ("command.permissions", "Review the tools allowed without prompting", "查看无需询问即可运行的工具"),
    ("command.agent", "Choose the subagent this chat runs as", "选择此对话使用的子代理"),
    ("command.reasoning", "Set the reasoning effort for this chat", "设置此对话的思考强度"),
    ("command.model", "Choose the model to run", "选择运行的模型"),
    ("command.mcp", "List MCP servers and their connection status", "列出 MCP 服务器及连接状态"),
    ("command.help", "List the commands and keyboard shortcuts", "列出命令和键盘快捷键"),
    ("tui.status.ResponseFinishedPressEnterToSendAsTheNextTurn", "response finished; press Enter to send as the next turn", "回复已结束；按 Enter 作为下一轮发送"),
    ("tui.status.QueuedFollowUpSent", "queued follow-up sent", "已发送排队中的后续消息"),
    ("tui.status.RunningTool", "running tool...", "正在运行工具…"),
    ("tui.status.SteeringActiveResponse", "steering active response...", "正在引导当前回复…"),
    ("tui.status.QueuedAsTheNextTurn", "queued as the next turn...", "已排队为下一轮…"),
    ("tui.status.SummarizingBranch", "summarizing branch...", "正在生成分支摘要…"),
    ("tui.status.LoadingModels", "loading models...", "正在加载模型…"),
    ("tui.status.LoadingPlugins", "loading plugins...", "正在加载插件…"),
    ("tui.status.Marketplaces", "marketplaces", "市场"),
    ("tui.status.CheckingMcpServers", "checking MCP servers...", "正在检查 MCP 服务器…"),
    ("tui.status.Connecting", "connecting...", "正在连接…"),
    ("tui.status.Compacting", "compacting...", "正在压缩…"),
    ("tui.status.Thinking", "thinking...", "正在思考…"),
    ("tui.status.Ready", "ready", "就绪"),
    ("ui.dragToResizeDoubleClickToReset", "Drag to resize · double-click to reset", "拖动以调整大小 · 双击重置"),
    ("ui.resizeSidebar", "Resize sidebar", "调整侧边栏大小"),
    ("ui.chatTabs", "Chat tabs", "对话标签页"),
    ("ui.jumpToAMessage", "Jump to a message", "跳到消息"),
    ("ui.startingBranchOrCommit", "Starting branch or commit", "起始分支或提交"),
    ("ui.changeScope", "Change scope", "更改范围"),
    ("ui.slashCommands", "Slash commands", "斜杠命令"),
    ("ui.filesAndFolders", "Files and folders", "文件与文件夹"),
    ("ui.noThreadMatches", "No thread matches “{value0}”.", "没有匹配“{value0}”的对话。"),
    ("ui.noModelMatches", "No model matches “{value0}”.", "没有匹配“{value0}”的模型。"),
    ("ui.noAgentMatches", "No agent matches “{value0}”.", "没有匹配“{value0}”的代理。"),
    ("ui.isAServerOnThisMachineNoKeyNeeded", "{value0} is a server on this machine — no key needed.", "{value0} 是本机服务器 — 无需密钥。"),
    ("ui.authorizesWithACredentialThisMachineAlreadyHasNoKeyNeeded", "{value0} authorizes with a credential this machine already has — no key needed.", "{value0} 使用此机器已有的凭据授权 — 无需密钥。"),
    ("ui.noRequestYetWindow", "No request yet · {value0} window", "尚无请求 · 窗口 {value0}"),
    ("ui.contextWindow", "context window: {value0}", "上下文窗口：{value0}"),
    ("ui.onBranch", "On branch {value0}{value1}", "当前分支 {value0}{value1}"),
    ("ui.hEADIsDetachedAt", "HEAD is detached at {value0}{value1}", "HEAD 已分离，位于 {value0}{value1}"),
    ("ui.model", "model: {value0}{value1}", "模型：{value0}{value1}"),
    ("model.label", "model: {value0}", "模型：{value0}"),
    ("ui.thinking", "thinking: {value0}", "思考强度：{value0}"),
    ("ui.project", "project: {value0}\n{value1}", "项目：{value0}\n{value1}"),
    ("project.label", "project: {value0}", "项目：{value0}"),
    ("ui.worktree", "worktree: {value0}", "工作树：{value0}"),
    ("worktree.label", "worktree {value0}", "工作树 {value0}"),
    ("ui.worktreeRunAThreadInACopyOfTheProjectOnItsOwnBranch", "Worktree\nRun a thread in a copy of the project on its own branch", "工作树\n在项目副本的独立分支中运行对话"),
    ("ui.switchTheCheckoutTheNextTurnRunsIn", "{value0}\n{value1}\nSwitch the checkout the next turn runs in", "{value0}\n{value1}\n切换下一轮运行所在的检出"),
    ("ui.thisProjectSOwnFolder", "this project's own folder{value0}", "此项目的原始文件夹{value0}"),
    ("ui.removeTheWorktreeAtItsCommitsWillBeKeptOnABranchInTheRepositoryUncommittedChange", "Remove the worktree {value0} at {value1}?\n\nIts commits will be kept on a branch in the repository. Uncommitted changes prevent removal.", "移除位于 {value1} 的工作树 {value0}？\n\n提交将保留在仓库的分支中。有未提交的更改时无法移除。"),
    ("ui.removeTheWorktreeAt", "Remove the worktree {value0} at {value1}?", "移除位于 {value1} 的工作树 {value0}？"),
    ("ui.removeFromTheSidebarItsSessionsStayOnDiskUseOxideSessionsDeleteFromTheProjectFol", "Remove “{value0}” from the sidebar?\n\nIts sessions stay on disk. Use oxide sessions delete from the project folder to delete sessions permanently.", "从侧边栏移除“{value0}”？\n\n会话仍保留在磁盘上。要永久删除会话，请在项目文件夹中运行 oxide sessions delete。"),
    ("ui.aTurnIsRunningInOpenItToQueueOrSteerOrStopIt", "A turn is running in “{value0}”; open it to queue or steer, or stop it.", "“{value0}”中有回合正在运行；打开它即可排队或引导，或将其停止。"),
    ("ui.queued", "Queued: {value0}", "已排队：{value0}"),
    ("ui.anAttachment", "an attachment", "一个附件"),
    ("ui.anotherThread", "another thread", "另一个对话"),
    ("ui.isTheAttachmentLimitIs", "{value0} is {value1}; the attachment limit is {value2}", "{value0} 大小为 {value1}；附件大小上限为 {value2}"),
    ("ui.isBeingLookedUp", "{value0} is being looked up", "正在查找 {value0}"),
    ("ui.isBeingVerified", "{value0} is being verified", "正在验证 {value0}"),
    ("ui.isBeingPutInPlace", "{value0} is being put in place", "正在安装 {value0}"),
    ("ui.oxideIsBeingDownloadedAndPutInPlace", "Oxide {value0} is being downloaded and put in place", "正在下载并安装 Oxide {value0}"),
    ("ui.theReleaseIsBeingDownloadedAndPutInPlace", "The release is being downloaded and put in place", "正在下载并安装此版本"),
    ("ui.theAppKeepsRunningUntilItIsRestarted", "{value0}. The app keeps running until it is restarted.", "{value0}。应用将在重启前继续运行。"),
    ("ui.downloaded", "{value0} downloaded", "已下载 {value0}"),
    ("ui.of", "{value0} of {value1} · {value2}%", "{value0} / {value1} · {value2}%"),
    ("ui.theOxideInstallerIsRunning", "The Oxide {value0} installer is running", "Oxide {value0} 安装程序正在运行"),
    ("ui.theCheckAsksGitHubForTheNewestReleaseSoAMachineWithNoNetworkOrARequestTheNetwork", "The check asks GitHub for the newest release, so a machine with no network — or a request the network refuses — says so here.", "检查更新需要向 GitHub 查询最新版本。网络不可用或请求被拒绝时将在此显示。"),
    ("ui.installingDownloadsTheReleaseForThisMachineAndPutsItInThisAppSPlace", "Installing downloads the release for this machine and puts it in this app's place. ", "安装会下载适用于此机器的版本并替换当前应用。"),
    ("update.keepsRunning", "The app keeps running until it is restarted.", "应用将在重启前继续运行。"),
    ("ui.aFolderInThisProject", "{value0} — a folder in this project", "{value0} — 此项目中的文件夹"),
    ("ui.referencedInThisMessage", "{value0} — referenced in this message", "{value0} — 此消息中的引用"),
    ("ui.source", "source: {value0}", "来源：{value0}"),
    ("ui.thisChat", "This chat: {value0}", "此对话：{value0}"),
    ("ui.nothingToDecideThisProjectNeedsNoTrustDecision", "Nothing to decide — this project needs no trust decision.", "无需决定 — 此项目无需信任确认。"),
    ("ui.checking", "checking", "正在检查"),
    ("ui.setupRequired", "setup required", "需要设置"),
    ("ui.toolOutputExpanded", "tool output expanded", "已展开工具输出"),
    ("ui.toolOutputCollapsed", "tool output collapsed", "已收起工具输出"),
    ("ui.checkingMCPServers", "Checking MCP servers", "正在检查 MCP 服务器"),
    ("ui.loadingPlugins", "Loading plugins", "正在加载插件"),
    ("ui.noSavedDecision", "no saved decision", "尚未保存决定"),
    ("ui.noReleaseSeenYet", "no release seen yet", "尚未发现版本"),
    ("ui.sentATestNotification", "sent a test notification", "已发送测试通知"),
    ("ui.gatedToolsRunWithoutAsking", "gated tools run without asking", "受限工具无需询问即可运行"),
    ("ui.askedBeforeAGatedToolRuns", "asked before a gated tool runs", "受限工具运行前需要询问"),
    ("ui.noTokensReportedYet", "no tokens reported yet", "尚未报告词元用量"),
    ("ui.portkeyUsageBarOff", "portkey usage bar off", "Portkey 用量栏已关闭"),
    ("ui.portkeyMonthBudgetCleared", "portkey month budget cleared", "Portkey 月度预算已清空"),
    ("ui.portkeyUsageSettingsSaved", "portkey usage settings saved", "Portkey 用量设置已保存"),
    ("ui.budgetMustBeAPositiveNumber", "budget must be a positive number", "预算必须为正数"),
    ("ui.branchSummaryFailed", "branch summary failed", "分支摘要生成失败"),
    ("ui.noAttachments", "no attachments", "暂无附件"),
    ("ui.modelUnchanged", "model unchanged", "模型未更改"),
    ("ui.sessionUnchanged", "session unchanged", "会话未更改"),
    ("ui.marketplacesClosed", "marketplaces closed", "市场已关闭"),
    ("ui.providerReturnedNoModels", "provider returned no models", "服务商未返回模型"),
    ("ui.noQueuedMessagesToRestore", "no queued messages to restore", "没有可恢复的排队消息"),
    ("ui.nothingToCopy", "nothing to copy", "没有可复制的内容"),
    ("ui.projectResourcesNotTrusted", "project resources not trusted", "尚未信任项目资源"),
    ("ui.connectCancelled", "connect cancelled", "已取消连接"),
    ("ui.waitingForAnAnswer", "waiting for an answer", "正在等待回答"),
    ("ui.aQuestionArrivedThatThisTerminalCannotAnswer", "a question arrived that this terminal cannot answer", "收到一个此终端无法回答的问题"),
    ("ui.steeringMessageSent", "steering message sent", "引导消息已发送"),
    ("ui.nothingToAttachFromTheClipboard", "nothing to attach from the clipboard", "剪贴板中没有可添加的附件"),
    ("ui.ctrlTToExpand", "Ctrl+T to expand", "Ctrl+T 展开"),
    ("ui.you", "you", "你"),
    ("ui.baseURL", "Base URL", "基础 URL"),
    ("ui.portkeyConfig", "Portkey Config", "Portkey 配置"),
    ("ui.user", "User", "用户"),
    ("ui.metadata", "Metadata", "元数据"),
    ("ui.budget", "Budget", "预算"),
    ("ui.currency", "Currency", "币种"),
    ("ui.endpoint", "Endpoint", "端点"),
    ("reasoning.max", "Max", "最高"),
    ("reasoning.xhigh", "Xhigh", "超高"),
    ("reasoning.high", "High", "高"),
    ("reasoning.medium", "Medium", "中"),
    ("reasoning.low", "Low", "低"),
    ("reasoning.minimal", "Minimal", "最低"),
    ("reasoning.off", "Off", "关闭"),
    ("reasoning.auto", "Auto", "自动"),
    ("reasoning.hint.max", "The most reasoning", "最高思考强度"),
    ("reasoning.hint.xhigh", "Extra-high reasoning", "超高思考强度"),
    ("reasoning.hint.high", "A lot of reasoning", "较高思考强度"),
    ("reasoning.hint.medium", "Balanced reasoning", "均衡思考强度"),
    ("reasoning.hint.low", "A little reasoning", "较低思考强度"),
    ("reasoning.hint.minimal", "The least reasoning", "最低思考强度"),
    ("reasoning.hint.off", "Turn thinking off where the model allows it", "在模型允许时关闭思考"),
    ("reasoning.hint.auto", "Let the provider decide", "由服务商决定"),
    ("run.workedFor", "Worked for {duration}", "已处理 {duration}"),
    ("run.workingFor", "Working for {duration}", "正在处理，已用时 {duration}"),
    ("changes.contextLine", "{count} unmodified line", "{count} 行未更改"),
    ("changes.contextLines", "{count} unmodified lines", "{count} 行未更改"),
    ("changes.undoConfirmOne", "Put {count} file back to how it was before this turn?", "将 {count} 个文件还原至本轮开始前的状态？"),
    ("changes.undoConfirm", "Put {count} files back to how they were before this turn?", "将 {count} 个文件还原至本轮开始前的状态？"),
    ("changes.moreFile", "Show {count} more file", "再显示 {count} 个文件"),
    ("changes.moreFiles", "Show {count} more files", "再显示 {count} 个文件"),
    ("changes.editedFile", "Edited {count} file", "已编辑 {count} 个文件"),
    ("changes.editedFiles", "Edited {count} files", "已编辑 {count} 个文件"),
    ("session.messageCountOne", "{count} message", "{count} 条消息"),
    ("session.messageCount", "{count} messages", "{count} 条消息"),
    ("sidebar.activeCount", "{count} active", "{count} 个运行中"),
    ("sidebar.taskCountOne", "{count} task", "{count} 个任务"),
    ("sidebar.taskCount", "{count} tasks", "{count} 个任务"),
    ("ui.oxideConfigDefault", "Oxide config default", "Oxide 配置默认值"),
    ("ui.chooseAModelFromAnyProviderYouHaveConnectedPickingOneFromAnotherProviderSwitches", "Choose a model from any provider you have connected. Picking one from another provider switches to it, which the terminal and the desktop app read too.", "从已连接的服务商中选择模型。选择其他服务商的模型会同时切换服务商；终端和桌面端也会读取此设置。"),
    ("ui.chooseAModelAvailableFromTheActiveProviderOrEnterAnyModelID", "Choose a model available from the active provider, or enter any model ID.", "选择当前服务商提供的模型，或输入任意模型 ID。"),
    ("ui.useTheModelStoredInTheOxideConfig", "Use the model stored in the Oxide config", "使用 Oxide 配置中保存的模型"),
    ("ui.currentModelOverride", "Current model override", "当前模型覆盖值"),
    ("ui.passThisModelIDWithModel", "Pass this model ID with --model", "通过 --model 传递此模型 ID"),
    ("ui.filterOrEnterAModelID", "Filter or enter a model ID…", "筛选或输入模型 ID…"),
    ("ui.agent", "Agent", "代理"),
    ("ui.chooseADiscoveredAgentOrTypeAnAgentNameBelow", "Choose a discovered agent, or type an agent name below.", "选择已发现的代理，或在下方输入代理名称。"),
    ("ui.noConfiguredAgentsWereDiscoveredYouCanStillEnterAName", "No configured agents were discovered; you can still enter a name.", "未发现已配置的代理；仍可输入名称。"),
    ("ui.runWithoutASubagentPrompt", "Run without a subagent prompt", "不使用子代理提示运行"),
    ("ui.passThisNameWithAgent", "Pass this name with --agent", "通过 --agent 传递此名称"),
    ("ui.filterOrEnterAnAgentName", "Filter or enter an agent name…", "筛选或输入代理名称…"),
    ("ui.theEffortPassedToTheNextTurnWithReasoning", "The effort passed to the next turn with --reasoning.", "通过 --reasoning 传给下一轮的思考强度。"),
    ("ui.openAFolderFirst", "Open a folder first.", "请先打开文件夹。"),
    ("ui.savedApprovals", "Saved approvals", "已保存的批准规则"),
    ("ui.toolsThisProjectRunsWithoutAskingClearingForgetsEveryRuleHere", "Tools this project runs without asking. Clearing forgets every rule here.", "此项目无需询问即可运行的工具。清空将移除此处的所有规则。"),
    ("ui.noProviderHasAStoredCredentialConnectAddsOne", "No provider has a stored credential. /connect adds one.", "没有服务商保存了凭据。使用 /connect 添加。"),
    ("ui.storedProviders", "Stored providers", "已保存的服务商"),
    ("ui.signOutOfAProviderTheCredentialIsRemovedWhereItIsStoredAndATurnThatUsedItSwitche", "Sign out of a provider. The credential is removed where it is stored, and a turn that used it switches to another logged-in provider.", "退出服务商登录并移除已保存的凭据。使用该服务商的回合将切换到其他已登录的服务商。"),
    ("ui.searchStoredProviders", "Search stored providers…", "搜索已保存的服务商…"),
    ("ui.trustingThisFolderLoadsItsLocalResourcesWhichCanRunCommandsOrChangeHowTheAgentBe", "Trusting this folder loads its local resources, which can run commands or change how the agent behaves. The decision is saved in trust.json and shared with the CLI and desktop.", "信任此文件夹将加载其本地资源，这些资源可能运行命令或改变代理行为。此决定保存在 trust.json 中，由 CLI 和桌面端共享。"),
    ("ui.useTheSavedDecision", "Use the saved decision", "使用已保存的决定"),
    ("ui.followTrustJsonOrDefaultProjectTrust", "Follow trust.json or defaultProjectTrust", "遵循 trust.json 或 defaultProjectTrust"),
    ("ui.trustThisFolder", "Trust this folder", "信任此文件夹"),
    ("ui.writeTrustJsonEveryFrontEndReadsTheSameDecision", "Write trust.json — every front-end reads the same decision", "写入 trust.json — 所有前端读取同一个决定"),
    ("ui.neverTrustThisFolder", "Never trust this folder", "从不信任此文件夹"),
    ("ui.theServersThisProjectLoadsAndWhetherOxideCanReachThemAToggleIsWrittenToTheFileTh", "The servers this project loads, and whether Oxide can reach them. A toggle is written to the file that defines the server — the same change the terminal's /mcp makes.", "此项目加载的服务器及其连接状态。开关会写入定义该服务器的文件，与终端 /mcp 的更改相同。"),
    ("ui.noMCPServersConfiguredForThisProjectAddOneWithOxideMcpAddOrAnMcpJsonInTheProject", "No MCP servers configured for this project. Add one with oxide mcp add, or an .mcp.json in the project.", "此项目尚未配置 MCP 服务器。使用 oxide mcp add 或项目中的 .mcp.json 添加。"),
    ("ui.runAChatInAnIsolatedCheckoutStartFromABranchOrCommitThenCreateABranchOrHandTheCh", "Run a chat in an isolated checkout. Start from a branch or commit, then create a branch or hand the chat back to Local when ready.", "在独立检出中运行对话。从分支或提交开始，完成后可创建分支或将对话移交回本地。"),
    ("ui.noWorktreesYetMakeOneFromTheNameOfTheTask", "No worktrees yet. Make one from the name of the task.", "暂无工作树。可使用任务名称创建。"),
    ("ui.newWorktree", "New worktree…", "新建工作树…"),
    ("ui.namesACheckoutAndStartsAChatInIt", "Names a checkout and starts a chat in it", "命名检出并在其中开始对话"),
    ("ui.noSessionsForThisProjectYetTheNextMessageStartsOne", "No sessions for this project yet — the next message starts one.", "此项目暂无会话 — 下一条消息将开始新会话。"),
    ("ui.threadsStoredForThisProjectPinnedFirstThenMostRecentPickingOneOpensItHere", "Threads stored for this project, pinned first, then most recent. Picking one opens it here.", "此项目保存的对话，置顶优先，其余按最近时间排序。选择一个即可在此打开。"),
    ("ui.closeThisThreadAndStartAFreshOne", "Close this thread and start a fresh one", "关闭此对话并开始新对话"),
    ("ui.startAFreshThread", "Start a fresh thread", "开始新对话"),
    ("ui.continueMostRecentSession", "Continue most recent session", "继续最近的会话"),
    ("ui.pickUpTheNewestSessionForThisProject", "Pick up the newest session for this project", "继续此项目最新的会话"),
    ("ui.openInThisPanelTheStoreHasNoFileForItYet", "Open in this panel — the store has no file for it yet", "在此面板中打开 — 尚未保存为文件"),
    ("ui.running", "Running", "运行中"),
    ("ui.pinned", "Pinned", "已置顶"),
    ("ui.undoTurn", "Undo turn", "撤销回合"),
    ("changes.restoreTurnFiles", "Put this turn's files back to how the run found them.", "将本轮文件还原至运行前的状态。"),
    ("ui.anEditMadeSinceTheTurnIsRefusedRatherThanUndone", "An edit made since the turn is refused rather than undone.", "本轮结束后又被编辑的文件不会被撤销。"),
    ("ui.keepTheTurnSFiles", "Keep the turn's files", "保留本轮文件"),
    ("ui.checkingForUpdates", "Checking for updates", "正在检查更新"),
    ("ui.oxideAsksTheInstalledCLIWhichReleaseOfThisExtensionIsNewestTheSameCheckThePanelR", "Oxide asks the installed CLI which release of this extension is newest — the same check the panel runs as `oxide update --check --component extension`.", "Oxide 通过已安装的 CLI 查询扩展最新版本，与面板运行 oxide update --check --component extension 相同。"),
    ("ui.askingGitHubForTheNewestExtensionRelease", "Asking GitHub for the newest extension release…", "正在向 GitHub 查询扩展最新版本…"),
    ("ui.theReleaseSVsixIsDownloadedCheckedAgainstTheChecksumItsReleasePublishedAndHanded", "The release's .vsix is downloaded, checked against the checksum its release published, and handed to VS Code to install.", "下载版本的 .vsix，验证发布的校验和后，交给 VS Code 安装。"),
    ("ui.thisTakesAsLongAsTheDownloadTheDialogReportsWhatHappenedWhenItIsDone", "This takes as long as the download; the dialog reports what happened when it is done.", "此操作需要等待下载完成；完成后将在此显示结果。"),
    ("ui.vSCodeHoldsTheNewVersionTheExtensionRunningInThisWindowIsTheOneThatWasThereWhenI", "VS Code holds the new version; the extension running in this window is the one that was there when it was installed.", "VS Code 已保存新版本；此窗口仍在运行安装前的版本。"),
    ("ui.restartThisWindowToRunIt", "Restart this window to run it", "重启此窗口以运行新版本"),
    ("ui.restartWindow", "Restart Window", "重启窗口"),
    ("ui.runTheVersionThatWasJustInstalled", "Run the version that was just installed", "运行刚安装的版本"),
    ("ui.restartLaterFromTheCommandPalette", "Restart later from the command palette", "稍后从命令面板重启"),
    ("ui.theCheckAsksGitHubWhichReleaseOfThisExtensionIsNewestSoAMachineWithNoNetworkOrAR", "The check asks GitHub which release of this extension is newest, so a machine with no network — or a request GitHub refuses — reports it here.", "检查更新需要向 GitHub 查询扩展最新版本。网络不可用或请求被拒绝时将在此显示。"),
    ("ui.theInstallDownloadsTheReleaseSVsixChecksItAgainstTheChecksumItsReleasePublishedA", "The install downloads the release's .vsix, checks it against the checksum its release published and hands it to VS Code.", "安装会下载版本的 .vsix，验证发布的校验和后交给 VS Code。"),
    ("ui.leaveThisExtensionAsItIs", "Leave this extension as it is", "保留当前扩展版本"),
    ("ui.theOxideCLIIsOlderThanThisPanel", "The oxide CLI is older than this panel", "oxide CLI 版本早于此面板"),
    ("ui.checkForUpdatesReadsTheExtensionSOwnReleasesThroughTheInstalledOxideAndThisCLIPr", "Check for Updates reads the extension's own releases through the installed oxide, and this CLI predates the report it reads — so updating the oxide command line is what makes both the check and the extension's next update reachable from here. It works on any version.", "检查更新通过已安装的 oxide 查询扩展版本。此 CLI 尚不支持所需的报告，请先更新 oxide 命令行工具，即可在此检查和安装扩展更新。此操作适用于所有版本。"),
    ("ui.installTheNewestCLI", "Install the newest CLI", "安装最新 CLI"),
    ("ui.runsOxideUpdate", "Runs oxide update", "运行 oxide update"),
    ("ui.leaveThisInstallationAsItIs", "Leave this installation as it is", "保留当前安装"),
    ("ui.openTheReleasePageInYourBrowser", "Open the release page in your browser", "在浏览器中打开发布页面"),
    ("ui.keepTheVersionInstalledNow", "Keep the version installed now", "保留当前安装的版本"),
    ("ui.nothingToInstall", "Nothing to install", "无需安装"),
    ("ui.installingDownloadsThisReleaseOfTheExtensionTheVsixPublishedOnItsOwnReleaseTrain", "Installing downloads this release of the extension — the .vsix published on its own release train — and hands it to VS Code, which runs it from the next window on.", "安装会下载此扩展版本的 .vsix 并交给 VS Code；重新打开窗口后生效。"),
    ("ui.anOlderRelease", "an older release", "较早的版本"),
    ("ui.chooseEnterContinueEscCancel", "↑/↓ choose · Enter continue · Esc cancel", "↑/↓ 选择 · Enter 继续 · Esc 取消"),
    ("ui.waitingForTheApprovalEscCancel", "waiting for the approval · Esc cancel", "等待批准 · Esc 取消"),
    ("ui.enterUseStoredKeyTypeToReplaceItBackspaceBackEscCancel", "Enter use stored key · type to replace it · Backspace back · Esc cancel", "Enter 使用已保存的密钥 · 输入以替换 · Backspace 返回 · Esc 取消"),
    ("ui.enterConnectBackspaceBackEscCancel", "Enter connect · Backspace back · Esc cancel", "Enter 连接 · Backspace 返回 · Esc 取消"),
    ("ui.fieldEnterNextSaveBackspaceBackEscCancel", "↑/↓ field · Enter next/save · Backspace back · Esc cancel", "↑/↓ 切换字段 · Enter 下一项/保存 · Backspace 返回 · Esc 取消"),
    ("ui.thisProviderIsConnectedEnterReusesTheStoredKey", "This provider is connected — Enter reuses the stored key.", "此服务商已连接 — Enter 使用已保存的密钥。"),
    ("ui.theCodeIsAlsoInYourClipboard", "  the code is also in your clipboard", "  验证码也已复制到剪贴板"),
    ("ui.askingGitHubForACode", "  asking GitHub for a code...", "  正在向 GitHub 请求验证码…"),
    ("tui.openLabel", "  open ", "  打开 "),
    ("ui.code", "  code ", "  验证码 "),
    ("tui.connectedMark", " · connected", " · 已连接"),
    ("ui.portkeyUsage", "portkey usage", "Portkey 用量"),
    ("ui.enterApplyEscCancelEmptyUsesTheProviderKey", "Enter apply · Esc cancel · empty uses the provider key", "Enter 应用 · Esc 取消 · 留空使用服务商密钥"),
    ("ui.enterApplyEscCancel", "Enter apply · Esc cancel", "Enter 应用 · Esc 取消"),
    ("ui.moveEnterToggleEscSaveClose", "↑/↓ move · Enter toggle · Esc save & close", "↑/↓ 移动 · Enter 切换 · Esc 保存并关闭"),
    ("ui.moveEnterEditEscSaveClose", "↑/↓ move · Enter edit · Esc save & close", "↑/↓ 移动 · Enter 编辑 · Esc 保存并关闭"),
    ("ui.modelsFromEveryLoggedInProviderUseConnectToAddMore", "Models from every logged-in provider. Use /connect to add more.", "所有已登录服务商的模型。使用 /connect 添加更多。"),
    ("ui.noMatchingModels", "  No matching models", "  没有匹配的模型"),
    ("tui.defaultMark", " · default", " · 默认"),
    ("ui.refreshingModelCatalogs", "  Refreshing model catalogs…", "  正在刷新模型列表…"),
    ("ui.modelCatalogsRefreshed", "  Model catalogs refreshed.", "  模型列表已刷新。"),
    ("ui.enterToSelectCtrlSToSetAsDefaultEscapeCtrlCToCancel", "  Enter to select · Ctrl+S to set as default · Escape/Ctrl+C to cancel", "  Enter 选择 · Ctrl+S 设为默认 · Escape/Ctrl+C 取消"),
    ("ui.renameSession", " rename session ", " 重命名会话 "),
    ("ui.deleteSession", " delete session? ", " 删除会话？ "),
    ("ui.sessions", " sessions ", " 会话 "),
    ("ui.enterSaveEscCancel", "Enter save · Esc cancel", "Enter 保存 · Esc 取消"),
    ("ui.theSessionFileWillBeMovedToTrashWhenAvailable", "The session file will be moved to trash when available.", "可用时会将会话文件移入废纸篓。"),
    ("ui.enterYConfirmEscNCancel", "Enter/y confirm · Esc/n cancel", "Enter/y 确认 · Esc/n 取消"),
    ("ui.noSessionsForThisProjectYet", "no sessions for this project yet", "此项目暂无会话"),
    ("ui.noMatchingSessions", "no matching sessions", "没有匹配的会话"),
    ("ui.marketplacesPublishPluginsFromAGitRepositoryOrLocalPath", "Marketplaces publish plugins from a git repository or local path.", "市场通过 Git 仓库或本地路径发布插件。"),
    ("ui.filterMarketplacesOrPlugins", "filter marketplaces or plugins…", "筛选市场或插件…"),
    ("ui.noMarketplacesYetPressCtrlAToAddOne", "No marketplaces yet.\n\nPress Ctrl+A to add one.", "暂无市场。\n\n按 Ctrl+A 添加。"),
    ("ui.noMatchingMarketplaces", "No matching marketplaces.", "没有匹配的市场。"),
    ("ui.selectAMarketplaceToSeeItsPlugins", "Select a marketplace to see its plugins.", "选择市场以查看其插件。"),
    ("ui.thisMarketplaceListsNoPlugins", "  this marketplace lists no plugins", "  此市场暂无插件"),
    ("ui.noMatchingPlugins", "  no matching plugins", "  没有匹配的插件"),
    ("tui.enabledMark", "  enabled", "  已启用"),
    ("tui.disabledMark", "  disabled", "  已禁用"),
    ("ui.working", "  working…", "  正在处理…"),
    ("ui.moveTabPaneEnterInstallDisableCtrlUUpdateCtrlAAddCtrlXRemoveCtrlRReloadEscClose", "  ↑/↓ move · Tab pane · Enter install/disable · Ctrl+U update · Ctrl+A add · Ctrl+X remove · Ctrl+R reload · Esc close", "  ↑/↓ 移动 · Tab 切换面板 · Enter 安装/禁用 · Ctrl+U 更新 · Ctrl+A 添加 · Ctrl+X 移除 · Ctrl+R 重新加载 · Esc 关闭"),
    ("ui.gitURLOwnerRepoShorthandOrLocalPathWithA", "Git URL, owner/repo shorthand, or local path with a", "Git URL、owner/repo 简写，或包含以下文件的本地路径："),
    ("ui.enterAddEscCancel", "Enter add · Esc cancel", "Enter 添加 · Esc 取消"),
    ("ui.commandsClickOrTabToComplete", "commands · click or Tab to complete", "命令 · 点击或按 Tab 补全"),
    ("ui.filesClickOrTabToComplete", "files · click or Tab to complete", "文件 · 点击或按 Tab 补全"),
    ("ui.sub", " (sub)", "（订阅）"),
    ("usage.auto", " (auto)", "（自动）"),
    ("ui.thinkingOff", " • thinking off", " • 思考已关闭"),
    ("ui.noModel", "no-model", "未选择模型"),
    ("ui.editProject", "Edit project", "编辑项目"),
    ("ui.pinProject", "Pin project", "置顶项目"),
    ("ui.unpinProject", "Unpin project", "取消置顶项目"),
    ("ui.pinThread", "Pin thread", "置顶对话"),
    ("ui.unpinThread", "Unpin thread", "取消置顶对话"),
    ("ui.archiveThread", "Archive thread", "归档对话"),
    ("ui.default", "Default", "默认"),
    ("ui.auto", "Auto", "自动"),
    ("ui.letTheProviderDecide", "Let the provider decide", "由服务商决定"),
    ("ui.turnThinkingOffWhereTheModelAllowsIt", "Turn thinking off where the model allows it", "在模型允许时关闭思考"),
    ("ui.theLeastReasoning", "The least reasoning", "最低思考强度"),
    ("ui.aLittleReasoning", "A little reasoning", "较低思考强度"),
    ("ui.balancedReasoning", "Balanced reasoning", "均衡思考强度"),
    ("ui.aLotOfReasoning", "A lot of reasoning", "较高思考强度"),
    ("ui.extraHighReasoning", "Extra-high reasoning", "超高思考强度"),
    ("ui.theMostReasoning", "The most reasoning", "最高思考强度"),
    ("ui.dismissNotification", "Dismiss notification", "关闭通知"),
    ("ui.alreadyAttached", "Already attached", "已添加此附件"),
    ("ui.atMost8AttachmentsPerMessage", "At most 8 attachments per message", "每条消息最多添加 8 个附件"),
    ("ui.removeAttachment", "Remove attachment", "移除附件"),
    ("ui.attachmentPreview", "Attachment preview", "附件预览"),
    ("ui.openPreview", "Open preview", "打开预览"),
    ("ui.openImagePreview", "Open image preview", "打开图片预览"),
    ("ui.projectAccessTrustedClickToReview", "Project access: trusted · click to review", "项目访问权限：已信任 · 点击查看"),
    ("ui.projectAccessProjectResourcesAreNotTrusted", "Project access: project resources are not trusted", "项目访问权限：尚未信任项目资源"),
    ("ui.projectTrusted", "Project trusted", "已信任项目"),
    ("ui.projectResourcesLeftUntrusted", "Project resources left untrusted", "尚未信任项目资源"),
    ("ui.agentsDefault", "Agents: Default", "代理：默认"),
    ("ui.defaultAgent", "Default agent", "默认代理"),
    ("ui.noAPIKey", "no API key", "未设置 API 密钥"),
    ("ui.projectResourcesOff", "project resources off", "项目资源已禁用"),
    ("ui.switchModel", "Switch model", "切换模型"),
    ("ui.chooseTheReasoningLevelShiftTabCycles", "Choose the reasoning level (Shift+Tab cycles)", "选择思考强度（Shift+Tab 循环切换）"),
    ("ui.noRequestYet", "No request yet", "尚无请求"),
    ("ui.localCheckout", "Local checkout", "本地检出"),
    ("ui.handOffChatToLocal", "Hand off chat to Local", "将对话移交到本地"),
    ("ui.handOffChat", "Hand off chat", "移交对话"),
    ("ui.handOff", "Hand off", "移交"),
    ("ui.movingChatAndChanges", "Moving chat and changes…", "正在移动对话及更改…"),
    ("ui.runAThreadInThisProjectSOwnFolder", "Run a thread in this project's own folder", "在此项目的原始文件夹中运行对话"),
    ("ui.threadsRunInThisProjectSOwnFolder", "Threads run in this project's own folder.", "对话在此项目的原始文件夹中运行。"),
    ("ui.removeWorktree", "Remove worktree", "移除工作树"),
    ("ui.theResponseFinishedStartingThisAsTheNextTurn", "The response finished; starting this as the next turn…", "回复已结束；正在将此消息作为下一轮发送…"),
    ("ui.sendOrClearTheCurrentDraftBeforeEditingAQueuedMessage", "Send or clear the current draft before editing a queued message.", "请先发送或清空当前草稿，再编辑排队中的消息。"),
    ("ui.moreQueuedMessageActions", "More queued-message actions", "更多排队消息操作"),
    ("ui.undo", "Undo", "撤销"),
    ("ui.undone", "Undone", "已撤销"),
    ("ui.undoChanges", "Undo changes", "撤销更改"),
    ("ui.putTheTurnSFilesBack", "Put the turn's files back", "已将本轮更改的文件还原"),
    ("ui.changes", "Changes", "更改"),
    ("ui.noChanges", "No changes.", "暂无更改。"),
    ("ui.readingTheChange", "Reading the change⋯", "正在读取更改…"),
    ("ui.binaryFileNoTextDiff", "Binary file — no text diff.", "二进制文件 — 无文本差异。"),
    ("ui.fileTooLargeToDiffLineByLine", "File too large to diff line by line.", "文件过大，无法逐行比较。"),
    ("ui.noTextualChanges", "No textual changes.", "没有文本更改。"),
    ("ui.loadingSources", "Loading sources…", "正在加载来源…"),
    ("ui.selectAProjectToSeeItsSources", "Select a project to see its sources.", "选择项目以查看其来源。"),
    ("ui.webSearch", "Web search", "网页搜索"),
    ("ui.noSourcesYet", "No sources yet.", "暂无来源。"),
    ("ui.readingChanges", "Reading changes…", "正在读取更改…"),
    ("ui.noChangesFromATurnInThisChatYet", "No changes from a turn in this chat yet.", "此对话尚无回合更改文件。"),
    ("ui.typeYourOwnAnswer", "Type your own answer", "输入自己的回答"),
    ("ui.typeYourAnswer", "Type your answer…", "输入回答…"),
    ("ui.orTypeYourOwnAnswer", "Or type your own answer…", "或输入自己的回答…"),
    ("ui.selectAllThatApply", "Select all that apply", "选择所有适用项"),
    ("ui.selectOneAnswer", "Select one answer", "选择一个答案"),
    ("ui.leaveEmptyToReuseTheStoredKey", "Leave empty to reuse the stored key", "留空以使用已保存的密钥"),
    ("ui.chooseAProviderFirst", "Choose a provider first.", "请先选择服务商。"),
    ("ui.inUse", "in use", "使用中"),
    ("ui.active", " (active)", "（当前）"),
    ("ui.couldNotCheckForUpdates", "Could not check for updates", "无法检查更新"),
    ("ui.theRelease", "The release", "此版本"),
    ("ui.installingUpdate", "Installing update", "正在安装更新"),
    ("ui.downloadingUpdate", "Downloading update", "正在下载更新"),
    ("ui.theInstallDidNotFinish", "The install did not finish.", "安装未完成。"),
    ("ui.couldNotInstallTheUpdate", "Could not install the update", "无法安装更新"),
    ("ui.finishTheInstallerThenOpenOxideAgainToRunTheNewVersion", "Finish the installer, then open Oxide again to run the new version.", "完成安装后，重新打开 Oxide 以运行新版本。"),
    ("ui.updateDownloadedYouNeedToRestartOxideToInstallTheUpdate", "Update downloaded. You need to restart Oxide to install the update.", "更新已下载。请重启 Oxide 以安装更新。"),
    ("ui.noProviderIsConnected", "No provider is connected.", "尚未连接服务商。"),
    ("ui.signOut", "Sign out", "退出登录"),
    ("ui.noUsageReportedYet", "No usage reported yet.", "尚未报告用量。"),
    ("ui.chatArchived", "Chat archived", "对话已归档"),
    ("ui.aTurnIsRunningStopItBeforeArchivingThisThread", "A turn is running; stop it before archiving this thread.", "请先停止正在运行的回合，再归档此对话。"),
    ("ui.aTurnIsRunningStopItBeforeRemovingThisProject", "A turn is running; stop it before removing this project.", "请先停止正在运行的回合，再移除此项目。"),
    ("ui.thatFolderIsAlreadyInTheList", "That folder is already in the list", "此文件夹已在列表中"),
    ("ui.atLeastOneSourceFolderIsRequired", "At least one source folder is required", "至少需要一个来源文件夹"),
    ("ui.projectNameIsRequired", "Project name is required", "请填写项目名称"),
    ("ui.aTurnIsRunningInThisThread", "A turn is running in this thread", "此对话中有回合正在运行"),
    ("ui.answered", "Answered", "已回答"),
    ("ui.waitingForYourAnswer", "Waiting for your answer…", "正在等待你的回答…"),
    ("ui.answerNothingAndLetTheAgentContinueWithItsOwnDefault", "Answer nothing and let the agent continue with its own default", "不作回答，让代理按默认选择继续"),
    ("ui.goBackToThePreviousQuestion", "Go back to the previous question", "返回上一个问题"),
    ("ui.sendTheseAnswersToTheAgent", "Send these answers to the agent", "将这些回答发送给代理"),
    ("ui.goToTheQuestionAfterThisOne", "Go to the question after this one", "转到下一个问题"),
    ("ui.review", "Review", "审查"),
    ("ui.openEveryFileInVSCodeSDiffEditor", "Open every file in VS Code's diff editor", "在 VS Code 差异编辑器中打开所有文件"),
    ("ui.walkThisTurnSFilesEachInVSCodeSDiffEditor", "Walk this turn's files, each in VS Code's diff editor", "在 VS Code 差异编辑器中逐个审查本轮文件"),
    ("ui.putThisTurnSFilesBackToHowTheRunFoundThem", "Put this turn's files back to how the run found them", "将本轮文件还原至运行前的状态"),
    ("ui.oxideIsWorking", "Oxide is working", "Oxide 正在处理"),
    ("ui.readyForTheNextMessage", "Ready for the next message", "准备好接收下一条消息"),
    ("ui.noUsageReported", "No usage reported", "尚未报告用量"),
    ("ui.clear", "Clear", "清空"),
    ("ui.removeEverythingPending", "Remove everything pending", "移除所有待发送内容"),
    ("ui.openTheFullSizeImage", "Open the full-size image", "打开原始大小的图片"),
    ("ui.resumedThreadHowThisCallEndedWasNotRecorded", "Resumed thread: how this call ended was not recorded", "已恢复对话：此调用的结束状态未记录"),
    ("ui.providers", "Providers", "服务商"),
    ("ui.noProvidersAreConfiguredForThisInstallation", "No providers are configured for this installation.", "此安装尚未配置服务商。"),
    ("ui.models", "Models", "模型"),
    ("ui.current", "Current", "当前"),
    ("ui.connected", "Connected", "已连接"),
    ("ui.stored", "Stored", "已保存"),
    ("ui.notConnected", "Not connected", "未连接"),
    ("ui.needsAuth", "Needs Auth", "需要授权"),
    ("ui.needsTrust", "Needs Trust", "需要信任"),
    ("ui.disabled", "Disabled", "已禁用"),
    ("ui.error", "Error", "错误"),
    ("ui.enabled", "Enabled", "已启用"),
    ("ui.recheck", "Recheck", "重新检查"),
    ("ui.loading", "Loading…", "正在加载…"),
    ("ui.noSessionsYet", "No sessions yet.", "暂无会话。"),
    ("ui.doNotTrustProject", "Do not trust project", "不信任项目"),
    ("ui.chooseAProviderOrTypeACustomProviderName", "Choose a provider, or type a custom provider name", "选择服务商，或输入自定义服务商名称"),
    ("ui.approveTheLoginInYourBrowser", "Approve the login in your browser", "请在浏览器中批准登录"),
    ("ui.pasteTheAPIKeyForThisProvider", "Paste the API key for this provider", "粘贴此服务商的 API 密钥"),
    ("ui.optionalSettingsBlankKeepsTheProviderDefault", "Optional settings — blank keeps the provider default", "可选设置 — 留空以使用服务商默认值"),
    ("ui.trustLoadsProjectResourcesThatMayChangeTheAgentOrExecute", "Trust loads project resources that may change the agent or execute", "信任后将加载项目资源，这些资源可能修改代理或执行"),
    ("ui.codeTheDecisionIsSharedAcrossAllFrontEnds", "code. The decision is shared across all front-ends.", "代码。此决定由所有前端共享。"),
    ("ui.chooseEnterConfirmEscDeclineForThisSession", "←/→ choose · Enter confirm · Esc decline for this session", "←/→ 选择 · Enter 确认 · Esc 在本次会话中拒绝"),
    ("ui.projectTrust", "project trust", "项目信任"),
    ("ui.renameTheSelectedSession", "Rename the selected session", "重命名所选会话"),
    ("ui.addAMarketplace", "Add a marketplace", "添加市场"),
    ("ui.plugins", "Plugins", "插件"),
    ("ui.portkeySpendBar", "Portkey spend bar", "Portkey 费用栏"),
    ("ui.read", "Read", "读取"),
    ("ui.run", "Run", "运行"),
    ("ui.editFailed", "Edit failed", "编辑失败"),
    ("ui.edited", "Edited", "已编辑"),
    ("ui.readFailed", "Read failed", "读取失败"),
    ("ui.runFailed", "Run failed", "运行失败"),
    ("ui.ran", "Ran", "已运行"),
    ("ui.branchSummary", "Branch summary", "分支摘要"),
    ("ui.updateAvailable", "Update Available", "有可用更新"),
    ("ui.aboutOxide", "About Oxide", "关于 Oxide"),
    ("ui.checkForUpdates", "Check for Updates…", "检查更新…"),
    ("ui.hideOxide", "Hide Oxide", "隐藏 Oxide"),
    ("ui.hideOthers", "Hide Others", "隐藏其他应用"),
    ("ui.showAll", "Show All", "显示全部"),
    ("ui.quitOxide", "Quit Oxide", "退出 Oxide"),
    ("ui.file", "File", "文件"),
    ("ui.newTab", "New Tab", "新建标签页"),
    ("ui.closeTab", "Close Tab", "关闭标签页"),
    ("ui.redo", "Redo", "重做"),
    ("ui.cut", "Cut", "剪切"),
    ("ui.copy", "Copy", "复制"),
    ("ui.paste", "Paste", "粘贴"),
    ("ui.selectAll", "Select All", "全选"),
    ("ui.window", "Window", "窗口"),
    ("ui.minimize", "Minimize", "最小化"),
    ("ui.zoom", "Zoom", "缩放"),
    ("ui.closeWindow", "Close Window", "关闭窗口"),
    ("ui.chooseAProjectFolder", "Choose a project folder", "选择项目文件夹"),
    ("ui.justNow", "just now", "刚刚"),
    ("ui.mAgo", "{value0}m ago", "{value0} 分钟前"),
    ("ui.hAgo", "{value0}h ago", "{value0} 小时前"),
    ("ui.dAgo", "{value0}d ago", "{value0} 天前"),
    ("ui.couldNotUndo", "Could not undo: {value0}", "无法撤销：{value0}"),
    ("ui.failedToLoadProjects", "Failed to load projects: {value0}", "无法加载项目：{value0}"),
    ("ui.trustUpdateFailed", "Trust update failed: {value0}", "无法更新信任设置：{value0}"),
    ("ui.couldNotCreateTheWorktree", "Could not create the worktree: {value0}", "无法创建工作树：{value0}"),
    ("ui.couldNotRemoveTheWorktree", "Could not remove the worktree: {value0}", "无法移除工作树：{value0}"),
    ("ui.createdBranch", "Created branch {value0}.", "已创建分支 {value0}。"),
    ("ui.couldNotCreateBranch", "Could not create branch: {value0}", "无法创建分支：{value0}"),
    ("ui.couldNotHandOffChat", "Could not hand off chat: {value0}", "无法移交对话：{value0}"),
    ("ui.failedToLoadThreads", "Failed to load threads: {value0}", "无法加载对话：{value0}"),
    ("ui.failedToOpenThread", "Failed to open thread: {value0}", "无法打开对话：{value0}"),
    ("ui.couldNotRead", "Could not read {value0}: {value1}", "无法读取 {value0}：{value1}"),
    ("common.errorDetail", "Error: {value0}", "错误：{value0}"),
    ("ui.viewChangesFor", "View changes for {value0}", "查看 {value0} 的更改"),
    ("ui.couldNotLoadSources", "Could not load sources: {value0}", "无法加载来源：{value0}"),
    ("ui.isIncludedInThisChat", "{value0} is included in this chat.", "此对话已包含 {value0}。"),
    ("ui.retrying", "Retrying ({value0}/{value1})…", "正在重试（{value0}/{value1}）…"),
    ("ui.compactedEarlierMessages", "Compacted {value0} earlier messages", "已压缩前面的 {value0} 条消息"),
    ("ui.connecting", "Connecting {value0}…", "正在连接 {value0}…"),
    ("provider.connected", "Connected {value0}", "已连接 {value0}"),
    ("ui.loginFailed", "Login failed: {value0}", "登录失败：{value0}"),
    ("ui.themesFailed", "Themes failed: {value0}", "无法加载主题：{value0}"),
    ("ui.themeFailed", "Theme failed: {value0}", "无法切换主题：{value0}"),
    ("ui.couldNotStopTheUpdate", "Could not stop the update: {value0}", "无法停止更新：{value0}"),
    ("update.installError", "Could not install the update: {value0}", "无法安装更新：{value0}"),
    ("ui.couldNotRestartOxide", "Could not restart Oxide: {value0}", "无法重启 Oxide：{value0}"),
    ("ui.couldNotOpenLink", "Could not open link: {value0}", "无法打开链接：{value0}"),
    ("ui.couldNotSaveReasoning", "Could not save reasoning: {value0}", "无法保存思考强度：{value0}"),
    ("ui.couldNotOpenFolderChooser", "Could not open folder chooser: {value0}", "无法打开文件夹选择器：{value0}"),
    ("ui.couldNotCreateProject", "Could not create project: {value0}", "无法创建项目：{value0}"),
    ("ui.couldNotReorderProjects", "Could not reorder projects: {value0}", "无法调整项目顺序：{value0}"),
    ("ui.couldNotArchiveThread", "Could not archive thread: {value0}", "无法归档对话：{value0}"),
    ("ui.couldNotRemoveProject", "Could not remove project: {value0}", "无法移除项目：{value0}"),
    ("ui.projectOptionsFor", "Project options for {value0}", "{value0} 的项目选项"),
    ("ui.newTaskIn", "New task in {value0}", "在 {value0} 中新建任务"),
    ("ui.runsInTheWorktree", "Runs in the worktree {value0}", "在工作树 {value0} 中运行"),
    ("ui.showMoreProjects", "Show {value0} more projects", "再显示 {value0} 个项目"),
    ("ui.showMoreChatsIn", "Show {value0} more chats in {value1}", "再显示 {value1} 中的 {value0} 个对话"),
    ("ui.projectActions", "{value0} project actions", "{value0} 的项目操作"),
    ("ui.close", "Close {value0}", "关闭 {value0}"),
    ("ui.agents", "Agents: {value0}", "代理：{value0}"),
    ("ui.jumpToMessage", "Jump to message {value0}: {value1}", "跳到第 {value0} 条消息：{value1}"),
    ("ui.open", "Open “{value0}”", "打开“{value0}”"),
    ("common.openNamed", "Open {value0}", "打开 {value0}"),
    ("ui.aTurnIsRunningIn", "A turn is running in “{value0}”", "“{value0}”中有回合正在运行"),
    ("ui.aTurnFinishedIn", "A turn finished in “{value0}”", "“{value0}”中的回合已完成"),
    ("ui.openIt", "{value0}. Open it.", "{value0}。点击打开。"),
    ("ui.worktreeOn", "Worktree {value0} on {value1}\n{value2}", "工作树 {value0}，分支 {value1}\n{value2}"),
    ("ui.removeTheWorktree", "Remove the worktree {value0}", "移除工作树 {value0}"),
    ("ui.runAThreadIn", "Run a thread in {value0}\n{value1} · {value2}", "在 {value0} 中运行对话\n{value1} · {value2}"),
    ("ui.threadsRunInTheWorktreeOn", "Threads run in the worktree {value0} on {value1}.", "对话在工作树 {value0} 的 {value1} 分支上运行。"),
    ("ui.createdTheWorktreeAt", "Created the worktree {value0} at {value1}.", "已在 {value1} 创建工作树 {value0}。"),
    ("ui.removedItsBranchIsStillInTheRepository", "Removed {value0}. Its branch {value1} is still in the repository.", "已移除 {value0}。其分支 {value1} 仍保留在仓库中。"),
    ("ui.movedQueuedMessageToPositionOf", "Moved queued message to position {value0} of {value1}.", "已将排队消息移至第 {value0} 位，共 {value1} 条。"),
    ("ui.ofQuestions", "{value0} of {value1} questions", "第 {value0} 个问题，共 {value1} 个"),
    ("ui.changedFiles", "{value0} changed files", "{value0} 个文件已更改"),
    ("ui.currentBranch", "Current branch: {value0}", "当前分支：{value0}"),
    ("ui.workedFor", "Worked for {value0}", "已处理 {value0}"),
    ("ui.workingFor", "Working for {value0}", "正在处理，已用时 {value0}"),
    ("ui.ofTheContextWindowUsed", "{value0}% of the context window used", "已使用上下文窗口的 {value0}%"),
    ("ui.attachmentS", "{value0} attachment(s)", "{value0} 个附件"),
    ("attachment.openFullImage", "{value0} · open the full-size image", "{value0} · 打开原始大小的图片"),
    ("ui.noProviderMatches", "No provider matches “{value0}”.", "没有匹配“{value0}”的服务商。"),
    ("ui.signInToAProviderTheCredentialIsStoredWhereTheTerminalSConnectStoresItAndTheNext", "Sign in to a provider. The credential is stored where the terminal's /connect stores it, and the next turn uses it.", "登录服务商。凭据保存在终端 /connect 使用的位置，并在下一轮生效。"),
    // Common actions, drawn as a dialog's buttons or a row's control.
    ("common.close", "Close", "关闭"),
    ("common.cancel", "Cancel", "取消"),
    ("common.save", "Save", "保存"),
    ("common.create", "Create", "创建"),
    ("common.delete", "Delete", "删除"),
    ("common.remove", "Remove", "移除"),
    ("common.dismiss", "Dismiss", "忽略"),
    ("common.later", "Later", "稍后"),
    ("common.refresh", "Refresh", "刷新"),
    ("common.search", "Search", "搜索"),
    ("common.open", "Open", "打开"),
    ("common.rename", "Rename", "重命名"),
    ("common.none", "None", "无"),
    ("common.back", "Back", "上一步"),
    ("common.next", "Next", "下一步"),
    ("common.submit", "Submit", "提交"),
    ("common.retry", "Retry", "重试"),
    ("common.done", "Done", "完成"),
    // The language itself.
    ("language.name", "Language", "语言"),
    ("language.changed", "Language set to {language}.", "界面语言已切换为{language}。"),
    (
        "language.unknown",
        "Unknown language `{tag}`. Available: {options}",
        "未知语言 `{tag}`。可选：{options}",
    ),
    ("language.current", "Language: {language}", "语言：{language}"),
    // The sidebar's own chrome.
    ("sidebar.newChat", "New Chat", "新建对话"),
    (
        "sidebar.newChatHint",
        "Starts a thread in the current project",
        "在当前项目中开始一个新对话",
    ),
    ("sidebar.projects", "Projects", "项目"),
    ("sidebar.newProject", "New project", "新建项目"),
    ("sidebar.newProjectHint", "Pick the folders to include", "选择要包含的文件夹"),
    ("sidebar.theme", "Theme", "主题"),
    ("sidebar.checkUpdates", "Check for updates", "检查更新"),
    ("sidebar.language", "Language", "语言"),
    ("sidebar.recents", "Recents", "最近"),
    ("sidebar.recentThreads", "Recent threads", "最近的对话"),
    ("sidebar.showMore", "Show more", "显示更多"),
    ("sidebar.showLess", "Show less", "收起"),
    ("sidebar.noChats", "No chats", "暂无对话"),
    ("sidebar.archive", "Archive", "归档"),
    ("sidebar.pin", "Pin", "固定"),
    ("sidebar.unpin", "Unpin", "取消固定"),
    ("sidebar.renameThread", "Rename thread", "重命名对话"),
    ("sidebar.deleteThread", "Delete thread", "删除对话"),
    ("sidebar.addProject", "Add project", "添加项目"),
    // The header over a conversation.
    ("topbar.viewChanges", "View changes", "查看更改"),
    ("topbar.projectDetails", "Project details", "项目详情"),
    ("topbar.newTab", "New tab", "新建标签页"),
    ("topbar.jumpLatest", "Jump to latest message", "跳到最新消息"),
    ("topbar.worktree", "Worktree", "工作树"),
    ("topbar.runOpen", "Open", "打开"),
    (
        "topbar.runElsewhere",
        "A turn is running in “{title}”",
        "“{title}”中有回合正在运行",
    ),
    // The composer: the box, its chips and what a message's controls say.
    ("composer.chooseProject", "Choose a project", "选择项目"),
    (
        "composer.chooseProjectHint",
        "Pick the folder this window runs in",
        "选择此窗口运行所在的文件夹",
    ),
    ("composer.placeholder", "Ask Oxide…", "询问 Oxide…"),
    ("composer.message", "Message", "消息"),
    ("composer.send", "Send", "发送"),
    ("composer.sendHint", "Send (Enter)", "发送（Enter）"),
    ("composer.stop", "Stop", "停止"),
    ("composer.stopHint", "Stop the running turn", "停止正在运行的回合"),
    ("composer.attach", "Attach images or PDFs", "附加图片或 PDF"),
    (
        "composer.attachHint",
        "Attach images or PDFs (paste with the platform paste shortcut)",
        "附加图片或 PDF（使用系统粘贴快捷键）",
    ),
    ("composer.connect", "Connect a provider", "连接服务商"),
    ("composer.trust", "Project access", "项目访问权限"),
    ("composer.agents", "Agents", "代理"),
    ("composer.permissions", "Tool approvals", "工具审批"),
    ("composer.help", "Keyboard shortcuts", "键盘快捷键"),
    ("composer.model", "Model", "模型"),
    ("composer.thinking", "Thinking level", "思考强度"),
    ("composer.queued", "Queued messages", "已排队的消息"),
    ("composer.queuedActions", "Queued message actions", "排队消息操作"),
    ("composer.queue", "Queue", "排队"),
    ("composer.steer", "Steer", "引导"),
    ("composer.steerHint", "Steer the active response (Enter)", "引导当前回复（Enter）"),
    ("composer.queueHint", "Queue as the next turn (Enter)", "排队为下一轮（Enter）"),
    ("composer.useSteer", "Use Steer for new messages", "新消息使用引导"),
    ("composer.useQueue", "Use Queue for new messages", "新消息使用排队"),
    ("composer.attachment", "Attachment", "附件"),
    ("composer.edit", "Edit", "编辑"),
    ("composer.moveUp", "Move up", "上移"),
    ("composer.moveDown", "Move down", "下移"),
    ("composer.deliver", "Delivery", "送达方式"),
    ("composer.worktree", "Worktree", "工作树"),
    ("composer.ready", "Ready", "就绪"),
    ("composer.working", "Working…", "正在处理…"),
    ("composer.stopping", "Stopping…", "正在停止…"),
    ("composer.steering", "Steering the active response…", "正在引导当前回复…"),
    ("composer.selectProject", "Select a project first.", "请先选择项目。"),
    ("composer.queuedMessage", "Queued: {text}", "已排队：{text}"),
    (
        "composer.turnElsewhere",
        "A turn is running in “{title}”; open it to queue or steer, or stop it.",
        "“{title}”中有回合正在运行；打开它即可排队或引导，或将其停止。",
    ),
    // The page a window opens on when no thread is in it.
    ("home.title", "What should we build?", "我们要构建什么？"),
    (
        "home.pickFolder",
        "Pick a folder with the project chip below, or open a recent thread.",
        "用下方的项目按钮选择文件夹，或打开最近的对话。",
    ),
    (
        "home.addFolder",
        "Add a folder to run in — the project chip below, or the + beside Projects.",
        "添加一个运行目录 — 使用下方的项目按钮，或“项目”旁的 +。",
    ),
    (
        "home.openFolder",
        "Open a folder to run Oxide: sessions and context are per project.",
        "打开文件夹以运行 Oxide：会话和上下文按项目区分。",
    ),
    (
        "home.suggestion1",
        "Explain this codebase and its architecture.",
        "解释这个代码库及其架构。",
    ),
    (
        "home.suggestion2",
        "Find and fix the highest-priority bug in this repository.",
        "找到并修复这个仓库中优先级最高的缺陷。",
    ),
    (
        "home.suggestion3",
        "Add tests for the most important untested code path.",
        "为最重要的未测试代码路径添加测试。",
    ),
    (
        "home.suggestion4",
        "Review the working tree changes and summarize the risks.",
        "审查工作区中的更改并总结风险。",
    ),
    // The listings and dialogs the composer opens.
    (
        "dialog.language.subtitle",
        "The language this window, the terminal and the editor panel share. Switching it rewrites the same settings.json key every front-end reads.",
        "此窗口、终端和编辑器面板共用的语言。切换后会改写所有前端读取的同一个 settings.json 键。",
    ),
    (
        "dialog.projects.subtitle",
        "The folders this window can run in, the same list the sidebar draws. Picking one opens it here.",
        "此窗口可以运行的文件夹，与侧边栏中的列表相同。选择其中一个即在此打开。",
    ),
    ("dialog.worktrees.title", "Worktrees", "工作树"),
    (
        "dialog.worktrees.subtitle",
        "Run a chat in an isolated checkout. Choose a starting branch or commit; your local checkout stays in place.",
        "在独立的检出中运行对话。选择起始分支或提交；本地检出保持原样。",
    ),
    ("dialog.worktrees.recheck", "Recheck the worktrees", "重新检查工作树"),
    ("dialog.worktrees.name", "New worktree name", "新工作树名称"),
    ("dialog.worktrees.create", "Create worktree", "创建工作树"),
    ("dialog.worktrees.startFrom", "Start from", "起始于"),
    (
        "dialog.worktrees.basePlaceholder",
        "HEAD (current checkout)",
        "HEAD（当前检出）",
    ),
    (
        "dialog.worktrees.includeChanges",
        "Include uncommitted changes from the current checkout",
        "包含当前检出中未提交的更改",
    ),
    (
        "dialog.worktrees.detached",
        "Detached HEAD (create a branch later)",
        "分离 HEAD（稍后创建分支）",
    ),
    ("dialog.worktrees.handoff", "Hand off chat to Worktree", "将对话移交到工作树"),
    ("dialog.worktrees.createBranch", "Create branch here", "在此创建分支"),
    ("dialog.mcps.title", "MCP servers", "MCP 服务器"),
    (
        "dialog.mcps.subtitle",
        "The servers this project loads, and whether Oxide can reach them. They come from the same mcp.json files the CLI reads, and turning one off writes to the file that defines it.",
        "此项目加载的服务器，以及 Oxide 能否连接它们。它们来自 CLI 读取的同一批 mcp.json 文件，关闭某个服务器会写入定义它的文件。",
    ),
    ("dialog.mcps.recheck", "Recheck the servers", "重新检查服务器"),
    ("dialog.mcps.manage", "Manage MCP servers", "管理 MCP 服务器"),
    ("dialog.sessions.title", "Sessions", "会话"),
    (
        "dialog.sessions.subtitle",
        "Threads stored for this project, most recent first. Picking one opens it here; the sidebar shows the same list beside every project.",
        "此项目已保存的对话，最新的在前。选择其中一个即在此打开；侧边栏会在每个项目旁显示相同的列表。",
    ),
    ("dialog.sessions.new", "New thread", "新建对话"),
    ("dialog.sessions.search", "Search sessions…", "搜索会话…"),
    // The editor panel's own chrome, which is narrower than the app's.
    ("panel.newChat", "New chat", "新建对话"),
    ("panel.sessionHistory", "Session history", "会话历史"),
    (
        "panel.placeholder",
        "Ask Oxide…  Enter to send · Shift+Enter for a newline · paste or drop an image",
        "询问 Oxide… Enter 发送 · Shift+Enter 换行 · 粘贴或拖入图片",
    ),
    (
        "panel.attachHint",
        "Attach images, PDFs or files (paste or drop them here too)",
        "附加图片、PDF 或文件（也可粘贴或拖入）",
    ),
    (
        "panel.homeLead",
        "Ask Oxide to make a change, explain code, or run something.",
        "让 Oxide 修改代码、解释代码或运行命令。",
    ),
    ("dialog.sources.title", "Sources", "来源"),
    ("dialog.sources.add", "Add sources", "添加来源"),
    ("dialog.sources.viewAll", "View all", "查看全部"),
    ("dialog.agents.title", "Agents", "代理"),
    ("dialog.agents.subtitle", "Choose the agent this chat runs as.", "选择此对话运行的代理。"),
    ("dialog.trust.title", "Trust this project?", "信任此项目？"),
    (
        "dialog.trust.body",
        "Trusting this project loads its local resources, which can run commands or change how the agent behaves. This decision is shared with the CLI and extension.",
        "信任此项目将加载它的本地资源，这些资源可能运行命令或改变代理的行为。此决定与 CLI 和扩展共享。",
    ),
    ("dialog.trust.deny", "Don't trust", "不信任"),
    ("dialog.trust.allow", "Trust project", "信任项目"),
    ("dialog.models.title", "Model", "模型"),
    ("dialog.models.filter", "Filter models…", "筛选模型…"),
    ("dialog.reasoning.title", "Reasoning effort", "推理强度"),
    ("dialog.themes.title", "Theme", "主题"),
    (
        "dialog.themes.subtitle",
        "Built-in and .oxide/themes files, shared with the CLI.",
        "内置主题和 .oxide/themes 文件，与 CLI 共享。",
    ),
    ("dialog.permissions.title", "Tool approvals", "工具审批"),
    (
        "dialog.permissions.subtitle",
        "Tools allowed without prompting in this project.",
        "在此项目中无需询问即可使用的工具。",
    ),
    ("dialog.permissions.clear", "Clear all", "全部清除"),
    ("dialog.confirm.title", "Confirm", "确认"),
    ("dialog.confirm.ok", "Confirm", "确认"),
    ("dialog.rename.title", "Rename thread", "重命名对话"),
    ("dialog.rename.name", "Name", "名称"),
    ("dialog.help.title", "Keyboard shortcuts", "键盘快捷键"),
    ("help.enter", "Send with the default follow-up behavior", "按默认后续方式发送"),
    ("help.newline", "Newline", "换行"),
    (
        "help.opposite",
        "Use the opposite follow-up behavior once",
        "本次改用相反的后续方式",
    ),
    ("help.paste", "Attach a clipboard image", "附加剪贴板中的图片"),
    ("help.newTab", "New chat tab", "新建对话标签页"),
    ("help.closeTab", "Close chat tab", "关闭对话标签页"),
    ("help.cycleTab", "Next / previous chat tab", "下一个 / 上一个对话标签页"),
    (
        "help.sessionKey",
        "Open the thread at that place in the sidebar",
        "打开侧边栏中对应位置的对话",
    ),
    ("help.reasoning", "Cycle reasoning", "循环切换推理强度"),
    ("help.model", "Open the model picker", "打开模型选择器"),
    ("help.help", "Toggle this help", "显示或隐藏此帮助"),
    ("help.reviewFiles", "Walk a change review's files", "在变更审查的文件间移动"),
    ("help.escape", "Close any dialog", "关闭任何对话框"),
    (
        "palette.commandsHint",
        "↑↓ choose · Enter run · Esc close",
        "↑↓ 选择 · Enter 运行 · Esc 关闭",
    ),
    (
        "palette.pathsHint",
        "↑↓ choose · Enter insert · Esc close",
        "↑↓ 选择 · Enter 插入 · Esc 关闭",
    ),
    ("toast.notifications", "Notifications", "通知"),
    ("dialog.image.preview", "Image preview", "图片预览"),
    ("dialog.update.title", "Updates", "更新"),
    ("dialog.update.notes", "Release notes", "发行说明"),
    ("dialog.update.background", "Download in background", "后台下载"),
    ("dialog.update.install", "Update", "更新"),
    ("dialog.update.available", "Update Available (v{version})", "有可用更新（v{version}）"),
    ("dialog.update.current", "Oxide {version} is up to date", "Oxide {version} 已是最新版本"),
    ("dialog.update.restart", "Restart and install update", "重启并安装更新"),
    ("dialog.createProject.title", "Create project", "创建项目"),
    ("dialog.createProject.name", "Project name", "项目名称"),
    ("dialog.createProject.sources", "Source folders", "源文件夹"),
    ("dialog.createProject.addFolder", "+ Add a folder", "+ 添加文件夹"),
    ("dialog.createProject.addPath", "Add path", "添加路径"),
    (
        "dialog.createProject.pathPlaceholder",
        "Or type a path, e.g. ~/Projects/app",
        "或输入路径，例如 ~/Projects/app",
    ),
    ("dialog.createProject.remove", "Remove project", "移除项目"),
    ("dialog.connect.title", "Connect a provider", "连接服务商"),
    (
        "dialog.connect.subtitle",
        "Credentials are stored in the same auth.json the CLI uses.",
        "凭据保存在 CLI 使用的同一个 auth.json 中。",
    ),
    ("dialog.connect.search", "Search providers…", "搜索服务商…"),
    ("dialog.connect.key", "API key", "API 密钥"),
    ("dialog.connect.keyPlaceholder", "Paste your API key", "粘贴你的 API 密钥"),
    ("dialog.connect.model", "Model (optional)", "模型（可选）"),
    ("dialog.connect.url", "Base URL (optional)", "基础 URL（可选）"),
    ("dialog.connect.default", "default", "默认"),
    ("dialog.approval.title", "Permission required", "需要授权"),
    (
        "dialog.approval.subtitle",
        "Oxide wants to run a tool that needs your approval.",
        "Oxide 想运行一个需要你批准的工具。",
    ),
    ("dialog.approval.deny", "Deny", "拒绝"),
    ("dialog.approval.once", "Allow once", "允许一次"),
    ("dialog.approval.always", "Always allow", "始终允许"),
    ("dialog.question.title", "Question", "问题"),
    (
        "dialog.question.step",
        "{index} of {total} questions",
        "第 {index} 个问题，共 {total} 个",
    ),
    // A tool call's own answer, as a card names it.
    ("approval.runCommand", "Run a shell command", "运行 shell 命令"),
    ("approval.editFile", "Edit a file", "编辑文件"),
    ("approval.writeFile", "Write a file", "写入文件"),
    ("approval.readFile", "Read a file", "读取文件"),
    ("approval.searchFiles", "Search the project", "搜索项目"),
    ("approval.useTool", "Run a tool", "运行工具"),
    // The terminal's own transcript.
    ("tui.thinking", "Thinking", "思考中"),
    ("tui.thoughtFor", "Thought for {duration}", "思考了 {duration}"),
    ("tui.jumpLatest", "↓ Jump to latest message · End", "↓ 跳到最新消息 · End"),
    ("tui.steering", "Steering", "引导"),
    ("tui.followUp", "Follow-up", "后续"),
    (
        "tui.editQueued",
        "↳ {key} to edit all queued messages",
        "↳ {key} 可编辑全部排队消息",
    ),
    (
        "tui.moreQueued",
        "⋯ {count} more queued messages",
        "⋯ 还有 {count} 条排队消息",
    ),
    (
        "tui.moreQueuedOne",
        "⋯ {count} more queued message",
        "⋯ 还有 {count} 条排队消息",
    ),
    (
        "tui.approve",
        "approve `{tool}`? {hint}",
        "批准 `{tool}`？{hint}",
    ),
    (
        "tui.approvalHint",
        "y = once · a = always (this project) · n [reason] = deny",
        "y = 一次 · a = 始终（此项目） · n [原因] = 拒绝",
    ),
    (
        "tui.waitingApproval",
        "waiting for approval",
        "等待批准",
    ),
    (
        "tui.steerQueue",
        "Enter steer · Alt+Enter queue",
        "Enter 引导 · Alt+Enter 排队",
    ),
    // Interface strings audited across the terminal and VS Code panel.
    ("surface.newChat", "New chat", "新建对话"),
    ("surface.open", "Open", "打开"),
    ("surface.recheck", "Recheck", "重新检查"),
    ("surface.close", "Close", "关闭"),
    ("surface.searchSessionsPlaceholder", "Search sessions…", "搜索会话…"),
    ("surface.searchSessionsLabel", "Search sessions", "搜索会话"),
    ("surface.clearTheSearch", "Clear the search", "清除搜索"),
    ("surface.askOxideToMakeAChangeExplainCodeOrRunSomething", "Ask Oxide to make a change, explain code, or run something.", "让 Oxide 修改代码、解释代码或运行命令。"),
    ("surface.queuedMessages", "Queued messages", "排队消息"),
    ("surface.queuedMessageActions", "Queued message actions", "排队消息操作"),
    ("surface.filesAndFolders", "Files and folders", "文件和文件夹"),
    ("surface.turnSettings", "Turn settings", "本轮设置"),
    ("surface.modelAndReasoning", "Model and reasoning", "模型与推理"),
    ("surface.dropFilesToAttach", "Drop files to attach", "拖放文件以添加附件"),
    ("surface.imagePreview", "Image preview", "图片预览"),
    ("surface.closeEsc", "Close (Esc)", "关闭 (Esc)"),
    ("surface.attachmentPreview", "Attachment preview", "附件预览"),
    ("panel.sharedRuntime", "Runs use the same configuration, sessions and project trust as the terminal: `oxide` starts a turn with `--mode rpc`, so a tool that needs your approval waits for an answer here.", "运行时与终端共用配置、会话和项目信任设置：`oxide` 通过 `--mode rpc` 启动一轮任务，需要你批准的工具会在这里等待答复。"),
    ("surface.onlyImagesAndPdfsCanBeAttachedFromAPasteOrADrop", "Only images and PDFs can be attached from a paste or a drop.", "粘贴或拖放时只能添加图片和 PDF。"),
    ("surface.takeThisFileOutOfTheNextMessage", "Take this file out of the next message", "从下一条消息中移除此文件"),
    ("surface.removeFromTheNextMessage", "Remove from the next message", "从下一条消息中移除"),
    ("surface.theFileYouAreEditingSentWithTheNextMessage", "the file you are editing, sent with the next message", "正在编辑的文件，将随下一条消息发送"),
    ("surface.inlinedIntoTheNextMessage", "inlined into the next message", "内容将嵌入下一条消息"),
    ("surface.commandsAndSkills", "Commands and skills", "命令和技能"),
    ("surface.UsageValue", "Usage: {value}", "用量：{value}"),
    ("surface.QueuedCount", "{text} · {count} queued", "{text} · {count} 条排队消息"),
    ("surface.ClickToChange", "{name} — click to change", "{name} — 点击修改"),
    ("surface.InlineFile", "{name} — inlined into the next message", "{name} — 内容将嵌入下一条消息"),
    ("surface.Skill", "skill", "技能"),
    ("surface.EarlierLines", "Show {count} earlier lines", "显示前面的 {count} 行"),
    ("surface.MoreLines", "Show {count} more lines", "再显示 {count} 行"),
    ("surface.AttachmentLimit", "{name} is past the {size} MB attachment limit.", "{name} 超过 {size} MB 的附件大小限制。"),
    ("surface.ReadAttachment", "Could not read {name}.", "无法读取 {name}。"),
    ("surface.thatWorktreeIsNoLongerThere", "That worktree is no longer there.", "该工作树已不存在。"),
    ("surface.onlyFilesInsideTheCurrentProjectCanBeAttached", "Only files inside the current project can be attached.", "只能添加当前项目内的文件。"),
    ("surface.onlyImagesPdfsAndTextFilesCanBeAttached", "Only images, PDFs and text files can be attached.", "只能添加图片、PDF 和文本文件。"),
    ("surface.theClipboardHoldsNoImageOrPdfToAttach", "The clipboard holds no image or PDF to attach.", "剪贴板中没有可添加的图片或 PDF。"),
    ("surface.waitForTheChatHandoffToFinish", "Wait for the chat handoff to finish.", "请等待对话移交完成。"),
    ("surface.theActiveResponseFinishedQueuedThisMessageForTheNextTurn", "The active response finished; queued this message for the next turn.", "当前回复已结束，此消息已排队等待下一轮发送。"),
    ("surface.thatApprovalRequestIsNoLongerWaiting", "That approval request is no longer waiting.", "该批准请求已结束。"),
    ("surface.thatQuestionIsNoLongerWaiting", "That question is no longer waiting.", "该问题已结束。"),
    ("surface.nothingIsRunning", "Nothing is running.", "当前没有正在运行的任务。"),
    ("surface.runStoppedTheNextMessageContinuesThisSession", "Run stopped. The next message continues this session.", "任务已停止，下一条消息将继续此会话。"),
    ("surface.oxideProducedNoEventsSeeTheOxideOutputChannel", "oxide produced no events; see the Oxide output channel.", "oxide 未产生事件，请查看 Oxide 输出通道。"),
    ("surface.aTurnIsRunningStopItBeforeStartingANewSession", "A turn is running; stop it before starting a new session.", "任务正在运行，请先停止再新建会话。"),
    ("surface.newChatTheNextMessageStartsAThreadOfItsOwn", "New chat: the next message starts a thread of its own.", "新建对话：下一条消息将开启独立的会话。"),
    ("surface.aTurnIsRunningStopItBeforeSwitchingSessions", "A turn is running; stop it before switching sessions.", "任务正在运行，请先停止再切换会话。"),
    ("surface.theNextMessageContinuesTheMostRecentSession", "The next message continues the most recent session.", "下一条消息将继续最近的会话。"),
    ("surface.openAFolderBeforeClearingApprovals", "Open a folder before clearing approvals.", "请先打开文件夹，再清除批准记录。"),
    ("surface.forgotEveryToolThisProjectAllowedWithoutPrompting", "Forgot every tool this project allowed without prompting.", "已清除该项目的全部工具自动批准记录。"),
    ("surface.stopTheRunningTurnBeforeUndoingATurn", "Stop the running turn before undoing a turn.", "请先停止正在运行的任务，再撤销更改。"),
    ("surface.openAFolderBeforeUndoingATurn", "Open a folder before undoing a turn.", "请先打开文件夹，再撤销更改。"),
    ("surface.putTheTurnSFilesBack", "Put the turn's files back.", "已还原本轮修改的文件。"),
    ("surface.openAFolderBeforeSwitchingProviders", "Open a folder before switching providers.", "请先打开文件夹，再切换提供商。"),
    ("surface.openAFolderBeforeDecidingItsAccess", "Open a folder before deciding its access.", "请先打开文件夹，再设置信任权限。"),
    ("surface.noUsageReportedYetTheFooterFillsInPerTurn", "No usage reported yet — the footer fills in per turn.", "尚无用量记录，每轮结束后会更新底部用量。"),
    ("surface.checkingCheckouts", "Checking checkouts…", "正在检查检出目录…"),
    ("surface.theWorktreeWasMadeButTheCliDidNotNameIt", "The worktree was made, but the CLI did not name it.", "工作树已创建，但 CLI 未返回其名称。"),
    ("surface.couldNotReadThisProjectSWorktreesValue", "Could not read this project's worktrees: {value}", "无法读取该项目的工作树：{value}"),
    ("surface.couldNotReadTheClipboardValue", "Could not read the clipboard: {value}", "无法读取剪贴板：{value}"),
    ("surface.valueCouldNotBeRead", "{value} could not be read.", "无法读取 {value}。"),
    ("surface.valueIsAlreadyAttached", "{value} is already attached.", "已添加 {value}。"),
    ("surface.atMostValueAttachmentsPerMessage", "At most {value} attachments per message.", "每条消息最多可添加 {value} 个附件。"),
    ("surface.couldNotClearTheSavedApprovalsValue", "Could not clear the saved approvals: {value}", "无法清除保存的批准记录：{value}"),
    ("surface.valueNeedsAKey", "{value} needs a key.", "{value} 需要密钥。"),
    ("surface.connectedValue", "Connected {value}.", "已连接 {value}。"),
    ("surface.installedOxideValue", "Installed Oxide {value}.", "已安装 Oxide {value}。"),
    ("surface.couldNotUndoTheTurnValue", "Could not undo the turn: {value}", "无法撤销本轮更改：{value}"),
    ("surface.couldNotSwitchLanguageValue", "Could not switch language: {value}", "无法切换语言：{value}"),
    ("surface.couldNotSelectModelValue", "Could not select model: {value}", "无法选择模型：{value}"),
    ("surface.modelSetToValue", "Model set to {value}.", "模型已设为 {value}。"),
    ("surface.couldNotSaveTheTrustDecisionValue", "Could not save the trust decision: {value}", "无法保存信任设置：{value}"),

    ("terminal.argument.Enable", "enable", "启用"),
    ("terminal.argument.Disable", "disable", "禁用"),
    ("terminal.argument.LookForANewerReleaseAtLaunch", "look for a newer release at launch", "启动时检查新版本"),
    ("terminal.argument.StopLookingAtLaunch", "stop looking at launch", "启动时不检查更新"),
    ("terminal.argument.ListMarketplaces", "list marketplaces", "列出市场"),
    ("terminal.argument.AddAMarketplace", "add a marketplace", "添加市场"),
    ("terminal.argument.FetchAMarketplaceSLatestManifest", "fetch a marketplace's latest manifest", "获取市场的最新清单"),
    ("terminal.argument.RemoveAMarketplace", "remove a marketplace", "移除市场"),
    ("terminal.argument.ListInstalledPlugins", "list installed plugins", "列出已安装的插件"),
    ("terminal.argument.InstallAPlugin", "install a plugin", "安装插件"),
    ("terminal.argument.RemoveAPlugin", "remove a plugin", "移除插件"),
    ("terminal.argument.EnableAPlugin", "enable a plugin", "启用插件"),
    ("terminal.argument.DisableAPlugin", "disable a plugin", "禁用插件"),
    ("terminal.argument.ManageMarketplaces", "manage marketplaces", "管理市场"),
    ("terminal.argument.ListPendingAttachments", "list pending attachments", "列出待发送的附件"),
    ("terminal.argument.RemoveOneByIdOrNumber", "remove one by id or number", "按 ID 或序号移除附件"),
    ("terminal.argument.RemoveEveryAttachment", "remove every attachment", "移除全部附件"),
    ("terminal.argument.CopyTheWholeTranscript", "copy the whole transcript", "复制全部对话记录"),
    ("terminal.argument.EnableTheCompletionToast", "enable the completion toast", "启用完成通知"),
    ("terminal.argument.DisableTheCompletionToast", "disable the completion toast", "禁用完成通知"),
    ("terminal.argument.ToggleTheAlertSound", "toggle the alert sound", "切换提示音"),
    ("terminal.argument.SendASampleNotification", "send a sample notification", "发送测试通知"),
    ("terminal.argument.AskBeforeAGatedToolRuns", "ask before a gated tool runs", "运行受限工具前询问"),
    ("terminal.argument.RunGatedToolsWithoutAsking", "run gated tools without asking", "直接运行受限工具"),
    ("terminal.argument.ShowTheStateAndThisProjectSRules", "show the state and this project's rules", "显示状态和当前项目规则"),
    ("terminal.argument.ForgetThisProjectSAlwaysAllowRules", "forget this project's always-allow rules", "清除当前项目的始终允许规则"),
    ("terminal.argument.ShowTheStatusAndSyntax", "show the status and syntax", "显示状态和语法"),
    ("terminal.argument.ShowTheBar", "show the bar", "显示用量栏"),
    ("terminal.argument.HideTheBar", "hide the bar", "隐藏用量栏"),
    ("terminal.argument.UserWhoseSpendToShow", "user whose spend to show", "要显示支出的用户"),
    ("terminal.argument.UsageApiKey", "usage API key", "用量 API 密钥"),
    ("terminal.argument.MonthlyBudget", "monthly budget", "每月预算"),
    ("terminal.argument.BudgetCurrency", "budget currency", "预算币种"),
    ("terminal.argument.UsDollars", "US dollars", "美元"),
    ("terminal.argument.ChineseYuan", "Chinese yuan", "人民币"),
    ("terminal.argument.MetadataKeyHoldingTheUser", "metadata key holding the user", "存储用户的元数据键"),
    ("terminal.argument.LetTheModelDecide", "let the model decide", "由模型决定"),
    ("terminal.argument.DisableReasoning", "disable reasoning", "禁用推理"),
    ("terminal.argument.MinimalEffort", "minimal effort", "最低推理强度"),
    ("terminal.argument.LowEffort", "low effort", "低推理强度"),
    ("terminal.argument.MediumEffort", "medium effort", "中推理强度"),
    ("terminal.argument.HighEffort", "high effort", "高推理强度"),
    ("terminal.argument.ExtraHighEffort", "extra-high effort", "超高推理强度"),
    ("terminal.argument.MaximumEffort", "maximum effort", "最高推理强度"),
    ("terminal.argument.ShowTheSavedDecision", "show the saved decision", "显示已保存的决定"),
    ("terminal.argument.TrustThisProject", "trust this project", "信任此项目"),
    ("terminal.argument.DeclineThisProject", "decline this project", "不信任此项目"),
    ("terminal.argument.ReportTheLanguageInUse", "report the language in use", "显示当前使用的语言"),
    ("terminal.argument.EnglishUnitedKingdom", "English (United Kingdom)", "英语（英国）"),
    ("terminal.argument.Chinese", "简体中文", "简体中文"),
    ("terminal.message.WelcomeConnectAModelProviderToSendYourFirstMessage", "Welcome! Connect a model provider to send your first message.", "欢迎！连接模型提供商以发送第一条消息。"),
    ("terminal.message.SetupRequired", "setup required", "需要设置"),
    ("terminal.message.ThisModelHasNoConfigurableReasoningEffort", "This model has no configurable reasoning effort.", "此模型不支持配置推理强度。"),
    ("terminal.message.Denied", "denied", "已拒绝"),
    ("terminal.notice.CountSessionSPickOne", "{count} session(s) — pick one", "{count} 个会话 — 请选择"),
    ("terminal.notice.CountModelSPickOne", "{count} model(s) — pick one", "{count} 个模型 — 请选择"),
    ("terminal.notice.InstallingPluginName", "installing plugin `{name}`...", "正在安装插件 `{name}`…"),
    ("terminal.notice.AddingMarketplaceSubRest", "adding marketplace `{sub_rest}`...", "正在添加市场 `{sub_rest}`…"),
    ("terminal.notice.UpdatingMarketplaceSubRest", "updating marketplace `{sub_rest}`...", "正在更新市场 `{sub_rest}`…"),
    ("terminal.notice.ResumedId", "resumed {id}", "已恢复 {id}"),
    ("terminal.notice.OpeningUrl", "opening {url}", "正在打开 {url}"),
    ("terminal.notice.CouldNotOpenUrlErr", "could not open {url}: {err}", "无法打开 {url}：{err}"),
    ("terminal.notice.FailedToPasteFromClipboardError", "Failed to paste from clipboard: {error}", "无法从剪贴板粘贴：{error}"),
    ("terminal.notice.SessionNamedName", "session named `{name}`", "会话已命名为 `{name}`"),
    ("terminal.notice.ModelSetToRequested", "model set to {requested}", "模型已设为 {requested}"),
    ("terminal.notice.ThemeSetToRequested", "theme set to {requested}", "主题已设为 {requested}"),
    ("terminal.notice.UnknownThemeRequested", "unknown theme `{requested}`", "未知主题 `{requested}`"),
    ("terminal.notice.UnknownAgentName", "unknown agent `{name}`", "未知智能体 `{name}`"),
    ("extension.ATurnIsRunningStopItBeforeSwitchingCheckouts", "A turn is running; stop it before switching checkouts.", "任务正在运行，请先停止再切换检出目录。"),
    ("extension.ATurnIsRunningStopItBeforeMakingAWorktree", "A turn is running; stop it before making a worktree.", "任务正在运行，请先停止再创建工作树。"),
    ("extension.ATurnIsRunningStopItBeforeRemovingAWorktree", "A turn is running; stop it before removing a worktree.", "任务正在运行，请先停止再移除工作树。"),
    ("extension.OxideFinished", "Oxide finished.", "Oxide 已完成。"),
    ("extension.CreatedTheWorktreeValue0OnValue1", "Created the worktree {value0} on {value1}.", "已创建工作树 {value0}，分支为 {value1}。"),
    ("extension.AttachedTheFirstValue0LinesOfValue1", "Attached the first {value0} lines of {value1}.", "已添加 {value1} 的前 {value0} 行。"),
    ("extension.Value0IsNotATextFileAnImageOrAPdf", "{value0} is not a text file, an image or a PDF.", "{value0} 不是文本文件、图片或 PDF。"),
    ("extension.Value0IsValue1TheAttachmentLimitIsValue2", "{value0} is {value1}; the attachment limit is {value2}.", "{value0} 的大小为 {value1}，附件限制为 {value2}。"),
    ("extension.CouldNotWriteValue0ToAFile", "Could not write {value0} to a file.", "无法将 {value0} 写入文件。"),
    ("extension.Value0IsNotOneThisPanelRunsUseTheTerminal", "/{value0} is not one this panel runs — use the terminal.", "此面板不支持运行 /{value0}，请使用终端。"),
    ("extension.SteeringValue0", "Steering: {value0}", "正在引导：{value0}"),
    ("extension.Value0IsWaitingForApprovalOpenItInTheStripAboveToAnswer", "“{value0}” is waiting for approval — open it in the strip above to answer.", "“{value0}”正在等待批准，请从上方状态栏打开该对话以答复。"),
    ("extension.Value0IsWaitingForYourAnswerOpenItInTheStripAboveToAnswer", "“{value0}” is waiting for your answer — open it in the strip above to answer.", "“{value0}”正在等待你的回答，请从上方状态栏打开该对话以答复。"),
    ("extension.Value0FinishedOpenItInTheStripAboveToReadTheReply", "{value0} finished. Open it in the strip above to read the reply.", "{value0} 已完成，请从上方状态栏打开该对话以阅读回复。"),
    ("extension.Value0IsResumedButItsHistoryCouldNotBeReadValue1", "{value0} is resumed, but its history could not be read: {value1}", "已恢复 {value0}，但无法读取历史记录：{value1}"),
    ("extension.Value0IsResumedButItsHistoryCouldNotBeRead", "{value0} is resumed, but its history could not be read.", "已恢复 {value0}，但无法读取历史记录。"),
    ("extension.TheNewestValue0OfValue1Messages", " (the newest {value0} of {value1} messages)", "（最近 {value0} 条，共 {value1} 条消息）"),
    ("extension.ResumedValue0Value1TheNextMessageContinuesThisThread", "Resumed {value0}{value1} — the next message continues this thread.", "已恢复 {value0}{value1}，下一条消息将继续此对话。"),
    ("extension.ThreadArchivedToPermanentlyDeleteItsLogRunOxideSessionsDeleteValue0FromItsProjectFolderToRestoreItRu", "Thread archived. To permanently delete its log, run oxide sessions delete {value0} from its project folder. To restore it, run oxide sessions unarchive {value1}.", "对话已归档。要永久删除记录，请在项目文件夹中运行 oxide sessions delete {value0}；要恢复对话，请运行 oxide sessions unarchive {value1}。"),
    ("extension.CouldNotSignOutOfValue0Value1", "Could not sign out of {value0}: {value1}", "无法退出 {value0}：{value1}"),
    ("extension.CouldNotReadTheSavedApprovalsValue0", "Could not read the saved approvals: {value0}", "无法读取保存的批准记录：{value0}"),
    ("extension.OxideConnectValue0", "Oxide: connect {value0}", "Oxide：连接 {value0}"),
    ("extension.PasteTheApiKeyForValue0IssuedAtValue1", "Paste the API key for {value0} — issued at {value1}", "粘贴 {value0} 的 API 密钥，可在 {value1} 获取"),
    ("extension.PasteTheApiKeyForValue0", "Paste the API key for {value0}", "粘贴 {value0} 的 API 密钥"),
    ("extension.OxideValue0IsAvailableInstalledValue1", "Oxide {value0} is available (installed: {value1}).", "Oxide {value0} 已发布（已安装：{value1}）。"),
    ("extension.OxideValue0WasInstalledRestartTheWindowToRunIt", "Oxide {value0} was installed. Restart the window to run it.", "已安装 Oxide {value0}，重启窗口即可使用。"),
    ("extension.TheTurn", "The turn", "本轮任务"),
    ("extension.pinnedThread", "Pinned {name}.", "已置顶 {name}。"),
    ("extension.unpinnedThread", "Unpinned {name}.", "已取消置顶 {name}。"),
    ("extension.OxideOpenAFileFirst", "Oxide: open a file first.", "Oxide：请先打开文件。"),
    ("extension.OxideOpenTheFileSFolderAsTheActiveWorkspaceBeforeReferencingIt", "Oxide: open the file's folder as the active workspace before referencing it.", "Oxide：请先将该文件所在文件夹设为活动工作区，再引用文件。"),
    ("extension.OxideAskAboutTheSelection", "Oxide: ask about the selection", "Oxide：询问选中内容"),
    ("extension.WhatDoYouWantToKnow", "What do you want to know?", "你想了解什么？"),
    ("extension.OxideOnlyFilesOnDiskCanBeAttached", "Oxide: only files on disk can be attached.", "Oxide：只能添加磁盘上的文件。"),
    ("extension.OxideOpenTheFileSFolderAsTheActiveWorkspaceBeforeAttachingIt", "Oxide: open the file's folder as the active workspace before attaching it.", "Oxide：请先将该文件所在文件夹设为活动工作区，再添加文件。"),
    ("extension.ClickToOpenTheChatChooseAProviderWithTheConnectProviderCommandOrConnectInTheComposer", "Click to open the chat. Choose a provider with the Connect Provider command, or `/connect` in the composer.", "点击打开对话。可通过“连接提供商”命令或输入框中的 `/connect` 选择提供商。"),
    ("extension.Idle", "Idle", "空闲"),
    ("extension.Thinking", "Thinking…", "思考中…"),
    ("extension.Writing", "Writing…", "正在撰写…"),
    ("extension.WaitingForApproval", "Waiting for approval…", "等待批准…"),
    ("extension.WaitingForYourAnswer", "Waiting for your answer…", "等待你的回答…"),
    ("extension.Done", "Done", "已完成"),
    ("extension.RunningTool", "Running {tool}…", "正在运行 {tool}…"),
    ("extension.RetryingAttemptMaxInSecondsS", "Retrying ({attempt}/{max}) in {seconds}s…", "将在 {seconds} 秒后重试（{attempt}/{max}）…"),
    ("extension.RetryingAttemptMax", "Retrying ({attempt}/{max})…", "正在重试（{attempt}/{max}）…"),
    ("cli.help.PromptWordsAndFileReferencesPiStyleOxideFileMessage", "Prompt words and `@file` references (Pi-style `oxide @file \"message\"`)", "提示词和 `@file` 引用（例如 `oxide @file \"message\"`）"),
    ("cli.help.ModelToUseOverridesConfig", "Model to use (overrides config)", "使用的模型（覆盖配置）"),
    ("cli.help.ProviderNameOverridesConfig", "Provider name (overrides config)", "提供商名称（覆盖配置）"),
    ("cli.help.AgentToRunFromOxideAgentsOrClaudeAgents", "Agent to run (from .oxide/agents or .claude/agents)", "要运行的智能体（来自 .oxide/agents 或 .claude/agents）"),
    ("cli.help.OutputModePrintJsonOrRpcDefaultsToPrintForPrompts", "Output mode: `print`, `json`, or `rpc` (defaults to print for prompts)", "输出模式：`print`、`json` 或 `rpc`（有提示词时默认为 print）"),
    ("cli.help.ReasoningEffortAutoDefaultOffMinimalLowMediumHighXhighOrMax", "Reasoning effort: auto (default), off, minimal, low, medium, high, xhigh, or max", "推理强度：auto（默认）、off、minimal、low、medium、high、xhigh 或 max"),
    ("cli.help.AppendTextToTheSystemPromptRepeatable", "Append text to the system prompt (repeatable)", "向系统提示词追加文本（可重复）"),
    ("cli.help.ReplaceTheDefaultSystemPrompt", "Replace the default system prompt", "替换默认系统提示词"),
    ("cli.help.DisableAgentsMdAndClaudeMdContextFileDiscovery", "Disable AGENTS.md and CLAUDE.md context file discovery", "禁用 AGENTS.md 和 CLAUDE.md 上下文文件发现"),
    ("cli.help.ThemeNameForTheTuiDarkLightOrACustomOxideThemesFile", "Theme name for the TUI (dark, light, or a custom .oxide/themes file)", "终端界面主题名称（dark、light 或自定义 .oxide/themes 文件）"),
    ("cli.help.InterfaceLanguageForThisRunEnGbTheDefaultOrZhCn", "Interface language for this run: `en-GB` (the default) or `zh-CN`", "本次运行的界面语言：`en-GB`（默认）或 `zh-CN`"),
    ("cli.help.TrustProjectLocalResourcesForThisRun", "Trust project-local resources for this run", "本次运行信任项目本地资源"),
    ("cli.help.IgnoreProjectLocalResourcesForThisRun", "Ignore project-local resources for this run", "本次运行忽略项目本地资源"),
    ("cli.help.AskBeforeRunningAToolWhosePermissionRuleRequiresApprovalTheTuiAndModeRpcWhereTheFrontEndAnswersOverItsChannel", "Ask before running a tool whose permission rule requires approval (the TUI, and `--mode rpc` where the front-end answers over its channel)", "在运行权限规则要求批准的工具前询问（终端界面，或由前端通过通道答复的 `--mode rpc`）"),
    ("cli.help.RunPermissionGatedToolsWithoutAsking", "Run permission-gated tools without asking", "无需询问即运行受权限规则限制的工具"),
    ("cli.help.LetTheModelPutAQuestionToTheUserThroughTheAskToolWhichModeRpcAnswersWithAQuestionFrameWithoutItTheToolIsNotOfferedS", "Let the model put a question to the user through the `ask` tool, which `--mode rpc` answers with a `question` frame. Without it the tool is not offered, so a run with nobody to answer never asks", "允许模型通过 `ask` 工具提问，`--mode rpc` 通过 `question` 帧答复；未启用时不提供此工具，以免任务等待无人回答的问题"),
    ("cli.help.AllowlistSpecificToolsCommaSeparated", "Allowlist specific tools (comma-separated)", "允许指定的工具（用逗号分隔）"),
    ("cli.help.DisableSpecificToolsCommaSeparated", "Disable specific tools (comma-separated)", "禁用指定的工具（用逗号分隔）"),
    ("cli.help.UseASpecificSessionFileOrId", "Use a specific session file or ID", "使用指定的会话文件或 ID"),
    ("cli.help.SetTheSessionDisplayNameAtStartup", "Set the session display name at startup", "启动时设置会话显示名称"),
    ("cli.help.EphemeralModeDoNotSaveTheSession", "Ephemeral mode: do not save the session", "临时模式：不保存会话"),
    ("cli.help.PrintTheResponseAndExitInsteadOfLaunchingTheTui", "Print the response and exit instead of launching the TUI", "打印回复并退出，不启动终端界面"),
    ("cli.help.ResumeTheMostRecentSessionForThisProject", "Resume the most recent session for this project", "恢复当前项目最近的会话"),
    ("cli.help.BrowseAndSelectAPastSessionToResumePiStyleR", "Browse and select a past session to resume (Pi-style `-r`)", "浏览并选择要恢复的历史会话（`-r`）"),
    ("cli.help.ForkASessionFileOrIdIntoANewSession", "Fork a session file or id into a new session", "从会话文件或 ID 分支出新会话"),
    ("cli.help.AttachAnImageAPdfOrATextFileToThePromptRepeatable", "Attach an image, a PDF or a text file to the prompt (repeatable)", "向提示词添加图片、PDF 或文本附件（可重复）"),
    ("cli.help.WorkingDirectoryForTheAgent", "Working directory for the agent", "智能体的工作目录"),
    ("cli.help.RunInAGitWorktreeOfTheProjectMakingOneWhenItIsNotThereYetNameNamesItWithoutANameItIsNamedAfterThePrompt", "Run in a git worktree of the project, making one when it is not there yet (`=NAME` names it; without a name it is named after the prompt)", "在项目的 git 工作树中运行，不存在时创建（使用 `=NAME` 指定名称，否则根据提示词命名）"),
    ("cli.help.SaveTheModelOrReasoningChoiceSharedByEveryFrontEnd", "Save the model or reasoning choice shared by every front-end", "保存所有前端共用的模型或推理设置"),
    ("cli.help.ManageMcpServers", "Manage MCP servers", "管理 MCP 服务器"),
    ("cli.help.UninstallOxideAndRemoveRelatedFiles", "Uninstall Oxide and remove related files", "卸载 Oxide 并移除相关文件"),
    ("cli.help.KeepConfigurationFiles", "Keep configuration files", "保留配置文件"),
    ("cli.help.KeepSessionDataMemoryAndSnapshots", "Keep session data, memory, and snapshots", "保留会话数据、记忆和快照"),
    ("cli.help.ShowWhatWouldBeRemovedWithoutRemovingIt", "Show what would be removed without removing it", "预览将移除的内容，不实际移除"),
    ("cli.help.SkipTheConfirmationPrompt", "Skip the confirmation prompt", "跳过确认提示"),
    ("cli.help.UpdateTheCliToTheLatestRelease", "Update the CLI to the latest release", "将 CLI 更新到最新版本"),
    ("cli.help.ReportTheNewestReleaseWithoutInstallingIt", "Report the newest release without installing it", "查看最新版本，不执行安装"),
    ("cli.help.InstallASpecificVersionATagLikeV0260OrABare0260", "Install a specific version (a tag like v0.26.0, or a bare 0.26.0)", "安装指定版本（例如标签 v0.26.0 或版本号 0.26.0）"),
    ("cli.help.InstallEvenWhenTheRunningVersionIsAlreadyCurrent", "Install even when the running version is already current", "即使当前已是最新版本也执行安装"),
    ("cli.help.PrintTheCheckAsJsonForAFrontEndWithCheck", "Print the check as JSON for a front-end (with --check)", "以 JSON 格式向前端输出检查结果（需配合 --check）"),
    ("cli.help.CheckAnotherComponentSReleaseTrainInsteadOfTheCliSForAFrontEndThatCannotLinkOxideCoreWithCheck", "Check another component's release train instead of the CLI's, for a front-end that cannot link oxide-core (with --check)", "检查其他组件的版本，供无法链接 oxide-core 的前端使用（需配合 --check）"),
    ("cli.help.TheVersionTheCallerIsRunningForTheComponentItChecksWithCheck", "The version the caller is running, for the component it checks (with --check)", "调用方正在使用的被检查组件版本（需配合 --check）"),
    ("cli.help.ManageSavedSessions", "Manage saved sessions", "管理保存的会话"),
    ("cli.help.ManageClaudeCodeStylePluginsAndMarketplaces", "Manage Claude Code-style plugins and marketplaces", "管理 Claude Code 风格的插件和市场"),
    ("cli.help.ListTheSlashCommandsAClientCanOffer", "List the slash commands a client can offer", "列出客户端可提供的斜杠命令"),
    ("cli.help.PrintTheListingAsJson", "Print the listing as JSON", "以 JSON 格式输出列表"),
    ("cli.help.ReviewTheToolsAProjectAllowsWithoutPrompting", "Review the tools a project allows without prompting", "查看项目中无需询问即可运行的工具"),
    ("cli.help.ForgetAProviderSStoredCredentials", "Forget a provider's stored credentials", "清除提供商保存的凭据"),
    ("cli.help.ProviderNameAsOxideProvidersListsItTheActiveOneByDefault", "Provider name, as `oxide providers` lists it (the active one by default)", "提供商名称，与 `oxide providers` 列表一致（默认为当前提供商）"),
    ("cli.help.PrintTheResultAsJson", "Print the result as JSON", "以 JSON 格式输出结果"),
    ("cli.help.DecideWhetherAProjectSOwnResourcesLoad", "Decide whether a project's own resources load", "决定是否加载项目自身的资源"),
    ("cli.help.ListModelsAvailableFromConnectedProviders", "List models available from connected providers", "列出已连接提供商的可用模型"),
    ("cli.help.PrintProviderCatalogsAsJson", "Print provider catalogs as JSON", "以 JSON 格式输出提供商的模型目录"),
    ("cli.help.QueryOnlyTheActiveProviderForLatencySensitiveClients", "Query only the active provider (for latency-sensitive clients)", "仅查询当前提供商（供需要低延迟的客户端使用）"),
    ("cli.help.PrintTheActiveModelSReasoningLevelsAndCurrentChoice", "Print the active model's reasoning levels and current choice", "输出当前模型支持的推理等级和当前选项"),
    ("cli.help.WarmTheModelCacheFromTheProviderBeforeAnsweringSoAFrontEndThatNeverOpensTheModelPickerStillLearnsTheModelSOwnLevels", "Warm the model cache from the provider before answering, so a front-end that never opens the model picker still learns the model's own levels", "答复前从提供商预热模型缓存，让未打开模型选择器的前端也能获取模型支持的推理等级"),
    ("cli.help.ReadTheLevelsForThisModelRatherThanTheActiveOne", "Read the levels for this model rather than the active one", "读取指定模型而非当前模型的推理等级"),
    ("cli.help.PrintTheContextWindowARunResolvesForTheActiveModel", "Print the context window a run resolves for the active model", "输出当前模型解析后的上下文窗口"),
    ("cli.help.PrintTheWindowAsJson", "Print the window as JSON", "以 JSON 格式输出上下文窗口"),
    ("cli.help.ReadTheWindowForThisModelRatherThanTheActiveOne", "Read the window for this model rather than the active one", "读取指定模型而非当前模型的上下文窗口"),
    ("cli.help.ListTheProvidersAClientCanConnect", "List the providers a client can connect", "列出客户端可连接的提供商"),
    ("cli.help.ConnectAProviderStoringItsCredential", "Connect a provider, storing its credential", "连接提供商并保存凭据"),
    ("cli.help.ProviderNameAsOxideProvidersListsIt", "Provider name, as `oxide providers` lists it", "提供商名称，与 `oxide providers` 列表一致"),
    ("cli.help.ReadTheApiKeyFromStdinOneLine", "Read the API key from stdin (one line)", "从标准输入读取 API 密钥（一行）"),
    ("cli.help.ModelToUseWithThisProvider", "Model to use with this provider", "用于该提供商的模型"),
    ("cli.help.EndpointToUseWithThisProvider", "Endpoint to use with this provider", "用于该提供商的端点"),
    ("cli.help.ReadTheFilesARunChangedFromItsShadowSnapshots", "Read the files a run changed, from its shadow snapshots", "从影子快照读取任务修改的文件"),
    ("cli.help.ManageTheGitWorktreesAConversationCanRunIn", "Manage the git worktrees a conversation can run in", "管理对话可使用的 git 工作树"),
    ("cli.help.ReadTheSystemClipboardAsAnAttachmentForAFrontEnd", "Read the system clipboard as an attachment for a front-end", "读取系统剪贴板，为前端提供附件"),
    ("cli.help.PrintTheAttachmentAsJsonNameDataurlOrNull", "Print the attachment as JSON (`{\"name\":…,\"dataUrl\":…}`, or `null`)", "以 JSON 格式输出附件（`{\"name\":…,\"dataUrl\":…}` 或 `null`）"),
    ("cli.help.ReportOrChangeTheInterfaceLanguageEveryFrontEndDraws", "Report or change the interface language every front-end draws", "查看或修改所有前端共用的界面语言"),
    ("cli.help.PrintTheLanguageTheAvailableOnesAndTheStringsAsJson", "Print the language, the available ones and the strings as JSON", "以 JSON 格式输出当前语言、可用语言和翻译文本"),
    ("cli.help.ProjectDirectoryTheSettingsAreReadFromDefaultsToTheCurrentOne", "Project directory the settings are read from (defaults to the current one)", "读取设置的项目目录（默认为当前目录）"),
    ("cli.help.ReportTheLanguageInUseTheDefaultWhenNoCommandIsGiven", "Report the language in use (the default when no command is given)", "显示当前使用的语言（未提供子命令时的默认操作）"),
    ("cli.help.SwitchTheInterfaceLanguageForEveryFrontEnd", "Switch the interface language for every front-end", "切换所有前端的界面语言"),
    ("cli.help.LanguageTagEnGbTheDefaultOrZhCn", "Language tag: en-GB (the default) or zh-CN", "语言标签：en-GB（默认）或 zh-CN"),
    ("cli.help.ListThisProjectSWorktrees", "List this project's worktrees", "列出当前项目的工作树"),
    ("cli.help.ProjectDirectoryDefaultsToTheCurrentOne", "Project directory (defaults to the current one)", "项目目录（默认为当前目录）"),
    ("cli.help.MakeAWorktreeOfThisProject", "Make a worktree of this project", "为当前项目创建工作树"),
    ("cli.help.NameItDefaultsToOneDerivedFromTheProjectAndAFreeName", "Name it (defaults to one derived from the project and a free name)", "指定名称（默认根据项目生成可用名称）"),
    ("cli.help.TheCommitOrBranchToStartFromDefaultsToHead", "The commit or branch to start from (defaults to `HEAD`)", "起始提交或分支（默认为 `HEAD`）"),
    ("cli.help.KeepHeadDetachedUntilABranchIsCreated", "Keep HEAD detached until a branch is created", "保持 HEAD 分离，直到创建分支"),
    ("cli.help.IncludeStagedUnstagedAndUntrackedChanges", "Include staged, unstaged and untracked changes", "包含已暂存、未暂存和未跟踪的更改"),
    ("cli.help.CheckoutWhoseHeadAndChangesAreUsed", "Checkout whose HEAD and changes are used", "提供 HEAD 和更改的检出目录"),
    ("cli.help.PrintTheWorktreeAsJson", "Print the worktree as JSON", "以 JSON 格式输出工作树"),
    ("cli.help.RemoveAWorktreeSCheckoutLeavingItsBranchInTheRepository", "Remove a worktree's checkout, leaving its branch in the repository", "移除工作树的检出目录，保留仓库中的分支"),
    ("cli.help.TheNameItWasCreatedUnder", "The name it was created under", "创建时使用的名称"),
    ("cli.help.PrintTheRemovedWorktreeAsJson", "Print the removed worktree as JSON", "以 JSON 格式输出被移除的工作树"),
    ("cli.help.GiveAManagedCheckoutABranch", "Give a managed checkout a branch", "为托管检出目录创建分支"),
    ("cli.help.MoveAChatAndItsChangesBetweenLocalAndItsManagedWorktree", "Move a chat and its changes between Local and its managed worktree", "在本地目录和托管工作树之间移动对话及其更改"),
    ("cli.help.ListTheToolsThisProjectAllowsWithoutPrompting", "List the tools this project allows without prompting", "列出当前项目中无需询问即可运行的工具"),
    ("cli.help.ForgetEverySavedRuleForThisProject", "Forget every saved rule for this project", "清除当前项目保存的全部规则"),
    ("cli.help.ReportTheDecisionThisProjectResolvesTo", "Report the decision this project resolves to", "显示当前项目最终采用的信任决定"),
    ("cli.help.PrintTheDecisionAsJson", "Print the decision as JSON", "以 JSON 格式输出信任决定"),
    ("cli.help.SaveADecisionForThisProjectTheWayTheTerminalSTrustDoes", "Save a decision for this project, the way the terminal's `/trust` does", "保存项目的信任决定，与终端的 `/trust` 一致"),
    ("cli.help.TrustedOrUntrusted", "`trusted` or `untrusted`", "`trusted` 或 `untrusted`"),
    ("cli.help.PrintOneFileAsARunSBaselineRecordedIt", "Print one file as a run's baseline recorded it", "按任务基线中的记录输出一个文件"),
    ("cli.help.PathRelativeToTheProject", "Path relative to the project", "相对于项目的路径"),
    ("cli.help.TheRevisionTheRunStartedFromAsAgentEndReportedIt", "The revision the run started from, as `agent_end` reported it", "任务开始时的版本，由 `agent_end` 报告"),
    ("cli.help.PutAProjectBackToTheStateARunStartedFrom", "Put a project back to the state a run started from", "将项目恢复到任务开始时的状态"),
    ("cli.help.TheRevisionTheRunLeftAsAgentEndReportedItTheWorkTreeMustStillHoldItOrAChangeMadeSinceWouldGoToo", "The revision the run left, as `agent_end` reported it: the work tree must still hold it, or a change made since would go too", "任务结束时的版本，由 `agent_end` 报告；工作树必须仍处于此状态，以免覆盖此后所做的更改"),
    ("cli.help.ListSavedSessions", "List saved sessions", "列出保存的会话"),
    ("cli.help.IncludeSessionsFromEveryProject", "Include sessions from every project", "包含所有项目的会话"),
    ("cli.help.OnlyShowSessionsOlderThanThisManyDays", "Only show sessions older than this many days", "仅显示早于指定天数的会话"),
    ("cli.help.ShowArchivedSessionsInsteadOfActiveSessions", "Show archived sessions instead of active sessions", "显示已归档的会话"),
    ("cli.help.PrintSessionMetadataAsJson", "Print session metadata as JSON", "以 JSON 格式输出会话元数据"),
    ("cli.help.PinASavedSessionInDesktopAndTheExtension", "Pin a saved session in Desktop and the extension", "在桌面端和扩展中置顶保存的会话"),
    ("cli.help.UnpinASavedSession", "Unpin a saved session", "取消置顶保存的会话"),
    ("cli.help.HideASavedSessionWithoutDeletingItsLog", "Hide a saved session without deleting its log", "隐藏保存的会话，保留其记录"),
    ("cli.help.RestoreAnArchivedSession", "Restore an archived session", "恢复已归档的会话"),
    ("cli.help.PrintOneSavedSessionSConversation", "Print one saved session's conversation", "输出保存的会话内容"),
    ("cli.help.SessionIdOrPath", "Session id or path", "会话 ID 或路径"),
    ("cli.help.OnlyTheNewestNMessages", "Only the newest N messages", "仅输出最近 N 条消息"),
    ("cli.help.PrintTheSessionAsJsonForAClientThatDrawsIt", "Print the session as JSON, for a client that draws it", "以 JSON 格式输出会话，供客户端展示"),
    ("cli.help.DeleteSavedSessions", "Delete saved sessions", "删除保存的会话"),
    ("cli.help.SessionIdToDelete", "Session id to delete", "要删除的会话 ID"),
    ("cli.help.DeleteEverySessionForThisProject", "Delete every session for this project", "删除当前项目的全部会话"),
    ("cli.help.DeleteSessionsOlderThanThisManyDays", "Delete sessions older than this many days", "删除早于指定天数的会话"),
    ("cli.help.CompactASavedSessionIntoASummaryPlusItsMostRecentMessages", "Compact a saved session into a summary plus its most recent messages", "将保存的会话压缩为摘要并保留最近的消息"),
    ("cli.help.SessionIdOrOmitWithAll", "Session id, or omit with --all", "会话 ID，使用 --all 时可省略"),
    ("cli.help.CompactEverySessionForThisProject", "Compact every session for this project", "压缩当前项目的全部会话"),
    ("cli.help.MergeTwoSavedSessionsIntoANewSession", "Merge two saved sessions into a new session", "将两个保存的会话合并为新会话"),
    ("cli.help.FirstSessionIdOrPath", "First session id or path", "第一个会话的 ID 或路径"),
    ("cli.help.SecondSessionIdOrPath", "Second session id or path", "第二个会话的 ID 或路径"),
    ("cli.help.SummarizeTheSecondSessionInsteadOfConcatenatingItVerbatim", "Summarize the second session instead of concatenating it verbatim", "总结第二个会话，而非直接拼接原文"),
    ("cli.help.ListInstalledPlugins", "List installed plugins", "列出已安装的插件"),
    ("cli.help.InstallAPluginFromAConfiguredMarketplace", "Install a plugin from a configured marketplace", "从已配置的市场安装插件"),
    ("cli.help.PluginNameOptionallyNameMarketplace", "Plugin name, optionally `name@marketplace`", "插件名称，可使用 `name@marketplace`"),
    ("cli.help.UninstallAPlugin", "Uninstall a plugin", "卸载插件"),
    ("cli.help.EnableADisabledPlugin", "Enable a disabled plugin", "启用已禁用的插件"),
    ("cli.help.DisableAPlugin", "Disable a plugin", "禁用插件"),
    ("cli.help.ManagePluginMarketplaces", "Manage plugin marketplaces", "管理插件市场"),
    ("cli.help.AddAMarketplaceFromAGitUrlOrLocalPath", "Add a marketplace from a git URL or local path", "通过 git URL 或本地路径添加市场"),
    ("cli.help.GitUrlOrLocalPathContainingClaudePluginMarketplaceJson", "Git URL or local path containing .claude-plugin/marketplace.json", "包含 .claude-plugin/marketplace.json 的 git URL 或本地路径"),
    ("cli.help.ListConfiguredMarketplaces", "List configured marketplaces", "列出已配置的市场"),
    ("cli.help.FetchTheLatestManifestFromAMarketplaceSGitRemote", "Fetch the latest manifest from a marketplace's git remote", "从市场的 git 远程仓库获取最新清单"),
    ("cli.help.MarketplaceName", "Marketplace name", "市场名称"),
    ("cli.help.RemoveAMarketplace", "Remove a marketplace", "移除市场"),
    ("cli.help.ListConfiguredMcpServers", "List configured MCP servers", "列出已配置的 MCP 服务器"),
    ("cli.help.ShowAServerSConfiguration", "Show a server's configuration", "显示服务器配置"),
    ("cli.help.ServerName", "Server name", "服务器名称"),
    ("cli.help.AddAnMcpServer", "Add an MCP server", "添加 MCP 服务器"),
    ("cli.help.CommandAndArgumentsStdioOrUrlHttp", "Command and arguments (stdio) or URL (http)", "命令及参数（stdio）或 URL（http）"),
    ("cli.help.TransportStdioDefaultOrHttp", "Transport: stdio (default) or http", "传输方式：stdio（默认）或 http"),
    ("cli.help.EnvironmentVariableKeyValueRepeatableStdio", "Environment variable KEY=VALUE (repeatable, stdio)", "环境变量 KEY=VALUE（可重复，stdio）"),
    ("cli.help.HttpHeaderKeyValueRepeatableHttp", "HTTP header KEY=VALUE (repeatable, http)", "HTTP 请求头 KEY=VALUE（可重复，http）"),
    ("cli.help.RoutingDomainsOwnedByTheServerRepeatableOrCommaSeparated", "Routing domains owned by the server (repeatable or comma-separated)", "服务器管理的路由域（可重复或用逗号分隔）"),
    ("cli.help.WorkingDirectoryForAStdioServer", "Working directory for a stdio server", "stdio 服务器的工作目录"),
    ("cli.help.OauthClientIdRemoteServer", "OAuth client ID (remote server)", "OAuth 客户端 ID（远程服务器）"),
    ("cli.help.OauthClientSecretRemoteServer", "OAuth client secret (remote server)", "OAuth 客户端密钥（远程服务器）"),
    ("cli.help.OauthLoopbackCallbackPortRemoteServer", "OAuth loopback callback port (remote server)", "OAuth 本地回调端口（远程服务器）"),
    ("cli.help.OauthScopeRepeatableRemoteServer", "OAuth scope (repeatable, remote server)", "OAuth 作用域（可重复，远程服务器）"),
    ("cli.help.OauthRedirectUriOverrideRemoteServer", "OAuth redirect URI override (remote server)", "覆盖 OAuth 重定向 URI（远程服务器）"),
    ("cli.help.WhereToStoreTheServerProjectDefaultOrGlobal", "Where to store the server: project (default) or global", "服务器配置的存储位置：project（默认）或 global"),
    ("cli.help.AddAServerFromAJsonObject", "Add a server from a JSON object", "通过 JSON 对象添加服务器"),
    ("cli.help.JsonObjectWithCommandStdioOrUrlHttp", "JSON object with `command` (stdio) or `url` (http)", "包含 `command`（stdio）或 `url`（http）的 JSON 对象"),
    ("cli.help.RemoveAnMcpServer", "Remove an MCP server", "移除 MCP 服务器"),
    ("cli.help.LimitTheRemovalToProjectOrGlobalDefaultAnySource", "Limit the removal to project or global (default: any source)", "仅从 project 或 global 移除（默认：任意来源）"),
    ("cli.help.AuthorizeAnOauthProtectedRemoteServer", "Authorize an OAuth-protected remote server", "授权受 OAuth 保护的远程服务器"),
    ("cli.help.LimitTheLookupToProjectOrGlobalDefaultAnySource", "Limit the lookup to project or global (default: any source)", "仅从 project 或 global 查找（默认：任意来源）"),
    ("cli.help.TurnAServerOffWithoutRemovingItsConfiguration", "Turn a server off without removing its configuration", "禁用服务器，保留其配置"),
    ("cli.help.LimitTheChangeToProjectOrGlobalDefaultTheSourceThatDefinesIt", "Limit the change to project or global (default: the source that defines it)", "仅修改 project 或 global（默认：定义该服务器的来源）"),
    ("cli.help.TurnADisabledServerBackOn", "Turn a disabled server back on", "重新启用已禁用的服务器"),
    ("cli.help.generated0", "A native Rust AI coding agent CLI", "原生 Rust AI 编程智能体 CLI"),
    ("cli.help.generated1", "Print help", "显示帮助"),
    ("cli.help.generated2", "Print help (see more with '--help')", "显示帮助（使用 --help 查看更多）"),
    ("cli.help.generated3", "Print help (see a summary with '-h')", "显示帮助（使用 -h 查看摘要）"),
    ("cli.help.generated4", "Print version", "显示版本"),
    ("cli.help.generated5", "Print this message or the help of the given subcommand(s)", "显示此消息或指定子命令的帮助"),
    ("cli.help.generated6", "Print this message or the help of the given subcommand(s).", "显示此消息或指定子命令的帮助"),
    ("cli.help.generated7", "Print help information", "显示帮助信息"),
    ("extension.newWorktree", "New worktree", "新建工作树"),
    ("extension.worktreeNameHint", "Name this isolated checkout. Leave blank to generate a name.", "为此独立检出目录命名，留空则自动生成名称。"),
    ("extension.worktreeMode", "Worktree mode", "工作树模式"),
    ("extension.localChanges", "Local changes", "本地更改"),
    ("extension.attachFiles", "Oxide: attach files", "Oxide：添加文件"),
    ("extension.composer", "Composer", "消息输入框"),
    ("extension.shortcutsNote", "Enter: send · Shift+Enter: newline · Cmd/Ctrl+Shift+Enter: alternate Queue/Steer mode · Escape: close a dialog or stop", "Enter：发送 · Shift+Enter：换行 · Cmd/Ctrl+Shift+Enter：临时切换排队/引导模式 · Escape：关闭对话框或停止"),
    ("extension.detachedHead", "Detached HEAD", "分离 HEAD"),
    ("extension.newWorktreeBranch", "New oxide/<name> branch", "新建 oxide/<name> 分支"),
    ("extension.committedFiles", "Start from committed files", "从已提交文件开始"),
    ("extension.includeChanges", "Include uncommitted changes", "包含未提交的更改"),
    ("extension.attachmentSizeLimit", "{name} is {size}; the attachment limit is {limit}.", "{name} 的大小为 {size}，附件限制为 {limit}。"),
    ("extension.panelHelp", "Panel commands: /model /reasoning /agent /trust (the footer's chips), /connect, /mcp, /session, /new, /attach, /usage. A project's commands and skills are completed from `/` and run by the CLI — pick a skill to load its instructions.", "面板命令：/model /reasoning /agent /trust（底部的设置项）、/connect、/mcp、/session、/new、/attach、/usage。项目命令和技能可通过 `/` 补全，由 CLI 运行；选择技能以加载其指令。"),
    ("extension.restartWindow", "Restart Window", "重启窗口"),
    ("extension.later", "Later", "稍后"),
    ("extension.selectionExample", "e.g. why does this retry loop spin?", "例如：为什么这个重试循环一直运行？"),
    ("terminal.removeMarketplace", "Remove marketplace `{name}`?", "移除市场 `{name}`？"),
    ("terminal.removeMarketplacePlugin", "This also uninstalls {count} installed plugin.", "这也将卸载 {count} 个已安装插件。"),
    ("terminal.removeMarketplacePlugins", "This also uninstalls {count} installed plugins.", "这也将卸载 {count} 个已安装插件。"),
    ("terminal.compactedMessages", "Compacted {count} messages (~{tokens} tokens)", "已压缩 {count} 条消息（约 {tokens} 个 token）"),
    ("terminal.expandBranchSummary", "Branch summary · Ctrl+O to expand", "分支摘要 · Ctrl+O 展开"),
    ("terminal.tookDuration", "Took {duration}", "耗时 {duration}"),
    ("cli.help.layout", "{about-with-newline}\nUsage: {usage}\n\n{all-args}{after-help}", "{about-with-newline}\n用法：{usage}\n\n{all-args}{after-help}"),
    ("cli.help.commandHeading", "Commands", "命令"),
    ("cli.help.argumentHeading", "Arguments", "参数"),
    ("cli.help.optionHeading", "Options", "选项"),
    ("terminal.message.CodeResearchAutomateAndMore", "Code, research, automate, and more.", "编程、研究、自动化，尽在于此。"),
    ("terminal.message.ReasoningAutoProviderNative", "reasoning: auto (provider native)", "推理：自动（提供商默认）"),
    ("terminal.message.ThinkingBlocksVisible", "thinking blocks: visible", "思考过程：显示"),
    ("terminal.message.ThinkingBlocksHidden", "thinking blocks: hidden", "思考过程：隐藏"),
    ("terminal.message.SnapshotsUnavailableThisDirectoryIsNotSnapshotted", "snapshots unavailable (this directory is not snapshotted)", "快照不可用（此目录未建立快照）"),
    ("terminal.message.CompactNothingToCompactYet", "compact: nothing to compact yet", "压缩：暂无可压缩的内容"),
    ("terminal.message.NoProviderConnectedRunConnectToAddOne", "no provider connected — run /connect to add one", "尚未连接提供商，请运行 /connect 添加"),
    ("terminal.message.NoProviderConnectedRunConnectToAddAnApiKey", "no provider connected — run /connect to add an API key", "尚未连接提供商，请运行 /connect 添加 API 密钥"),
    ("terminal.message.NewSessionTheNextMessageStartsIt", "new session — the next message starts it", "新会话将在下一条消息发送时开始"),
    ("terminal.message.NoSessionsForThisProjectYet", "no sessions for this project yet", "此项目尚无会话"),
    ("terminal.message.NoSavedDecision", "no saved decision", "没有已保存的决定"),
    ("terminal.message.SavedTrustDecisionTrusted", "saved trust decision: trusted", "已保存信任决定：信任"),
    ("terminal.message.SavedTrustDecisionDeclined", "saved trust decision: declined", "已保存信任决定：不信任"),
    ("terminal.message.ReloadedConfigCommandsAndSkills", "reloaded config, commands and skills", "已重新加载配置、命令和技能"),
    ("terminal.message.SubtaskCommandRequiresAnAgentInItsFrontmatter", "subtask command requires an `agent` in its frontmatter", "子任务命令需要在前置元数据中指定 `agent`"),
    ("terminal.message.ClearedThisProjectSApprovals", "cleared this project's approvals", "已清除此项目的批准记录"),
    ("terminal.message.PortkeyUsageKeyClearedUsingTheProviderCredential", "portkey usage key cleared; using the provider credential", "已清除 Portkey 用量密钥，将使用提供商凭据"),
    ("terminal.message.NeedsAPortkeyLoginRunConnectPortkey", "needs a Portkey login — run /connect portkey", "需要登录 Portkey，请运行 /connect portkey"),
    ("terminal.message.EnterOneWordEGFirstnameLastname", "enter one word, e.g. firstname.lastname", "请输入一个不含空格的名称，例如 firstname.lastname"),
    ("terminal.message.BudgetMustBeAPositiveNumber", "budget must be a positive number", "预算必须为正数"),
    ("terminal.message.NoSessionToBranchSendAMessageFirst", "no session to branch — send a message first", "没有可分支的会话，请先发送消息"),
    ("terminal.message.EnterAnApiKey", "enter an API key", "请输入 API 密钥"),
    ("terminal.message.ReasoningLevel", "reasoning: {level}", "推理：{level}"),
    ("terminal.message.ContextFilesFiles", "context files: {files}", "上下文文件：{files}"),
    ("terminal.message.CopiedCountChars", "copied {count} chars", "已复制 {count} 个字符"),
    ("terminal.noMcpServers", "no MCP servers configured", "尚未配置 MCP 服务器"),
    ("terminal.authorizeMcp", "run `oxide mcp auth {name}` to authorize", "运行 `oxide mcp auth {name}` 进行授权"),
    ("terminal.mcpServerCount", "MCP servers ({count})", "MCP 服务器（{count}）"),
    ("approval.listDirectory", "List a directory", "列出目录"),
    ("approval.findFiles", "Find files", "查找文件"),
    ("approval.searchWorkspace", "Search the workspace", "搜索工作区"),
    ("approval.fetchUrl", "Fetch a URL", "获取 URL 内容"),
    ("approval.runSubagent", "Run a subagent", "运行子智能体"),
    ("approval.remoteTool", "Call {tool} on {server}", "调用 {server} 的 {tool}"),
    ("approval.namedTool", "Run {tool}", "运行 {tool}"),
    ("approval.result.once", "Allowed once", "已允许一次"),
    ("approval.result.always", "Always allowed in this project", "已在此项目中始终允许"),
    ("approval.result.deny", "Denied", "已拒绝"),
    ("approval.result.closed", "Not answered", "未答复"),
    ("question.result.dismissed", "Dismissed without an answer", "已关闭，未作答"),
    ("question.result.closed", "Not answered", "未答复"),
    ("extension.compactionNotice", "Compacted {count} earlier messages (~{tokens} tokens)", "已压缩前面的 {count} 条消息（约 {tokens} 个 token）"),
    ("ecosystem.resourceCounts", "{agents} agents · {commands} commands · {skills} skills · {servers} MCP servers · {hooks} hooks · {plugins} plugins", "{agents} 个智能体 · {commands} 个命令 · {skills} 个技能 · {servers} 个 MCP 服务器 · {hooks} 个钩子 · {plugins} 个插件"),
    ("terminal.startupResources", "mcp: {configured} configured · {loaded} loaded · {tools} tools · hooks: {hooks}{unavailable} · memory: {memory} entries", "MCP：已配置 {configured} 个 · 已加载 {loaded} 个 · {tools} 个工具 · 钩子：{hooks}{unavailable} · 记忆：{memory} 条"),
    ("terminal.unavailableSuffix", " (unavailable)", "（不可用）"),
    ("terminal.clipboardEmpty", "nothing to attach from the clipboard", "剪贴板中没有可添加的附件"),
    ("terminal.clipboardUnsupported", "nothing to attach from the clipboard (it holds {types})", "剪贴板中没有可添加的附件（包含 {types}）"),
    ("session.deleted", "deleted {id}", "已删除 {id}"),
    ("session.archivedNotice", "Session archived. Its log is kept. To delete it permanently, run `oxide sessions delete {id}` from this project. To restore it, run `oxide sessions unarchive {id}`.", "会话已归档，记录已保留。要永久删除，请在此项目中运行 `oxide sessions delete {id}`；要恢复，请运行 `oxide sessions unarchive {id}`。"),
    ("session.noProjectSessions", "no sessions for this project", "此项目尚无会话"),
    ("session.noCompactionNeeded", "no sessions needed compaction", "没有需要压缩的会话"),
    ("session.compacted", "compacted {id}", "已压缩 {id}"),
    ("session.skippedCompaction", "skipped {id} (already compact)", "已跳过 {id}（已压缩）"),
    ("session.compactionError", "error compacting {id}: {error}", "压缩 {id} 时出错：{error}"),
    ("session.merged", "merged {a} + {b} into {id}", "已将 {a} + {b} 合并为 {id}"),
    ("session.messageUnit", "msg", "条消息"),
];

pub fn relative_time(language: Language, now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    let (key, count) = if seconds < 60 {
        return t(language, "ui.justNow").to_string();
    } else if seconds < 3600 {
        ("time.minutesAgo", seconds / 60)
    } else if seconds < 86400 {
        ("time.hoursAgo", seconds / 3600)
    } else {
        ("time.daysAgo", seconds / 86400)
    };
    tf(language, key, &[("count", &count.to_string())])
}

/// Looks one key up. A missing key returns an empty string so front-ends can
/// keep their authored fallback.
pub fn t(language: Language, key: &str) -> &'static str {
    let Some((_, en, zh)) = CATALOG.iter().find(|(name, _, _)| *name == key) else {
        return "";
    };
    match language {
        Language::EnGb => en,
        Language::ZhCn if !zh.is_empty() => zh,
        Language::ZhCn => en,
    }
}

/// One key with its `{name}` placeholders filled, for a sentence whose subject
/// is only known at the call.
pub fn tf(language: Language, key: &str, vars: &[(&str, &str)]) -> String {
    let mut text = t(language, key).to_string();
    for (name, value) in vars {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// The strings one language draws, as the object a front-end that cannot link
/// this crate reads out of `oxide language --json`.
pub fn catalog(language: Language) -> serde_json::Map<String, serde_json::Value> {
    CATALOG
        .iter()
        .map(|(key, en, zh)| {
            let text = match language {
                Language::ZhCn if !zh.is_empty() => zh,
                _ => en,
            };
            (key.to_string(), serde_json::Value::String(text.to_string()))
        })
        .collect()
}

/// One language as a picker row.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageView {
    pub tag: &'static str,
    pub label: &'static str,
    pub native_label: &'static str,
}

/// Every language a picker offers.
pub fn views() -> Vec<LanguageView> {
    Language::ALL
        .iter()
        .map(|language| LanguageView {
            tag: language.tag(),
            label: language.label(),
            native_label: language.native_label(),
        })
        .collect()
}

/// The answer `oxide language --json` prints and the desktop app's `language`
/// command returns: the language in use, the ones a picker offers, and the
/// strings to draw in the one in use.
pub fn describe(cwd: &Path) -> serde_json::Value {
    let language = resolve(cwd);
    serde_json::json!({
        "language": language.tag(),
        "languages": views(),
        "strings": catalog(language),
    })
}

/// The language this process runs in: `OXIDE_LANG` first, since it is the
/// override for one launch, then the project's own settings, then the global
/// ones, then the language the app is authored in.
pub fn resolve(cwd: &Path) -> Language {
    let from_env = std::env::var("OXIDE_LANG").ok();
    resolve_from(cwd, from_env.as_deref())
}

/// [`resolve`] with the environment injected, so a test can hold one value
/// without touching the process's own.
pub fn resolve_from(cwd: &Path, from_env: Option<&str>) -> Language {
    if let Some(language) = from_env.and_then(Language::parse) {
        return language;
    }
    from_settings(cwd).unwrap_or(Language::DEFAULT)
}

/// The language the settings files name, the project's winning per key.
pub fn from_settings(cwd: &Path) -> Option<Language> {
    let mut paths = Vec::new();
    if let Some(root) = crate::ecosystem::project_root(cwd) {
        paths.push(root.join(".oxide").join("settings.json"));
    }
    paths.push(crate::config::settings_path());
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(language) = value
            .get("language")
            .and_then(|value| value.as_str())
            .and_then(Language::parse)
        {
            return Some(language);
        }
    }
    None
}

/// Persists a choice into the global `settings.json`, the file every front-end
/// reads, so the terminal, the desktop app and the panel switch together.
pub fn save(language: Language) -> Result<()> {
    crate::config::save_setting_to(
        &crate::config::settings_path(),
        "language",
        serde_json::Value::String(language.tag().to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_i18n_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_the_tags_a_person_writes() {
        for tag in ["en-GB", "en_GB", "en", "EN-gb", " en-AU "] {
            assert_eq!(Language::parse(tag), Some(Language::EnGb), "{tag}");
        }
        for tag in ["zh-CN", "zh_CN", "zh", "zh-Hans", "zh-hans-cn", "ZH-tw"] {
            assert_eq!(Language::parse(tag), Some(Language::ZhCn), "{tag}");
        }
        for tag in ["", "  ", "fr", "de-DE", "jp"] {
            assert_eq!(Language::parse(tag), None, "{tag}");
        }
    }

    #[test]
    fn defaults_to_the_language_the_app_is_written_in() {
        assert_eq!(Language::default(), Language::EnGb);
        assert_eq!(Language::DEFAULT.tag(), "en-GB");
        assert_eq!(
            Language::ALL
                .iter()
                .map(|language| language.tag())
                .collect::<Vec<_>>(),
            vec!["en-GB", "zh-CN"]
        );
    }

    #[test]
    fn translates_every_key_in_both_languages() {
        for (key, en, zh) in CATALOG {
            assert!(!key.is_empty(), "a key with no name");
            assert!(!en.is_empty(), "{key} has no English");
            assert!(!zh.is_empty(), "{key} has no Chinese");
            assert_eq!(t(Language::EnGb, key), *en, "{key}");
            assert_eq!(t(Language::ZhCn, key), *zh, "{key}");
        }
        assert_eq!(t(Language::EnGb, "common.close"), "Close");
        assert_eq!(t(Language::ZhCn, "common.close"), "关闭");
        assert_ne!(
            t(Language::EnGb, "home.title"),
            t(Language::ZhCn, "home.title")
        );
    }

    #[test]
    fn translations_preserve_placeholders() {
        fn names(text: &str) -> std::collections::BTreeSet<&str> {
            text.split('{')
                .skip(1)
                .filter_map(|part| part.split_once('}').map(|(name, _)| name))
                .collect()
        }
        for (key, en, zh) in CATALOG {
            assert_eq!(names(en), names(zh), "{key}");
        }
        for command in crate::commands::BUILTINS {
            let key = format!("command.{}", command.name);
            assert_eq!(t(Language::EnGb, &key), command.description);
            assert!(!t(Language::ZhCn, &key).is_empty());
        }
    }

    #[test]
    fn names_every_key_once() {
        let mut keys: Vec<_> = CATALOG.iter().map(|(key, _, _)| key).collect();
        keys.sort_unstable();
        let total = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), total, "a key is written down twice");
    }

    #[test]
    fn fills_the_placeholders_a_sentence_carries() {
        assert_eq!(
            tf(
                Language::EnGb,
                "composer.queuedMessage",
                &[("text", "hello")]
            ),
            "Queued: hello"
        );
        assert_eq!(
            tf(
                Language::ZhCn,
                "dialog.question.step",
                &[("index", "2"), ("total", "3")]
            ),
            "第 2 个问题，共 3 个"
        );
    }

    #[test]
    fn a_key_the_catalog_does_not_hold_reads_as_nothing() {
        assert_eq!(t(Language::EnGb, "no.such.key"), "");
        assert_eq!(t(Language::ZhCn, "no.such.key"), "");
    }

    #[test]
    fn the_catalog_a_front_end_reads_carries_every_key() {
        for language in Language::ALL {
            let entries = catalog(*language);
            assert_eq!(entries.len(), CATALOG.len());
            for (key, _, _) in CATALOG {
                assert_eq!(entries[*key], t(*language, key));
            }
        }
        let catalog = catalog(Language::ZhCn);
        assert_eq!(
            catalog.get("sidebar.projects"),
            Some(&serde_json::Value::String("项目".to_string()))
        );
        let described = describe(Path::new("/nonexistent"));
        assert!(described.get("languages").is_some());
        assert_eq!(
            described["strings"].as_object().unwrap().len(),
            CATALOG.len()
        );
    }

    #[test]
    fn the_override_wins_over_the_settings_files() {
        // The environment is the process's, so the holder is taken before the
        // variable is written and kept until it is removed again.
        let _held = crate::env_lock::hold();
        let dir = scratch("resolve");
        let cwd = dir.join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let settings = dir.join("settings.json");
        std::env::set_var("OXIDE_SETTINGS_FILE", &settings);

        assert_eq!(resolve_from(&cwd, None), Language::EnGb);

        std::fs::write(&settings, r#"{"language":"zh-CN"}"#).unwrap();
        assert_eq!(resolve_from(&cwd, None), Language::ZhCn);

        // The project's own file wins over the global one, per key.
        std::fs::create_dir_all(cwd.join(".oxide")).unwrap();
        std::fs::write(
            cwd.join(".oxide").join("settings.json"),
            r#"{"language":"en-GB"}"#,
        )
        .unwrap();
        assert_eq!(resolve_from(&cwd, None), Language::EnGb);

        // The launch override outranks both, and an unreadable tag falls through.
        assert_eq!(resolve_from(&cwd, Some("zh")), Language::ZhCn);
        assert_eq!(resolve_from(&cwd, Some("fr")), Language::EnGb);

        std::env::remove_var("OXIDE_SETTINGS_FILE");
    }

    #[test]
    fn saves_the_choice_where_every_front_end_reads_it() {
        let _held = crate::env_lock::hold();
        let dir = scratch("save");
        let settings = dir.join("settings.json");
        std::env::set_var("OXIDE_SETTINGS_FILE", &settings);
        std::fs::write(&settings, r#"{"notifyOnComplete":false}"#).unwrap();

        save(Language::ZhCn).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(value["language"], "zh-CN");
        // A key the switch does not own is left exactly as it was.
        assert_eq!(value["notifyOnComplete"], false);
        assert_eq!(from_settings(&dir.join("nowhere")), Some(Language::ZhCn));

        std::env::remove_var("OXIDE_SETTINGS_FILE");
    }
}
