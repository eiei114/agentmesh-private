//! Non-Multica request adapter contract.
//!
//! Converts agentmesh-request.v0 Markdown or JSON-compatible request sources into
//! compact canonical fields for tracker-neutral runners.

use agentmesh_request_evidence::{adapter_evidence_digest, RequestEvidenceFields};
use serde::Deserialize;
use serde_json::{json, Map, Value};

/// Plugin/schema version exposed in compact output.
pub const ADAPTER_VERSION: &str = "non-multica-request-adapter.v0";
const INPUT_SCHEMA_VERSION: &str = "non-multica-request-adapter-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "non-multica-request-adapter-compact.v0";
const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";
const MAX_SOURCE_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
struct AdapterInput {
    schema_version: String,
    #[serde(default)]
    markdown: Option<String>,
    #[serde(default)]
    request: Option<Value>,
}

#[derive(Debug, Default)]
struct RequestFields {
    title: Option<String>,
    request_kind: Option<String>,
    issue_type: Option<String>,
    ready_for_multica: Option<bool>,
    status: Option<String>,
    project_key: Option<String>,
    source_prd: Option<String>,
    source_design: Option<String>,
    source_roadmap: Option<String>,
    blocked_by: Vec<String>,
    unblocks: Vec<String>,
    sequence_index: Option<u64>,
    sequence_total: Option<u64>,
}

/// Adapt opaque plugin input and return deterministic compact JSON.
pub fn adapt_request_input(value: &Value) -> Value {
    let input: Result<AdapterInput, _> = serde_json::from_value(value.clone());
    let input = match input {
        Ok(input) => input,
        Err(err) => {
            return compact(
                None,
                vec![issue(
                    "input_invalid",
                    format!("input must match schema: {err}"),
                )],
            )
        }
    };

    let mut issues = Vec::new();
    if input.schema_version != INPUT_SCHEMA_VERSION {
        issues.push(issue(
            "unsupported_schema_version",
            format!("schema_version must be {INPUT_SCHEMA_VERSION}"),
        ));
    }
    if input.markdown.is_some() == input.request.is_some() {
        issues.push(issue(
            "source_shape_invalid",
            "provide exactly one of markdown or request",
        ));
        return compact(None, issues);
    }

    let fields = if let Some(markdown) = input.markdown {
        if markdown.len() > MAX_SOURCE_BYTES {
            issues.push(issue(
                "source_too_large",
                format!(
                    "source is {} bytes; limit is {MAX_SOURCE_BYTES}",
                    markdown.len()
                ),
            ));
        }
        let Some(frontmatter) = parse_frontmatter(&markdown) else {
            issues.push(issue(
                "frontmatter_missing",
                "YAML frontmatter block is required for markdown sources",
            ));
            return compact(None, issues);
        };
        fields_from_frontmatter(frontmatter)
    } else {
        let request = input.request.unwrap_or(Value::Null);
        let Some(object) = request.as_object() else {
            issues.push(issue("request_not_object", "request must be a JSON object"));
            return compact(None, issues);
        };
        fields_from_object(object)
    };

    validate_fields(&fields, &mut issues);
    compact(Some(fields), issues)
}

fn validate_fields(fields: &RequestFields, issues: &mut Vec<Value>) {
    if fields.title.as_deref().unwrap_or("").trim().is_empty() {
        issues.push(issue("title_missing", "request title is required"));
    }
    if fields.request_kind.as_deref() != Some("app") {
        issues.push(issue(
            "unsupported_request_kind",
            "request_kind must be app",
        ));
    }
    if fields.issue_type.as_deref().unwrap_or("").trim().is_empty() {
        issues.push(issue("issue_type_missing", "issue_type is required"));
    }
    if fields.sequence_index.is_some() != fields.sequence_total.is_some() {
        issues.push(issue(
            "sequence_incomplete",
            "sequence_index and sequence_total must be provided together",
        ));
    }
}

fn compact(fields: Option<RequestFields>, issues: Vec<Value>) -> Value {
    let valid = issues.is_empty();
    let evidence_digest = fields
        .as_ref()
        .map(evidence_fields)
        .map(|fields| adapter_evidence_digest(&fields));
    let canonical = fields.map(|f| {
        json!({
            "title": f.title,
            "request_kind": f.request_kind,
            "issue_type": f.issue_type,
            "ready_for_multica": f.ready_for_multica,
            "status": f.status,
            "project_key": f.project_key,
            "source_prd": f.source_prd,
            "source_design": f.source_design,
            "source_roadmap": f.source_roadmap,
            "blocked_by": f.blocked_by,
            "unblocks": f.unblocks,
            "sequence_index": f.sequence_index,
            "sequence_total": f.sequence_total,
        })
    });
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "adapter_version": ADAPTER_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "valid": valid,
        "canonical": canonical,
        "evidence_digest": evidence_digest,
        "issue_count": issues.len(),
        "issues": issues,
    })
}

fn issue(code: &str, message: impl Into<String>) -> Value {
    json!({"code": code, "message": message.into()})
}

fn evidence_fields(fields: &RequestFields) -> RequestEvidenceFields {
    RequestEvidenceFields {
        title: fields.title.clone(),
        request_kind: fields.request_kind.clone(),
        issue_type: fields.issue_type.clone(),
        ready_for_multica: fields.ready_for_multica,
        status: fields.status.clone(),
        project_key: fields.project_key.clone(),
        source_prd: fields.source_prd.clone(),
        source_design: fields.source_design.clone(),
        source_roadmap: fields.source_roadmap.clone(),
        blocked_by: fields.blocked_by.clone(),
        unblocks: fields.unblocks.clone(),
        sequence_index: fields.sequence_index,
        sequence_total: fields.sequence_total,
    }
}

fn parse_frontmatter(markdown: &str) -> Option<&str> {
    let rest = markdown.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some(&rest[..end])
}

fn fields_from_frontmatter(frontmatter: &str) -> RequestFields {
    let mut fields = RequestFields::default();
    for line in frontmatter.lines() {
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        set_field(&mut fields, key.trim(), scalar(raw.trim()));
    }
    fields
}

fn fields_from_object(object: &Map<String, Value>) -> RequestFields {
    let mut fields = RequestFields::default();
    for (key, value) in object {
        set_field(&mut fields, key, value.clone());
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

fn set_field(fields: &mut RequestFields, key: &str, value: Value) {
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
            .filter_map(|v| v.as_str().map(ToString::to_string))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert_eq!(
                adapt_request_input(&input),
                expected,
                "{input_name} should match {expected_name}"
            );
        }
    }
}
