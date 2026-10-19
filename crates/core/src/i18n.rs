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
        "在当前项目中开始一个新线程",
    ),
    ("sidebar.projects", "Projects", "项目"),
    ("sidebar.newProject", "New project", "新建项目"),
    ("sidebar.newProjectHint", "Pick the folders to include", "选择要包含的文件夹"),
    ("sidebar.theme", "Theme", "主题"),
    ("sidebar.checkUpdates", "Check for updates", "检查更新"),
    ("sidebar.language", "Language", "语言"),
    ("sidebar.recents", "Recents", "最近"),
    ("sidebar.recentThreads", "Recent threads", "最近的线程"),
    ("sidebar.showMore", "Show more", "显示更多"),
    ("sidebar.showLess", "Show less", "收起"),
    ("sidebar.noChats", "No chats", "暂无对话"),
    ("sidebar.archive", "Archive", "归档"),
    ("sidebar.pin", "Pin", "固定"),
    ("sidebar.unpin", "Unpin", "取消固定"),
    ("sidebar.renameThread", "Rename thread", "重命名线程"),
    ("sidebar.deleteThread", "Delete thread", "删除线程"),
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
        "用下方的项目按钮选择文件夹，或打开最近的线程。",
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
        "此项目已保存的线程，最新的在前。选择其中一个即在此打开；侧边栏会在每个项目旁显示相同的列表。",
    ),
    ("dialog.sessions.new", "New thread", "新建线程"),
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
    ("dialog.rename.title", "Rename thread", "重命名线程"),
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
        "打开侧边栏中对应位置的线程",
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
];

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
        let catalog = catalog(Language::ZhCn);
        assert_eq!(catalog.len(), CATALOG.len());
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
