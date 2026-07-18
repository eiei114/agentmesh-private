//! Tool-neutral Markdown request validator.
//!
//! This plugin accepts one Markdown request document as JSON, validates the
//! deterministic request contract, and emits a compact adapter-neutral result.

use serde::Deserialize;
use serde_json::{json, Value};

/// Plugin/schema version exposed in compact output.
pub const VALIDATOR_VERSION: &str = "markdown-request-validator.v0";
const INPUT_SCHEMA_VERSION: &str = "markdown-request-validator-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "markdown-request-validator-compact.v0";
const MAX_MARKDOWN_BYTES: usize = 64 * 1024;
const REQUIRED_SECTIONS: &[&str] = &[
    "What to build",
    "Acceptance criteria",
    "Blocked by",
    "User stories covered",
    "Notes",
];

#[derive(Debug, Deserialize)]
struct ValidatorInput {
    schema_version: String,
    markdown: String,
}

/// Validate opaque plugin input and return deterministic compact JSON.
pub fn validate_request_input(value: &Value) -> Value {
    let input: Result<ValidatorInput, _> = serde_json::from_value(value.clone());
    let input = match input {
        Ok(input) => input,
        Err(err) => {
            return compact(
                false,
                None,
                vec![issue(
                    "input_invalid",
                    format!("input must match schema: {err}"),
                )],
            );
        }
    };

    let mut issues = Vec::new();
    if input.schema_version != INPUT_SCHEMA_VERSION {
        issues.push(issue(
            "unsupported_schema_version",
            format!("schema_version must be {INPUT_SCHEMA_VERSION}"),
        ));
    }
    let byte_len = input.markdown.len();
    if byte_len == 0 {
        issues.push(issue("markdown_empty", "markdown must not be empty"));
    }
    if byte_len > MAX_MARKDOWN_BYTES {
        issues.push(issue(
            "markdown_too_large",
            format!("markdown is {byte_len} bytes; limit is {MAX_MARKDOWN_BYTES}"),
        ));
    }

    let frontmatter = parse_frontmatter(&input.markdown);
    if frontmatter.is_none() {
        issues.push(issue(
            "frontmatter_missing",
            "YAML frontmatter block is required",
        ));
    }
    let title = frontmatter.and_then(|fm| frontmatter_value(fm, "title"));
    if title.as_deref().unwrap_or("").trim().is_empty() {
        issues.push(issue("title_missing", "frontmatter title is required"));
    }

    let headings = headings(&input.markdown);
    for section in REQUIRED_SECTIONS {
        if !headings.iter().any(|h| h == section) {
            issues.push(issue(
                "required_section_missing",
                format!("missing section: {section}"),
            ));
        }
    }

    let valid = issues.is_empty();
    compact(valid, title, issues)
}

fn compact(valid: bool, title: Option<String>, issues: Vec<Value>) -> Value {
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "validator_version": VALIDATOR_VERSION,
        "valid": valid,
        "title": title,
        "required_sections": REQUIRED_SECTIONS,
        "issue_count": issues.len(),
        "issues": issues,
    })
}

fn issue(code: &str, message: impl Into<String>) -> Value {
    json!({"code": code, "message": message.into()})
}

fn parse_frontmatter(markdown: &str) -> Option<&str> {
    let rest = markdown.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some(&rest[..end])
}

fn frontmatter_value(frontmatter: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    frontmatter.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix(&prefix)?;
        Some(rest.trim().trim_matches('"').to_string())
    })
}

fn headings(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            let text = trimmed.strip_prefix("## ")?;
            Some(text.trim().to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_markdown() -> String {
        "---\ntitle: \"Add validator\"\n---\n# Add validator\n\n## What to build\nBuild it.\n\n## Acceptance criteria\n- Pass.\n\n## Blocked by\n- None.\n\n## User stories covered\n- As a tool author...\n\n## Notes\n- Tool-neutral.\n".into()
    }

    #[test]
    fn valid_fixture_is_compact_and_ok() {
        let output = validate_request_input(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "markdown": valid_markdown()
        }));
        assert_eq!(output["schema_version"], OUTPUT_SCHEMA_VERSION);
        assert_eq!(output["valid"], true);
        assert_eq!(output["issue_count"], 0);
        assert_eq!(output["title"], "Add validator");
    }

    #[test]
    fn invalid_document_reports_deterministic_issues() {
        let output = validate_request_input(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "markdown": "# Missing contract\n"
        }));
        assert_eq!(output["valid"], false);
        assert_eq!(output["issues"][0]["code"], "frontmatter_missing");
        assert_eq!(output["issues"][1]["code"], "title_missing");
    }

    #[test]
    fn bounded_input_is_enforced() {
        let output = validate_request_input(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "markdown": "x".repeat(MAX_MARKDOWN_BYTES + 1)
        }));
        assert!(output["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "markdown_too_large"));
    }

    #[test]
    fn recorded_fixtures_match_expected_payloads() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "valid_request_input.json",
                "expected_valid_compact_payload.json",
            ),
            (
                "invalid_request_input.json",
                "expected_invalid_compact_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            let actual = validate_request_input(&input);
            assert_eq!(
                actual, expected,
                "{input_name} should match {expected_name}"
            );
        }
    }
}
