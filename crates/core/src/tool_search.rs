//! Deferred tool discovery using Pi's BM25 approach (k1=1.2, b=0.75).

use crate::llm::ToolSpec;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "is", "it", "of", "on",
    "or", "that", "the", "this", "to", "with",
];

fn terms(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut split = String::new();
    for (index, &ch) in chars.iter().enumerate() {
        if index > 0 && ch.is_ascii_uppercase() {
            let previous = chars[index - 1];
            let next_lower = chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
            if previous.is_ascii_lowercase()
                || previous.is_ascii_digit()
                || (previous.is_ascii_uppercase() && next_lower)
            {
                split.push(' ');
            }
        }
        split.push(ch.to_ascii_lowercase());
    }
    split
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|term| !term.is_empty() && !STOP_WORDS.contains(term))
        .map(|term| {
            if term.len() > 4 && term.ends_with("ies") {
                format!("{}y", &term[..term.len() - 3])
            } else if term.len() > 4
                && ["ches", "shes", "sses", "xes", "zes"]
                    .iter()
                    .any(|s| term.ends_with(s))
            {
                term[..term.len() - 2].to_string()
            } else if term.len() > 3 && term.ends_with('s') && !term.ends_with("ss") {
                term[..term.len() - 1].to_string()
            } else {
                term.to_string()
            }
        })
        .collect()
}

fn schema_text(schema: &Value, parts: &mut Vec<String>) {
    if let Some(description) = schema.get("description").and_then(Value::as_str) {
        parts.push(description.to_string());
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            parts.push(name.clone());
            schema_text(property, parts);
        }
    }
    if let Some(items) = schema.get("items") {
        schema_text(items, parts);
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(variants) = schema.get(key).and_then(Value::as_array) {
            for variant in variants {
                schema_text(variant, parts);
            }
        }
    }
}

pub(crate) fn document(tool: &ToolSpec, namespace: &str) -> String {
    let mut parts = vec![
        tool.function.name.clone(),
        tool.function.name.replace('_', " "),
        tool.function.description.clone(),
        namespace.to_string(),
    ];
    schema_text(&tool.function.parameters, &mut parts);
    parts.join(" ")
}

pub(crate) fn rank(query: &str, documents: &[&str], limit: usize) -> Vec<usize> {
    let query: BTreeSet<String> = terms(query).into_iter().collect();
    if query.is_empty() || documents.is_empty() || limit == 0 {
        return Vec::new();
    }
    let counts: Vec<BTreeMap<String, usize>> = documents
        .iter()
        .map(|text| {
            let mut counts = BTreeMap::new();
            for term in terms(text) {
                *counts.entry(term).or_default() += 1;
            }
            counts
        })
        .collect();
    let lengths: Vec<usize> = counts.iter().map(|counts| counts.values().sum()).collect();
    let average = (lengths.iter().sum::<usize>() as f64 / documents.len() as f64).max(1.0);
    let inverse: BTreeMap<&str, f64> = query
        .iter()
        .map(|term| {
            let frequency = counts
                .iter()
                .filter(|counts| counts.contains_key(term))
                .count() as f64;
            (
                term.as_str(),
                (1.0 + (documents.len() as f64 - frequency + 0.5) / (frequency + 0.5)).ln(),
            )
        })
        .collect();
    let mut scores: Vec<(usize, f64)> = counts
        .iter()
        .enumerate()
        .filter_map(|(index, counts)| {
            let norm = 1.2 * (0.25 + 0.75 * lengths[index] as f64 / average);
            let score: f64 = query
                .iter()
                .filter_map(|term| {
                    counts.get(term).map(|count| {
                        let count = *count as f64;
                        inverse[term.as_str()] * count * 2.2 / (count + norm)
                    })
                })
                .sum();
            (score > 0.0).then_some((index, score))
        })
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scores
        .into_iter()
        .take(limit)
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::FunctionSpec;
    use serde_json::json;

    #[test]
    fn discovers_capabilities_from_names_schema_and_server_notes() {
        let tool = ToolSpec {
            kind: "function",
            function: FunctionSpec {
                name: "company__readThread".into(),
                description: "Read messages".into(),
                parameters: json!({"properties":{"channelId":{"description":"Slack channel"}},"anyOf":[{"properties":{"timestamp":{"description":"Permalink timestamp"}}}]}),
            },
        };
        let document = document(&tool, "Company tools for private collaboration");
        assert_eq!(
            rank("Slack thread permalink", &[&document, "Run a SQL query"], 8),
            vec![0]
        );
        assert_eq!(
            terms("readThread HTTPRequest issues searches"),
            vec!["read", "thread", "http", "request", "issue", "search"]
        );
    }

    #[test]
    fn ranks_relevant_tools_and_keeps_limits_and_ties_stable() {
        let documents = [
            "Slack search messages",
            "Read Slack thread messages",
            "Deploy services",
        ];
        assert_eq!(rank("read thread", &documents, 1), vec![1]);
        assert_eq!(rank("unknown", &documents, 8), Vec::<usize>::new());
        assert!(rank("the and", &documents, 8).is_empty());
        assert_eq!(rank("slack", &["slack", "slack"], 8), vec![0, 1]);
    }
}
