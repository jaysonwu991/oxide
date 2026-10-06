//! The slash commands a client offers, as one catalog for every front-end.
//!
//! Two kinds of entry end up in a client's `/` menu. *Built-ins* are draws the
//! client itself performs (its model picker, its MCP list, a new session), so
//! the catalog only names and describes them; each client implements the
//! action. *Configured* entries are the project's own commands, prompt
//! templates and skills, which a client runs by sending `/name args` as a
//! prompt — the CLI and the desktop resolve those against the same ecosystem
//! before the turn starts.
//!
//! Discovery here goes through [`crate::ecosystem`], so a client sees exactly
//! the commands the agent would resolve, including the ones contributed by
//! installed plugin packages, and a project that is not trusted contributes
//! none of its own.
//!
//! An agent is not a slash command: a client selects one through its own agent
//! picker (`/agent` in the terminal, `--agent` for a run), so agent files
//! contribute no entry here. The routing a command's frontmatter declares
//! (`agent:`, `subtask:`) is resolved by the CLI when it runs the `/name`
//! prompt, so a client that only sends the prompt needs no entry for it.

use crate::ecosystem::{self, LoadOptions};
use crate::trust;
use serde::Serialize;
use std::path::Path;

/// A client's own drawing: it runs the action rather than sending a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The client performs it (open a picker, start a session).
    Client,
    /// A configured command or prompt template: send `/name args`.
    Prompt,
    /// A configured skill: send `/name`, its own name. The terminal spells it
    /// `/skill:<name>` and the CLI resolves both.
    Skill,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Prompt => "prompt",
            Self::Skill => "skill",
        }
    }
}

/// A front-end that offers the catalog's commands.
///
/// A client draws its `/` menu from the entries it is named in, so the one table
/// says both what a command is called and which clients perform it. A command
/// left out of a client's list is one that client would otherwise answer with a
/// "not available here" note instead of an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontEnd {
    /// The terminal UI (`oxide` with no mode flag).
    Terminal,
    /// The desktop app.
    Desktop,
    /// The VS Code extension's panel.
    Panel,
}

impl FrontEnd {
    /// Every front-end, in the order a listing names them.
    pub const ALL: &'static [FrontEnd] = &[FrontEnd::Terminal, FrontEnd::Desktop, FrontEnd::Panel];

    pub fn label(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Desktop => "desktop",
            Self::Panel => "panel",
        }
    }
}

/// One built-in command. `name` is written without its slash, and is the one
/// spelling every client answers: a second spelling for the same draw is what a
/// menu has to explain and a reader has to remember, so the catalog holds a name
/// per command and nothing else.
pub struct Builtin {
    pub name: &'static str,
    pub description: &'static str,
    /// An argument hint for autocomplete and help, e.g. `on|off|list|clear`.
    pub arguments: Option<&'static str>,
    /// The clients that perform it. A command only one client draws is named for
    /// that client alone: an agent picker is VS Code's, the theme picker and
    /// provider logout are the terminal's and the desktop app's. Provider login
    /// is all three — each draws that picker over `oxide providers`.
    pub front_ends: &'static [FrontEnd],
}

impl Builtin {
    /// Whether this client performs the command, which is what decides if it is
    /// in that client's `/` menu.
    pub fn offered_to(&self, front_end: FrontEnd) -> bool {
        self.front_ends.contains(&front_end)
    }
}

/// The commands the clients offer. Order is the order they are shown in.
pub const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "help",
        description: "List the commands and keyboard shortcuts",
        arguments: None,
        front_ends: FrontEnd::ALL,
    },
    Builtin {
        name: "mcp",
        description: "List MCP servers and their connection status",
        arguments: None,
        front_ends: FrontEnd::ALL,
    },
    Builtin {
        name: "model",
        description: "Choose the model to run",
        arguments: None,
        front_ends: FrontEnd::ALL,
    },
    Builtin {
        name: "reasoning",
        description: "Set the reasoning effort for this chat",
        arguments: Some("auto|off|minimal|low|medium|high|xhigh|max"),
        front_ends: FrontEnd::ALL,
    },
    Builtin {
        name: "agent",
        description: "Choose the subagent this chat runs as",
        arguments: None,
        // The panel picks a subagent; a run's `--agent` is the CLI's own flag
        // rather than a slash command, and the desktop app has no picker.
        front_ends: &[FrontEnd::Panel],
    },
    Builtin {
        name: "permissions",
        description: "Review the tools allowed without prompting",
        arguments: Some("on|off|list|clear"),
        front_ends: &[FrontEnd::Terminal, FrontEnd::Desktop],
    },
    Builtin {
        name: "trust",
        description: "Decide whether this project's own resources load",
        arguments: None,
        front_ends: FrontEnd::ALL,
    },
    Builtin {
        name: "session",
        description: "Resume a session from this project",
        arguments: None,
        front_ends: FrontEnd::ALL,
    },
    Builtin {
        name: "new",
        description: "Start a new session",
        arguments: None,
        front_ends: FrontEnd::ALL,
    },
    Builtin {
        name: "usage",
        description: "Show this chat's tokens, cost and context",
        arguments: None,
        front_ends: FrontEnd::ALL,
    },
    Builtin {
        name: "attach",
        description: "Attach images, PDFs or files",
        arguments: None,
        front_ends: FrontEnd::ALL,
    },
    Builtin {
        name: "theme",
        description: "Choose the color theme",
        arguments: None,
        front_ends: &[FrontEnd::Terminal, FrontEnd::Desktop],
    },
    Builtin {
        name: "connect",
        description: "Sign in to a provider",
        arguments: Some("provider"),
        front_ends: FrontEnd::ALL,
    },
    Builtin {
        name: "logout",
        description: "Forget a provider's stored credentials",
        arguments: Some("provider"),
        front_ends: &[FrontEnd::Terminal, FrontEnd::Desktop],
    },
];

/// One row of a client's `/` menu, whether it draws it itself or sends it.
#[derive(Debug, Clone, Serialize)]
pub struct CommandEntry {
    pub name: String,
    pub description: String,
    /// An argument hint, or `None` when the command takes none. A client uses
    /// it to decide between running a command and completing it for editing.
    pub arguments: Option<String>,
    /// `client`, `prompt`, or `skill`.
    pub kind: String,
    /// The front-ends that offer it: `terminal`, `desktop` and/or `panel`. A
    /// client shows an entry it is named in and leaves the others out of its
    /// menu, so a command one front-end performs is not offered by another as
    /// something it cannot do.
    pub front_ends: Vec<&'static str>,
    /// The same fact for a client that predates `front_ends`: whether the VS Code
    /// panel is *not* one of the front-ends that perform it. A client that reads
    /// only this hides such a row from its menu, which is what it did with the
    /// field when it meant "the desktop app's own command".
    pub desktop_only: bool,
    /// `builtin` for a client's own command, otherwise the scope the entry was
    /// discovered in (`project` or `global`).
    pub source: String,
}

/// Whether the panel is not one of the front-ends a command is offered to, which
/// is what a client too old to read `front_ends` reads instead.
fn desktop_only(front_ends: &[&'static str]) -> bool {
    !front_ends.contains(&FrontEnd::Panel.label())
}

/// The front-ends a configured entry is offered to: a command, prompt template
/// or skill is sent as a prompt, which every client can do.
fn every_front_end() -> Vec<&'static str> {
    FrontEnd::ALL
        .iter()
        .map(|front_end| front_end.label())
        .collect()
}

impl CommandEntry {
    fn builtin(command: &Builtin) -> Self {
        Self {
            name: command.name.to_string(),
            description: command.description.to_string(),
            arguments: command.arguments.map(str::to_string),
            kind: Kind::Client.label().to_string(),
            front_ends: command
                .front_ends
                .iter()
                .map(|front_end| front_end.label())
                .collect(),
            desktop_only: desktop_only(
                &command
                    .front_ends
                    .iter()
                    .map(|front_end| front_end.label())
                    .collect::<Vec<_>>(),
            ),
            source: "builtin".to_string(),
        }
    }
}

/// The built-in command a name refers to, matched without its slash and without
/// regard to case.
pub fn builtin(name: &str) -> Option<&'static Builtin> {
    let name = name.trim().trim_start_matches('/').to_ascii_lowercase();
    BUILTINS.iter().find(|command| command.name == name)
}

/// The built-ins alone: what a client offers when there is no project to read the
/// commands, prompt templates and skills of — the rows a client performs itself
/// need no folder, so a `/` menu asked for before one is chosen still has them.
pub fn builtin_entries() -> Vec<CommandEntry> {
    BUILTINS.iter().map(CommandEntry::builtin).collect()
}

/// The built-ins one front-end performs, in the catalog's own order: the rows of
/// its `/` menu, so a client names and describes them from here rather than from
/// a list of its own that drifts.
pub fn builtin_entries_for(front_end: FrontEnd) -> Vec<CommandEntry> {
    BUILTINS
        .iter()
        .filter(|command| command.offered_to(front_end))
        .map(CommandEntry::builtin)
        .collect()
}

/// The input with its command rewritten to the catalog's own spelling, so a name
/// typed in another case reaches the same arm. Only the first word is touched,
/// and the rest of the line is left as typed.
///
/// Anything the catalog does not know comes back unchanged: a command that is
/// the front-end's own, a configured command, prompt template or skill, and an
/// ordinary message that happens to start with a slash.
pub fn canonical_slash(raw: &str) -> String {
    let Some(command) = raw.trim_start().strip_prefix('/') else {
        return raw.to_string();
    };
    let end = command.find(char::is_whitespace).unwrap_or(command.len());
    let (word, rest) = command.split_at(end);
    match builtin(word) {
        Some(builtin) => format!("/{}{rest}", builtin.name),
        None => raw.to_string(),
    }
}

/// Every entry a client shows for `cwd`: the built-ins, then the commands,
/// prompt templates and skills visible from the project.
pub fn palette(cwd: &Path) -> Vec<CommandEntry> {
    palette_with(cwd, trust::project_trusted(cwd))
}

/// [`palette`] with the trust decision already resolved, so a client that asked
/// the user itself (the desktop app) reports the same set.
pub fn palette_with(cwd: &Path, project_trusted: bool) -> Vec<CommandEntry> {
    let options = LoadOptions {
        context_files: false,
        project_resources: project_trusted,
    };
    let project = ecosystem::load_opts(cwd, options);
    let global = ecosystem::load_opts(
        cwd,
        LoadOptions {
            project_resources: false,
            ..options
        },
    );

    let mut entries: Vec<CommandEntry> = builtin_entries();
    // The project load already merges the global scope, so the global listing
    // is what is compared against to tell where an entry came from.
    let global = configured(&global);

    for mut entry in configured(&project) {
        // A configured entry that matches a built-in is unreachable — the
        // client draws its own command first — so it is dropped rather than
        // listed twice.
        if builtin(&entry.name).is_some() {
            continue;
        }
        entry.source = scope_of(&entry, &global).to_string();
        entries.push(entry);
    }
    entries
}

/// The scope a configured entry was found in. The project listing contains the
/// global one, so an entry of the same name is only the global scope's when it
/// is *identical* to what the global scope defines: a project that overrides a
/// global command of the same name contributes its own definition, and the
/// name alone would mislabel it as global.
fn scope_of(entry: &CommandEntry, global: &[CommandEntry]) -> &'static str {
    let from_global = global
        .iter()
        .find(|other| other.name == entry.name)
        .is_some_and(|other| same_definition(other, entry));
    if from_global {
        "global"
    } else {
        "project"
    }
}

/// Whether two entries describe the same command, so a project's own version of
/// a name can be told apart from the global one it shadows.
fn same_definition(left: &CommandEntry, right: &CommandEntry) -> bool {
    left.name == right.name
        && left.description == right.description
        && left.arguments == right.arguments
        && left.kind == right.kind
}

/// The commands, prompt templates and skills of one loaded ecosystem, as menu
/// entries. The scope they came from is filled in by the caller. Agents are not
/// entries: they are chosen with a client's own picker rather than typed as a
/// slash command.
fn configured(ecosystem: &ecosystem::Ecosystem) -> Vec<CommandEntry> {
    let mut entries = Vec::new();
    for command in &ecosystem.commands {
        entries.push(CommandEntry {
            name: command.name.clone(),
            description: command
                .description
                .clone()
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| summarize(&command.template)),
            arguments: takes_arguments(&command.template).then(|| "arguments".to_string()),
            kind: Kind::Prompt.label().to_string(),
            front_ends: every_front_end(),
            desktop_only: false,
            source: String::new(),
        });
    }
    for template in &ecosystem.prompt_templates {
        entries.push(CommandEntry {
            name: template.name.clone(),
            description: template
                .description
                .clone()
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| summarize(&template.body)),
            arguments: template
                .argument_hint
                .clone()
                .or_else(|| takes_arguments(&template.body).then(|| "arguments".to_string())),
            kind: Kind::Prompt.label().to_string(),
            front_ends: every_front_end(),
            desktop_only: false,
            source: String::new(),
        });
    }
    for skill in &ecosystem.skills {
        // A skill is a slash command of its own: `/name` is the spelling a
        // client's menu lists it under, the way Claude Code lists one. The
        // terminal's `/skill:<name>` is the same skill and resolves too. A
        // command or template of the same name is what `/name` runs, so the
        // skill is not listed under a spelling that would run something else.
        if entries.iter().any(|entry| entry.name == skill.name) {
            continue;
        }
        entries.push(CommandEntry {
            name: skill.name.clone(),
            description: skill
                .description
                .clone()
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| summarize(&skill.content)),
            // A skill takes a free-form argument, which the CLI appends to the
            // loaded instructions, so a client completes it for editing rather
            // than running it bare.
            arguments: Some("[arguments]".to_string()),
            kind: Kind::Skill.label().to_string(),
            front_ends: every_front_end(),
            desktop_only: false,
            source: String::new(),
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries.dedup_by(|left, right| left.name == right.name);
    entries
}

/// Whether a template body consumes arguments, which decides whether a client
/// runs the command or completes it so the user can type them.
fn takes_arguments(body: &str) -> bool {
    body.contains("$ARGUMENTS")
        || (1..=9).any(|index| body.contains(&format!("${index}")))
        || body.contains("${")
}

/// The listing a client shows, or that a terminal user reads: `oxide commands`.
pub fn list(cwd: &Path, json: bool) -> anyhow::Result<()> {
    let entries = palette(cwd);
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    // One column of names, aligned so the descriptions line up, with the scope
    // of a configured command spelled out because that decides whether it
    // loads in a project the user has not trusted.
    let labels: Vec<String> = entries.iter().map(label).collect();
    let width = labels
        .iter()
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(0);
    println!("Commands ({}):", entries.len());
    for (label, entry) in labels.iter().zip(&entries) {
        let scope = match entry.source.as_str() {
            "builtin" => String::new(),
            source => format!("  [{source}]"),
        };
        println!("  {label:width$}  {}{scope}", entry.description);
    }
    Ok(())
}

/// `/name` and its argument hint: `/reasoning auto|off|minimal|low|medium|high|xhigh|max`.
fn label(entry: &CommandEntry) -> String {
    let mut label = format!("/{}", entry.name);
    if let Some(arguments) = &entry.arguments {
        label.push(' ');
        label.push_str(arguments);
    }
    label
}

/// A one-line description for an entry that has none: its first non-empty line
/// with Markdown markers stripped, so the menu stays readable.
fn summarize(body: &str) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .trim_start_matches('#')
        .trim();
    if line.chars().count() <= 80 {
        return line.to_string();
    }
    let mut text: String = line.chars().take(79).collect();
    text.push('…');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_commands_{tag}_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        dir
    }

    fn names(entries: &[CommandEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.name.as_str()).collect()
    }

    #[test]
    fn builtins_resolve_by_name() {
        assert_eq!(builtin("mcp").unwrap().name, "mcp");
        assert_eq!(builtin("/mcp").unwrap().name, "mcp");
        assert_eq!(builtin("MCP").unwrap().name, "mcp");
        assert!(builtin("nope").is_none());
        // A command has one spelling: a second name for the same draw is not
        // something a client answers, nor the catalog resolves.
        for command in BUILTINS {
            assert_eq!(builtin(command.name).unwrap().name, command.name);
        }
        for retired in ["mcps", "approvals", "access", "thinking", "sessions"] {
            assert!(builtin(retired).is_none(), "{retired}");
        }
    }

    #[test]
    fn a_slash_command_is_resolved_to_the_catalogs_spelling() {
        assert_eq!(canonical_slash("/MCP"), "/mcp");
        assert_eq!(canonical_slash("/mcp"), "/mcp");
        assert_eq!(canonical_slash("/Reasoning high"), "/reasoning high");
        // The rest of the line is what the user typed, and a spelling the
        // catalog does not know — the terminal's own command, a configured one,
        // a skill, an ordinary message — is passed through untouched.
        assert_eq!(canonical_slash("/openai"), "/openai");
        assert_eq!(canonical_slash("/hotkeys"), "/hotkeys");
        assert_eq!(canonical_slash("/ship the branch"), "/ship the branch");
        assert_eq!(canonical_slash("what is /mcp?"), "what is /mcp?");
    }

    #[test]
    fn each_front_end_is_offered_the_commands_it_performs() {
        let owns = |front_end, name: &str| {
            builtin_entries_for(front_end)
                .iter()
                .any(|entry| entry.name == name)
        };
        assert!(owns(FrontEnd::Terminal, "theme"));
        assert!(owns(FrontEnd::Terminal, "permissions"));
        assert!(!owns(FrontEnd::Terminal, "agent"));
        assert!(owns(FrontEnd::Desktop, "theme"));
        assert!(!owns(FrontEnd::Desktop, "agent"));
        assert!(!owns(FrontEnd::Panel, "theme"));
        assert!(!owns(FrontEnd::Panel, "logout"));
        assert!(owns(FrontEnd::Panel, "agent"));
        // Nothing is offered to a front-end that does not offer it.
        for command in BUILTINS {
            for front_end in FrontEnd::ALL {
                assert_eq!(
                    owns(*front_end, command.name),
                    command.offered_to(*front_end),
                    "{} for {}",
                    command.name,
                    front_end.label()
                );
            }
        }
        assert!(builtin_entries_for(FrontEnd::Terminal)
            .iter()
            .all(|entry| entry.front_ends.contains(&FrontEnd::Terminal.label())));
    }

    #[test]
    fn builtins_agree_on_names_and_descriptions() {
        for command in BUILTINS {
            assert!(!command.name.is_empty());
            assert!(!command.description.is_empty());
            assert!(!command.name.contains('/'));
        }
    }

    #[test]
    fn the_palette_starts_with_the_builtins() {
        let dir = temp_dir("builtins");
        let entries = palette_with(&dir, true);
        // The machine running the test may have commands of its own in the
        // global scope, so only the prefix is asserted.
        assert_eq!(
            names(&entries[..BUILTINS.len()]),
            BUILTINS
                .iter()
                .map(|command| command.name)
                .collect::<Vec<_>>()
        );
        assert!(entries[..BUILTINS.len()]
            .iter()
            .all(|entry| entry.source == "builtin" && entry.kind == "client"));
        assert!(entries
            .iter()
            .any(|entry| entry.name == "theme" && entry.front_ends == ["terminal", "desktop"]));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_builtins_alone_are_the_rows_a_client_performs_itself() {
        let entries = builtin_entries();
        assert_eq!(
            names(&entries),
            BUILTINS
                .iter()
                .map(|command| command.name)
                .collect::<Vec<_>>()
        );
        assert!(entries
            .iter()
            .all(|entry| entry.kind == "client" && entry.source == "builtin"));
        // A project contributes nothing here: a command, a prompt template and a
        // skill are what a folder answers for, and none of them is in this list.
        let dir = temp_dir("builtins_alone");
        std::fs::create_dir_all(dir.join(".oxide").join("commands")).unwrap();
        std::fs::write(
            dir.join(".oxide").join("commands").join("ship.md"),
            "Open a pull request\n",
        )
        .unwrap();
        let still = builtin_entries();
        assert!(!still.iter().any(|entry| entry.name == "ship"));
        assert_eq!(still.len(), entries.len());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_projects_commands_templates_and_skills_are_listed() {
        let dir = temp_dir("configured");
        std::fs::create_dir_all(dir.join(".oxide").join("commands")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide").join("prompts")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide").join("skills").join("tdd")).unwrap();
        std::fs::write(
            dir.join(".oxide").join("commands").join("ship.md"),
            "---\ndescription: Ship the current branch\n---\nOpen a pull request for $ARGUMENTS\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(".oxide").join("prompts").join("summarize.md"),
            "---\ndescription: Summarize a file\nargument-hint: path\n---\nSummarize $1.\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(".oxide")
                .join("skills")
                .join("tdd")
                .join("SKILL.md"),
            "---\ndescription: Write the test first\n---\nAlways write a failing test first.\n",
        )
        .unwrap();

        let entries = palette_with(&dir, true);
        let ship = entries.iter().find(|entry| entry.name == "ship").unwrap();
        assert_eq!(ship.description, "Ship the current branch");
        assert_eq!(ship.arguments.as_deref(), Some("arguments"));
        assert_eq!(ship.kind, "prompt");
        assert_eq!(ship.source, "project");

        let summarize = entries
            .iter()
            .find(|entry| entry.name == "summarize")
            .unwrap();
        assert_eq!(summarize.arguments.as_deref(), Some("path"));
        assert_eq!(summarize.source, "project");

        let skill = entries.iter().find(|entry| entry.name == "tdd").unwrap();
        assert_eq!(skill.kind, "skill");
        assert_eq!(skill.description, "Write the test first");
        // A skill takes a free-form argument, so the entry is completed for
        // editing rather than run bare.
        assert_eq!(skill.arguments.as_deref(), Some("[arguments]"));

        // A description-less template falls back to its own first line.
        std::fs::write(
            dir.join(".oxide").join("commands").join("plain.md"),
            "## Fix the build\n\nRun cargo build.\n",
        )
        .unwrap();
        let plain = palette_with(&dir, true)
            .into_iter()
            .find(|entry| entry.name == "plain")
            .unwrap();
        assert_eq!(plain.description, "Fix the build");
        assert_eq!(plain.arguments, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_untrusted_project_contributes_nothing_of_its_own() {
        let dir = temp_dir("untrusted");
        std::fs::create_dir_all(dir.join(".oxide").join("commands")).unwrap();
        std::fs::write(
            dir.join(".oxide").join("commands").join("secret.md"),
            "Do the secret thing.\n",
        )
        .unwrap();

        assert!(!names(&palette_with(&dir, false)).contains(&"secret"));
        assert!(names(&palette_with(&dir, true)).contains(&"secret"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_configured_command_cannot_shadow_a_builtin() {
        let dir = temp_dir("shadow");
        std::fs::create_dir_all(dir.join(".oxide").join("commands")).unwrap();
        std::fs::write(
            dir.join(".oxide").join("commands").join("model.md"),
            "Pretend to pick a model.\n",
        )
        .unwrap();
        // A project's own command that spells a retired alias is an ordinary
        // command now: only a name the catalog holds is drawn by the client
        // first, and `mcps` is no longer one of them.
        std::fs::write(
            dir.join(".oxide").join("commands").join("mcps.md"),
            "Pretend to list servers.\n",
        )
        .unwrap();

        let entries = palette_with(&dir, true);
        assert_eq!(
            entries.iter().filter(|entry| entry.name == "model").count(),
            1
        );
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.name == "model")
                .unwrap()
                .source,
            "builtin"
        );
        assert_eq!(
            entries.iter().filter(|entry| entry.name == "mcps").count(),
            1
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_skill_a_command_claims_is_listed_once() {
        let dir = temp_dir("skill_shadow");
        std::fs::create_dir_all(dir.join(".oxide").join("commands")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide").join("skills").join("tdd")).unwrap();
        std::fs::write(
            dir.join(".oxide").join("commands").join("tdd.md"),
            "---\ndescription: The project's own test-first command\n---\nWrite the test first.\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(".oxide")
                .join("skills")
                .join("tdd")
                .join("SKILL.md"),
            "---\ndescription: A skill named the same\n---\nWrite the test first, too.\n",
        )
        .unwrap();

        let entries = palette_with(&dir, true);
        let rows: Vec<&CommandEntry> = entries.iter().filter(|entry| entry.name == "tdd").collect();
        // The command is what `/tdd` resolves to, so it is the one row.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "prompt");
        assert_eq!(rows[0].description, "The project's own test-first command");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_project_override_is_not_reported_as_global() {
        let entry = CommandEntry {
            name: "ship".to_string(),
            description: "Ship the branch".to_string(),
            arguments: Some("arguments".to_string()),
            kind: Kind::Prompt.label().to_string(),
            front_ends: every_front_end(),
            desktop_only: false,
            source: String::new(),
        };
        let mut overridden = entry.clone();
        overridden.description = "Ship the branch the project's way".to_string();
        let global = [entry.clone()];
        // The same definition is the global scope's, whoever loaded it.
        assert_eq!(scope_of(&entry, &global), "global");
        // A name the project defines differently is the project's own, even
        // though the global scope has one of that name.
        assert_eq!(scope_of(&overridden, &global), "project");
        assert_eq!(scope_of(&overridden, &[]), "project");
    }

    #[test]
    fn a_summary_is_one_line_and_bounded() {
        assert_eq!(summarize("\n\n# Heading\nmore"), "Heading");
        let long = summarize(&"x".repeat(200));
        assert_eq!(long.chars().count(), 80);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn argument_templates_are_detected() {
        assert!(takes_arguments("Review $ARGUMENTS"));
        assert!(takes_arguments("Fix $1"));
        assert!(takes_arguments("Use ${1:-main}"));
        assert!(!takes_arguments("Summarize the repository."));
    }
}
