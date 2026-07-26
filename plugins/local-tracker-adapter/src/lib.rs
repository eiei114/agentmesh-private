//! Local tracker adapter contract.
//!
//! Converts `agentmesh-request.v0` Markdown or JSON-compatible request sources
//! into a deterministic compact payload for a local taskfile-style tracker.
//! Stable request fields stay under `canonical`; adapter-owned routing fields and
//! passthrough extensions stay under `adapter`.

use agentmesh_request_evidence::{adapter_evidence_digest, RequestEvidenceFields};
use serde::Deserialize;
use serde_json::{json, Map, Value};

/// Plugin/schema version exposed in compact output.
pub const ADAPTER_VERSION: &str = "local-tracker-adapter.v0";
const INPUT_SCHEMA_VERSION: &str = "local-tracker-adapter-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "local-tracker-adapter-compact.v0";
const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";
const LOCAL_TRACKER_VERSION: &str = "local-taskfile.v0";
const MAX_SOURCE_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
struct AdapterInput {
    schema_version: String,
    #[serde(default)]
    markdown: Option<String>,
    #[serde(default)]
    request: Option<Value>,
    #[serde(default)]
    adapter: AdapterOptions,
}

#[derive(Debug, Default, Deserialize)]
struct AdapterOptions {
    #[serde(default)]
    passthrough: Option<Value>,
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
                None,
                None,
                vec![issue(
                    "input_invalid",
                    format!("input must match schema: {err}"),
                )],
                None,
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
        return compact(None, None, None, issues, None);
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
            return compact(None, None, None, issues, None);
        };
        fields_from_frontmatter(frontmatter)
    } else {
        let request = input.request.unwrap_or(Value::Null);
        let Some(object) = request.as_object() else {
            issues.push(issue("request_not_object", "request must be a JSON object"));
            return compact(None, None, None, issues, None);
        };
        fields_from_object(object)
    };

    validate_fields(&fields, &mut issues);
    let extension = adapter_extension(&input.adapter, &mut issues);
    let canonical = canonical_payload(&fields);
    let evidence_digest = Some(adapter_evidence_digest(&evidence_fields(&fields)));
    if issues.is_empty() {
        let adapter = adapter_payload(&fields, &extension);
        let tracker = tracker_ready_payload(&fields);
        compact(
            Some(canonical),
            Some(adapter),
            Some(tracker),
            issues,
            evidence_digest,
        )
    } else {
        compact(Some(canonical), None, None, issues, evidence_digest)
    }
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

fn adapter_extension(adapter: &AdapterOptions, issues: &mut Vec<Value>) -> Value {
    match &adapter.passthrough {
        None => json!({}),
        Some(value) if value.is_object() => value.clone(),
        Some(_) => {
            issues.push(issue(
                "adapter_passthrough_not_object",
                "adapter.passthrough must be an object when provided",
            ));
            json!({})
        }
    }
}

fn compact(
    canonical: Option<Value>,
    adapter: Option<Value>,
    tracker_ready_payload: Option<Value>,
    issues: Vec<Value>,
    evidence_digest: Option<Value>,
) -> Value {
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "adapter_version": ADAPTER_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "valid": issues.is_empty(),
        "canonical": canonical,
        "evidence_digest": evidence_digest,
        "adapter": adapter,
        "tracker_ready_payload": tracker_ready_payload,
        "issue_count": issues.len(),
        "issues": issues,
    })
}

fn canonical_payload(fields: &RequestFields) -> Value {
    json!({
        "title": fields.title,
        "request_kind": fields.request_kind,
        "issue_type": fields.issue_type,
        "status": fields.status,
        "project_key": fields.project_key,
        "source_prd": fields.source_prd,
        "source_design": fields.source_design,
        "source_roadmap": fields.source_roadmap,
        "blocked_by": fields.blocked_by,
        "unblocks": fields.unblocks,
        "sequence_index": fields.sequence_index,
        "sequence_total": fields.sequence_total,
    })
}

fn adapter_payload(fields: &RequestFields, extension: &Value) -> Value {
    let project = fields.project_key.as_deref().unwrap_or("unassigned");
    let title = fields.title.as_deref().unwrap_or("untitled");
    json!({
        "tracker": LOCAL_TRACKER_VERSION,
        "local_id": format!("local-taskfile://{project}/{}", slug(title)),
        "state": fields.status.as_deref().unwrap_or("draft"),
        "extension": extension,
    })
}

fn tracker_ready_payload(fields: &RequestFields) -> Value {
    let project = fields.project_key.as_deref().unwrap_or("unassigned");
    let title = fields.title.as_deref().unwrap_or("untitled");
    json!({
        "id": format!("local-taskfile://{project}/{}", slug(title)),
        "title": fields.title,
        "kind": fields.issue_type,
        "project": fields.project_key,
        "state": fields.status.as_deref().unwrap_or("draft"),
        "source": {
            "request_schema_version": REQUEST_SCHEMA_VERSION,
            "prd": fields.source_prd,
            "design": fields.source_design,
            "roadmap": fields.source_roadmap,
        },
        "dependencies": {
            "blocked_by": fields.blocked_by,
            "unblocks": fields.unblocks,
        },
        "sequence": {
            "index": fields.sequence_index,
            "total": fields.sequence_total,
        },
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

fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut previous_dash = false;
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            previous_dash = false;
        } else if !previous_dash && !out.is_empty() {
            out.push('-');
            previous_dash = true;
        }
        if out.len() >= 64 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
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

    #[test]
    fn slug_is_stable_and_ascii_only() {
        assert_eq!(
            slug("Add a local tracker adapter app!"),
            "add-a-local-tracker-adapter-app"
        );
        assert_eq!(slug("---"), "untitled");
    }
}
