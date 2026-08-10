//! Tool-neutral Markdown request validator.
//!
//! This plugin accepts one Markdown request document as JSON, validates the
//! deterministic request contract, and emits a compact adapter-neutral result.

pub mod adapter_error_contract;
pub mod request_dry_run_summary;
pub mod request_fingerprint_manifest;

use agentmesh_request_evidence::{adapter_evidence_digest, RequestEvidenceFields};
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
    let fields = frontmatter.map(fields_from_frontmatter);
    let title = fields.as_ref().and_then(|fields| fields.title.clone());
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

    let evidence_digest = fields.as_ref().map(adapter_evidence_digest);
    let valid = issues.is_empty();
    compact(valid, title, evidence_digest, issues)
}

fn compact(
    valid: bool,
    title: Option<String>,
    evidence_digest: Option<Value>,
    issues: Vec<Value>,
) -> Value {
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "validator_version": VALIDATOR_VERSION,
        "valid": valid,
        "title": title,
        "required_sections": REQUIRED_SECTIONS,
        "evidence_digest": evidence_digest,
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

fn fields_from_frontmatter(frontmatter: &str) -> RequestEvidenceFields {
    let mut fields = RequestEvidenceFields::default();
    for line in frontmatter.lines() {
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        set_field(&mut fields, key.trim(), scalar(raw.trim()));
    }
    fields
}

fn scalar(raw: &str) -> Value {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.starts_with('[') {
        return serde_json::from_str(trimmed)
            .unwrap_or_else(|_| Value::String(trimmed.to_string()));
    }
    match trimmed {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => trimmed
            .parse::<u64>()
            .map_or_else(|_| Value::String(trimmed.to_string()), |n| json!(n)),
    }
}

fn set_field(fields: &mut RequestEvidenceFields, key: &str, value: Value) {
    match key {
        "title" => fields.title = value.as_str().map(ToString::to_string),
        "request_kind" => fields.request_kind = value.as_str().map(ToString::to_string),
        "issue_type" => fields.issue_type = value.as_str().map(ToString::to_string),
        "ready_for_multica" => fields.ready_for_multica = value.as_bool(),
        "status" => fields.status = value.as_str().map(ToString::to_string),
        "project_key" => fields.project_key = value.as_str().map(ToString::to_string),
        "source_prd" => fields.source_prd = value.as_str().map(ToString::to_string),
        "source_design" => fields.source_design = value.as_str().map(ToString::to_string),
        "source_roadmap" => fields.source_roadmap = value.as_str().map(ToString::to_string),
        "blocked_by" => fields.blocked_by = strings(value),
        "unblocks" => fields.unblocks = strings(value),
        "sequence_index" => fields.sequence_index = value.as_u64(),
        "sequence_total" => fields.sequence_total = value.as_u64(),
        _ => {}
    }
}

fn strings(value: Value) -> Vec<String> {
    value.as_array().map_or_else(Vec::new, |items| {
        items
            .iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect()
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
