//! `agentmesh docs list` JSON contract.

use super::EMBEDDED_DOCS;
use serde::Serialize;
use std::process::ExitCode;

pub const DOCS_LIST_HELP: &str = "Use `agentmesh docs list` to view embedded document metadata.";
const LIST_SCHEMA_VERSION: &str = "agentmesh-docs-list.v0";

#[derive(Debug, Serialize)]
struct DocsListEntry<'a> {
    name: &'a str,
    description: &'a str,
    source: &'a str,
}

#[derive(Debug, Serialize)]
struct DocsListOutput<'a> {
    schema_version: &'a str,
    results: Vec<DocsListEntry<'a>>,
    help: &'a str,
}

/// Emit the compact list JSON contract to stdout.
pub fn docs_list_command() -> ExitCode {
    let payload = render_docs_list_json();
    println!("{payload}");
    ExitCode::SUCCESS
}

fn render_docs_list_json() -> String {
    let mut results: Vec<DocsListEntry<'_>> = EMBEDDED_DOCS
        .iter()
        .map(|document| DocsListEntry {
            name: document.name,
            description: document.description,
            source: document.source,
        })
        .collect();
    results.sort_by(|left, right| left.name.cmp(right.name));

    let payload = DocsListOutput {
        schema_version: LIST_SCHEMA_VERSION,
        results,
        help: DOCS_LIST_HELP,
    };
    serde_json::to_string(&payload).expect("serialize docs list JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docs_list_results_are_lexicographically_sorted_by_name() {
        let json = render_docs_list_json();
        let payload: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let names = payload["results"]
            .as_array()
            .expect("results array")
            .iter()
            .map(|entry| entry["name"].as_str().expect("name"))
            .collect::<Vec<_>>();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[test]
    fn docs_list_includes_all_embedded_documents() {
        let json = render_docs_list_json();
        let payload: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let names = payload["results"]
            .as_array()
            .expect("results array")
            .iter()
            .map(|entry| entry["name"].as_str().expect("name"))
            .collect::<Vec<_>>();
        let embedded: Vec<_> = EMBEDDED_DOCS.iter().map(|doc| doc.name).collect();
        assert_eq!(names.len(), embedded.len());
        for name in embedded {
            assert!(names.contains(&name), "missing embedded doc {name}");
        }
    }
}
