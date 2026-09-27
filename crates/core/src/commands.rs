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

/// One built-in command. `name` is written without its slash.
pub struct Builtin {
    pub name: &'static str,
    /// Alternative spellings a client accepts, e.g. `mcps` for `mcp`.
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    /// An argument hint for autocomplete and help, e.g. `on|off|list|clear`.
    pub arguments: Option<&'static str>,
    /// Desktop-only commands have no meaning where the client cannot perform
    /// them: provider login and the theme picker live in the app's own UI.
    pub desktop_only: bool,
}

/// The commands every client offers. Order is the order they are shown in.
pub const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "help",
        aliases: &[],
        description: "List the commands and keyboard shortcuts",
        arguments: None,
        desktop_only: false,
    },
    Builtin {
        name: "mcp",
        aliases: &["mcps"],
        description: "List MCP servers and their connection status",
        arguments: None,
        desktop_only: false,
    },
    Builtin {
        name: "model",
        aliases: &[],
        description: "Choose the model to run",
        arguments: None,
        desktop_only: false,
    },
    Builtin {
        name: "reasoning",
        aliases: &["thinking"],
        description: "Set the reasoning effort for this chat",
        arguments: Some("auto|off|low|medium|high"),
        desktop_only: false,
    },
    Builtin {
        name: "agent",
        aliases: &[],
        description: "Choose the subagent this chat runs as",
        arguments: None,
        desktop_only: false,
    },
    Builtin {
        name: "permissions",
        aliases: &["approvals"],
        description: "Review the tools allowed without prompting",
        arguments: Some("on|off|list|clear"),
        desktop_only: false,
    },
    Builtin {
        name: "trust",
        aliases: &["access"],
        description: "Decide whether this project's own resources load",
        arguments: None,
        desktop_only: false,
    },
    Builtin {
        name: "session",
        aliases: &["sessions"],
        description: "Resume a session from this project",
        arguments: None,
        desktop_only: false,
    },
    Builtin {
        name: "new",
        aliases: &["clear"],
        description: "Start a new session",
        arguments: None,
        desktop_only: false,
    },
    Builtin {
        name: "usage",
        aliases: &["cost"],
        description: "Show this chat's tokens, cost and context",
        arguments: None,
        desktop_only: false,
    },
    Builtin {
        name: "attach",
        aliases: &[],
        description: "Attach images, PDFs or files",
        arguments: None,
        desktop_only: false,
    },
    Builtin {
        name: "theme",
        aliases: &[],
        description: "Choose the color theme",
        arguments: None,
        desktop_only: true,
    },
    Builtin {
        name: "connect",
        aliases: &["login"],
        description: "Sign in to a provider",
        arguments: Some("provider"),
        desktop_only: true,
    },
    Builtin {
        name: "logout",
        aliases: &[],
        description: "Forget a provider's stored credentials",
        arguments: Some("provider"),
        desktop_only: true,
    },
];

/// One row of a client's `/` menu, whether it draws it itself or sends it.
#[derive(Debug, Clone, Serialize)]
pub struct CommandEntry {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    /// An argument hint, or `None` when the command takes none. A client uses
    /// it to decide between running a command and completing it for editing.
    pub arguments: Option<String>,
    /// `client`, `prompt`, or `skill`.
    pub kind: String,
    pub desktop_only: bool,
    /// `builtin` for a client's own command, otherwise the scope the entry was
    /// discovered in (`project` or `global`).
    pub source: String,
}

impl CommandEntry {
    fn builtin(command: &Builtin) -> Self {
        Self {
            name: command.name.to_string(),
            aliases: command
                .aliases
                .iter()
                .map(|alias| alias.to_string())
                .collect(),
            description: command.description.to_string(),
            arguments: command.arguments.map(str::to_string),
            kind: Kind::Client.label().to_string(),
            desktop_only: command.desktop_only,
            source: "builtin".to_string(),
        }
    }
}

/// The built-in command a name or alias refers to.
pub fn builtin(name: &str) -> Option<&'static Builtin> {
    let name = name.trim().trim_start_matches('/').to_ascii_lowercase();
    BUILTINS
        .iter()
        .find(|command| command.name == name || command.aliases.contains(&name.as_str()))
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

    let mut entries: Vec<CommandEntry> = BUILTINS.iter().map(CommandEntry::builtin).collect();
    // The project load already merges the global scope, so the global listing
    // is what is compared against to tell where an entry came from.
    let global = configured(&global);

    for mut entry in configured(&project) {
        // A configured entry that matches a built-in or an alias is unreachable
        // — the client draws its own command first — so it is dropped rather
        // than listed twice.
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
        && left.aliases == right.aliases
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
            aliases: Vec::new(),
            description: command
                .description
                .clone()
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| summarize(&command.template)),
            arguments: takes_arguments(&command.template).then(|| "arguments".to_string()),
            kind: Kind::Prompt.label().to_string(),
            desktop_only: false,
            source: String::new(),
        });
    }
    for template in &ecosystem.prompt_templates {
        entries.push(CommandEntry {
            name: template.name.clone(),
            aliases: Vec::new(),
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
            aliases: Vec::new(),
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

/// `/name`, then its aliases and argument hint: `/reasoning (thinking) auto|off|low|medium|high`.
fn label(entry: &CommandEntry) -> String {
    let mut label = format!("/{}", entry.name);
    if !entry.aliases.is_empty() {
        label.push_str(&format!(" ({})", entry.aliases.join(", ")));
    }
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
    fn builtins_resolve_by_name_and_alias() {
        assert_eq!(builtin("mcp").unwrap().name, "mcp");
        assert_eq!(builtin("/mcps").unwrap().name, "mcp");
        assert_eq!(builtin("MCP").unwrap().name, "mcp");
        assert_eq!(builtin("approvals").unwrap().name, "permissions");
        assert_eq!(builtin("clear").unwrap().name, "new");
        assert!(builtin("nope").is_none());
    }

    #[test]
    fn builtins_agree_on_names_and_descriptions() {
        for command in BUILTINS {
            assert!(!command.name.is_empty());
            assert!(!command.description.is_empty());
            assert!(!command.name.contains('/'));
            assert!(!command.aliases.contains(&command.name));
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
            .any(|entry| entry.name == "theme" && entry.desktop_only));
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
        assert!(!names(&entries).contains(&"mcps"));
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
            aliases: Vec::new(),
            description: "Ship the branch".to_string(),
            arguments: Some("arguments".to_string()),
            kind: Kind::Prompt.label().to_string(),
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
