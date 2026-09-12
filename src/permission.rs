use crate::config::Config;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone)]
enum RuleSet {
    Flat(Action),
    Patterns(Vec<(String, Action)>),
}

#[derive(Debug, Clone, Default)]
pub struct Permissions {
    baseline: Option<Action>,
    rules: Vec<(String, RuleSet)>,
}

impl Permissions {
    pub fn from_config(config: &Config) -> Self {
        let value = config
            .active_agent
            .as_ref()
            .and_then(|agent| agent.permission.clone());
        match value {
            Some(value) => Self::from_value(&value),
            None => Self::default(),
        }
    }

    fn from_value(value: &Value) -> Self {
        match value {
            Value::String(action) => Self {
                baseline: Action::parse_str(action),
                rules: Vec::new(),
            },
            Value::Object(map) => {
                let mut rules = Vec::new();
                for (tool, rule) in map {
                    match rule {
                        Value::String(action) => {
                            if let Some(action) = Action::parse_str(action) {
                                rules.push((tool.clone(), RuleSet::Flat(action)));
                            }
                        }
                        Value::Object(patterns) => {
                            let mut entries = Vec::new();
                            for (pattern, action) in patterns {
                                if let Some(action) = action.as_str().and_then(Action::parse_str) {
                                    entries.push((pattern.clone(), action));
                                }
                            }
                            rules.push((tool.clone(), RuleSet::Patterns(entries)));
                        }
                        _ => {}
                    }
                }
                Self {
                    baseline: None,
                    rules,
                }
            }
            _ => Self::default(),
        }
    }

    pub fn decide(&self, tool: &str, subject: &str) -> Action {
        let mut action = self.baseline.unwrap_or_else(|| default_action(tool));
        for (name, rule) in &self.rules {
            if name != "*" && name != tool {
                continue;
            }
            match rule {
                RuleSet::Flat(value) => action = *value,
                RuleSet::Patterns(patterns) => {
                    for (pattern, value) in patterns {
                        if pattern_matches(pattern, subject) {
                            action = *value;
                        }
                    }
                }
            }
        }
        action
    }
}

impl Action {
    fn parse_str(value: &str) -> Option<Action> {
        match value {
            "allow" => Some(Action::Allow),
            "ask" => Some(Action::Ask),
            "deny" => Some(Action::Deny),
            _ => None,
        }
    }
}

fn default_action(tool: &str) -> Action {
    match tool {
        "read_file" | "list_dir" | "glob" | "grep" | "webfetch" => Action::Allow,
        "write_file" | "patch" | "bash" => Action::Ask,
        _ => Action::Allow,
    }
}

pub fn subject_for(tool: &str, args: &Value) -> String {
    match tool {
        "bash" => args
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    wildcard(
        &pattern.chars().collect::<Vec<_>>(),
        &value.chars().collect::<Vec<_>>(),
    )
}

fn wildcard(pattern: &[char], text: &[char]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }
    match pattern[0] {
        '*' => wildcard(&pattern[1..], text) || (!text.is_empty() && wildcard(pattern, &text[1..])),
        '?' => !text.is_empty() && wildcard(&pattern[1..], &text[1..]),
        ch => !text.is_empty() && text[0] == ch && wildcard(&pattern[1..], &text[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_allow_reads_and_ask_writes() {
        let permissions = Permissions::default();
        assert_eq!(permissions.decide("read_file", "a.rs"), Action::Allow);
        assert_eq!(permissions.decide("bash", "ls"), Action::Ask);
        assert_eq!(permissions.decide("write_file", "a.rs"), Action::Ask);
    }

    #[test]
    fn pattern_rules_use_last_match() {
        let permissions = Permissions::from_value(&json!({
            "bash": { "*": "ask", "git *": "allow" }
        }));
        assert_eq!(permissions.decide("bash", "git status"), Action::Allow);
        assert_eq!(permissions.decide("bash", "rm -rf /"), Action::Ask);
    }

    #[test]
    fn flat_rules_and_wildcards() {
        let permissions = Permissions::from_value(&json!({ "edit": "deny", "*": "allow" }));
        assert_eq!(permissions.decide("edit", "a.rs"), Action::Deny);
        assert_eq!(permissions.decide("bash", "ls"), Action::Allow);
    }

    #[test]
    fn top_level_string_sets_baseline() {
        let permissions = Permissions::from_value(&json!("deny"));
        assert_eq!(permissions.decide("read_file", "a.rs"), Action::Deny);
    }

    #[test]
    fn extracts_subject_from_arguments() {
        assert_eq!(subject_for("bash", &json!({"command":"ls"})), "ls");
        assert_eq!(subject_for("write_file", &json!({"path":"a.rs"})), "a.rs");
    }
}
