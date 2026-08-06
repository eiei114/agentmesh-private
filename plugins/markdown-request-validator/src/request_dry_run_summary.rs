//! Deterministic request dry-run summary App.
//!
//! The summary is intentionally adapter-neutral: it previews request materialization
//! as stable Markdown and exposes the same facts as a normalized JSON evidence
//! block so non-Multica runtimes can compare dry-run outcomes without tracker
//! fields leaking into the contract.

use serde_json::{json, Map, Value};
use std::fmt::Write as _;

/// Plugin/schema version exposed in compact output.
pub const SUMMARY_VERSION: &str = "request-dry-run-summary.v0";
const INPUT_SCHEMA_VERSION: &str = "request-dry-run-summary-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "request-dry-run-summary-compact.v0";
const EVIDENCE_SCHEMA_VERSION: &str = "request-dry-run-summary-evidence.v0";
const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";
const MAX_SOURCE_BYTES: usize = 64 * 1024;
const ACCEPTED_REQUEST_KINDS: &[&str] = &["app", "repair"];
const CANONICAL_FRONTMATTER_KEYS: &[&str] = &[
    "title",
    "request_kind",
    "issue_type",
    "ready_for_multica",
    "status",
    "project_key",
    "source_prd",
    "source_design",
    "source_roadmap",
    "blocked_by",
    "unblocks",
    "sequence_index",
    "sequence_total",
];

#[derive(Debug, Clone, Copy)]
enum FieldKind {
    String,
    Bool,
    StringArray,
    PositiveInteger,
}

#[derive(Debug, Clone, Copy)]
struct FieldSpec {
    key: &'static str,
    kind: FieldKind,
    required: bool,
}

const FRONTMATTER_SPECS: &[FieldSpec] = &[
    field_spec("title", FieldKind::String, true),
    field_spec("request_kind", FieldKind::String, true),
    field_spec("issue_type", FieldKind::String, true),
    field_spec("ready_for_multica", FieldKind::Bool, false),
    field_spec("status", FieldKind::String, true),
    field_spec("project_key", FieldKind::String, true),
    field_spec("source_prd", FieldKind::String, true),
    field_spec("source_design", FieldKind::String, true),
    field_spec("source_roadmap", FieldKind::String, true),
    field_spec("blocked_by", FieldKind::StringArray, true),
    field_spec("unblocks", FieldKind::StringArray, true),
    field_spec("sequence_index", FieldKind::PositiveInteger, true),
    field_spec("sequence_total", FieldKind::PositiveInteger, true),
    field_spec("pr_required", FieldKind::Bool, false),
    field_spec("pr_allowed", FieldKind::Bool, false),
    field_spec("release_allowed", FieldKind::Bool, false),
    field_spec("production_allowed", FieldKind::Bool, false),
    field_spec("version_bump_required", FieldKind::Bool, false),
    field_spec("package_publish_expected", FieldKind::Bool, false),
    field_spec("squad_candidate", FieldKind::Bool, false),
    field_spec("multi_role_required", FieldKind::Bool, false),
    field_spec("needs_design_first", FieldKind::Bool, false),
    field_spec("needs_decomposition", FieldKind::Bool, false),
    field_spec("expected_pr_count", FieldKind::PositiveInteger, false),
    field_spec("ambiguity_level", FieldKind::PositiveInteger, false),
    field_spec("failure_streak", FieldKind::PositiveInteger, false),
];

const fn field_spec(key: &'static str, kind: FieldKind, required: bool) -> FieldSpec {
    FieldSpec {
        key,
        kind,
        required,
    }
}

/// Summarize a request dry-run as deterministic Markdown plus normalized JSON evidence.
pub fn summarize_request_dry_run(value: &Value) -> Value {
    let mut errors = Vec::new();
    let Some(input) = value.as_object() else {
        errors.push(error(
            "AGENTMESH_INPUT_SCHEMA_INVALID",
            "input_schema_invalid",
            Some("$"),
            "input must be a JSON object",
        ));
        return compact(None, None, None, "unsupported", Map::new(), errors);
    };

    let schema_version = required_string(input, "schema_version", &mut errors);
    if let Some(version) = &schema_version {
        if version != INPUT_SCHEMA_VERSION {
            errors.push(error(
                "AGENTMESH_INPUT_SCHEMA_INVALID",
                "input_schema_invalid",
                Some("$.schema_version"),
                format!("schema_version must be {INPUT_SCHEMA_VERSION}"),
            ));
        }
    }
    let request_id = required_string(input, "request_id", &mut errors);
    let scope = required_string(input, "scope", &mut errors);
    let target_app = required_string(input, "target_app", &mut errors);

    let (source_shape, frontmatter) = parse_source(input, &mut errors);
    if source_shape != "unsupported" {
        validate_frontmatter(&frontmatter, source_shape, &mut errors);
    }

    compact(
        request_id,
        scope,
        target_app,
        source_shape,
        normalized_frontmatter(&frontmatter),
        errors,
    )
}

fn required_string(
    input: &Map<String, Value>,
    key: &str,
    errors: &mut Vec<Value>,
) -> Option<String> {
    let path = format!("$.{key}");
    match input.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        Some(Value::String(_)) | None | Some(Value::Null) => {
            errors.push(error(
                "AGENTMESH_FIELD_REQUIRED",
                "missing_field",
                Some(path),
                format!("{key} is required"),
            ));
            None
        }
        Some(_) => {
            errors.push(error(
                "AGENTMESH_INPUT_SCHEMA_INVALID",
                "input_schema_invalid",
                Some(path),
                format!("{key} must be a string"),
            ));
            None
        }
    }
}

fn parse_source(
    input: &Map<String, Value>,
    errors: &mut Vec<Value>,
) -> (&'static str, Map<String, Value>) {
    let has_markdown = input.contains_key("markdown");
    let has_request = input.contains_key("request");
    if has_markdown == has_request {
        errors.push(error(
            "AGENTMESH_UNSUPPORTED_REQUEST_SHAPE",
            "unsupported_request_shape",
            Some("$"),
            "provide exactly one of markdown or request",
        ));
        return ("unsupported", Map::new());
    }

    if has_markdown {
        return match input.get("markdown") {
            Some(Value::String(markdown)) => parse_markdown_source(markdown, errors),
            _ => {
                errors.push(error(
                    "AGENTMESH_INPUT_SCHEMA_INVALID",
                    "input_schema_invalid",
                    Some("$.markdown"),
                    "markdown must be a string",
                ));
                ("markdown", Map::new())
            }
        };
    }

    match input.get("request") {
        Some(Value::Object(request)) => ("request", request.clone()),
        _ => {
            errors.push(error(
                "AGENTMESH_UNSUPPORTED_REQUEST_SHAPE",
                "unsupported_request_shape",
                Some("$.request"),
                "request must be a JSON object",
            ));
            ("request", Map::new())
        }
    }
}

fn parse_markdown_source(
    markdown: &str,
    errors: &mut Vec<Value>,
) -> (&'static str, Map<String, Value>) {
    if markdown.len() > MAX_SOURCE_BYTES {
        errors.push(error(
            "AGENTMESH_INPUT_SCHEMA_INVALID",
            "input_schema_invalid",
            Some("$.markdown"),
            format!(
                "markdown is {} bytes; limit is {MAX_SOURCE_BYTES}",
                markdown.len()
            ),
        ));
    }

    let normalized = markdown.replace("\r\n", "\n");
    let Some(frontmatter) = parse_frontmatter(&normalized) else {
        errors.push(error(
            "AGENTMESH_UNSUPPORTED_REQUEST_SHAPE",
            "unsupported_request_shape",
            Some("$.markdown"),
            "markdown requests require a YAML frontmatter block",
        ));
        return ("markdown", Map::new());
    };

    ("markdown", parse_frontmatter_fields(frontmatter, errors))
}

fn parse_frontmatter(markdown: &str) -> Option<&str> {
    let rest = markdown.strip_prefix("---\n")?;
    if let Some(end) = rest.find("\n---\n") {
        return Some(&rest[..end]);
    }

    let end = rest.strip_suffix("\n---")?.len();
    Some(&rest[..end])
}

fn parse_frontmatter_fields(frontmatter: &str, errors: &mut Vec<Value>) -> Map<String, Value> {
    let mut fields = Map::new();
    for (line_index, line) in frontmatter.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, raw)) = trimmed.split_once(':') else {
            errors.push(error(
                "AGENTMESH_FRONTMATTER_VALUE_INVALID",
                "invalid_frontmatter_value",
                Some(format!("$.markdown.frontmatter.line{}", line_index + 1)),
                "frontmatter entries must use key: value syntax",
            ));
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            errors.push(error(
                "AGENTMESH_FRONTMATTER_VALUE_INVALID",
                "invalid_frontmatter_value",
                Some(format!("$.markdown.frontmatter.line{}", line_index + 1)),
                "frontmatter key must not be empty",
            ));
            continue;
        }
        let (value, parse_error) = scalar(raw.trim());
        if let Some(message) = parse_error {
            errors.push(error(
                "AGENTMESH_FRONTMATTER_VALUE_INVALID",
                "invalid_frontmatter_value",
                Some(format!("$.markdown.frontmatter.{key}")),
                message,
            ));
        }
        fields.insert(key.to_string(), value);
    }
    fields
}

fn scalar(raw: &str) -> (Value, Option<String>) {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        return (
            Value::String(trimmed[1..trimmed.len() - 1].to_string()),
            None,
        );
    }
    if trimmed.starts_with('[') {
        return match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => (value, None),
            Err(err) => (
                Value::String(trimmed.to_string()),
                Some(format!(
                    "frontmatter array value is not valid JSON (offset line {}, column {})",
                    err.line(),
                    err.column()
                )),
            ),
        };
    }
    match trimmed {
        "true" => (Value::Bool(true), None),
        "false" => (Value::Bool(false), None),
        _ => trimmed.parse::<u64>().map_or_else(
            |_| (Value::String(trimmed.to_string()), None),
            |n| (json!(n), None),
        ),
    }
}

fn validate_frontmatter(fields: &Map<String, Value>, source_shape: &str, errors: &mut Vec<Value>) {
    for spec in FRONTMATTER_SPECS {
        validate_field(spec, fields, source_shape, errors);
    }
    validate_request_kind(fields, source_shape, errors);
    validate_sequence(fields, source_shape, errors);
}

fn validate_field(
    spec: &FieldSpec,
    fields: &Map<String, Value>,
    source_shape: &str,
    errors: &mut Vec<Value>,
) {
    let path = field_path(source_shape, spec.key);
    match fields.get(spec.key) {
        None | Some(Value::Null) => {
            if spec.required {
                errors.push(error(
                    "AGENTMESH_FIELD_REQUIRED",
                    "missing_field",
                    Some(path),
                    format!("frontmatter field {} is required", spec.key),
                ));
            }
        }
        Some(value) => validate_field_value(spec, value, &path, errors),
    }
}

fn validate_field_value(spec: &FieldSpec, value: &Value, path: &str, errors: &mut Vec<Value>) {
    let invalid = match spec.kind {
        FieldKind::String => value
            .as_str()
            .is_none_or(|text| spec.required && text.trim().is_empty()),
        FieldKind::Bool => !value.is_boolean(),
        FieldKind::StringArray => !value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string)),
        FieldKind::PositiveInteger => !value.as_u64().is_some_and(|n| n > 0),
    };
    if invalid {
        errors.push(error(
            "AGENTMESH_FRONTMATTER_VALUE_INVALID",
            "invalid_frontmatter_value",
            Some(path),
            format!(
                "frontmatter field {} must be {}",
                spec.key,
                field_kind_name(spec.kind)
            ),
        ));
    }
}

fn validate_request_kind(fields: &Map<String, Value>, source_shape: &str, errors: &mut Vec<Value>) {
    let Some(request_kind) = fields.get("request_kind").and_then(Value::as_str) else {
        return;
    };
    if request_kind.trim().is_empty() || ACCEPTED_REQUEST_KINDS.contains(&request_kind) {
        return;
    }
    errors.push(error(
        "AGENTMESH_UNSUPPORTED_REQUEST_SHAPE",
        "unsupported_request_shape",
        Some(field_path(source_shape, "request_kind")),
        "request_kind must be one of app, repair",
    ));
}

fn validate_sequence(fields: &Map<String, Value>, source_shape: &str, errors: &mut Vec<Value>) {
    let sequence_index = fields.get("sequence_index").and_then(Value::as_u64);
    let sequence_total = fields.get("sequence_total").and_then(Value::as_u64);
    let (Some(sequence_index), Some(sequence_total)) = (sequence_index, sequence_total) else {
        return;
    };
    if sequence_index > sequence_total {
        errors.push(error(
            "AGENTMESH_FRONTMATTER_VALUE_INVALID",
            "invalid_frontmatter_value",
            Some(field_path(source_shape, "sequence_index")),
            "sequence_index must be less than or equal to sequence_total",
        ));
    }
}

fn field_kind_name(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::String => "a non-empty string",
        FieldKind::Bool => "a boolean",
        FieldKind::StringArray => "an array of strings",
        FieldKind::PositiveInteger => "a positive integer",
    }
}

fn field_path(source_shape: &str, key: &str) -> String {
    match source_shape {
        "markdown" => format!("$.markdown.frontmatter.{key}"),
        "request" => format!("$.request.{key}"),
        _ => format!("$.{key}"),
    }
}

fn normalized_frontmatter(fields: &Map<String, Value>) -> Map<String, Value> {
    let mut normalized = Map::new();
    for key in CANONICAL_FRONTMATTER_KEYS {
        normalized.insert(
            (*key).to_string(),
            fields.get(*key).cloned().unwrap_or(Value::Null),
        );
    }
    for (key, value) in fields {
        if !normalized.contains_key(key) {
            normalized.insert(key.clone(), value.clone());
        }
    }
    normalized
}

fn compact(
    request_id: Option<String>,
    scope: Option<String>,
    target_app: Option<String>,
    source_shape: &'static str,
    frontmatter_fields: Map<String, Value>,
    errors: Vec<Value>,
) -> Value {
    let valid = errors.is_empty();
    let evidence = evidence(
        request_id.clone(),
        scope.clone(),
        target_app.clone(),
        source_shape,
        frontmatter_fields,
        &errors,
    );
    let preview_markdown = preview_markdown(
        &request_id,
        &scope,
        &target_app,
        source_shape,
        &evidence,
        &errors,
    );
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "summary_version": SUMMARY_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "valid": valid,
        "request_id": request_id,
        "scope": scope,
        "target_app": target_app,
        "source_shape": source_shape,
        "preview_markdown": preview_markdown,
        "evidence": evidence,
        "error_count": errors.len(),
        "errors": errors,
    })
}

fn evidence(
    request_id: Option<String>,
    scope: Option<String>,
    target_app: Option<String>,
    source_shape: &str,
    frontmatter_fields: Map<String, Value>,
    errors: &[Value],
) -> Value {
    let valid = errors.is_empty();
    let error_codes = error_codes(errors);
    json!({
        "schema_version": EVIDENCE_SCHEMA_VERSION,
        "summary_version": SUMMARY_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "serialization": {
            "format": "json",
            "object_key_order": "lexicographic",
            "array_order": "contract-defined",
            "markdown_section_order": [
                "Request Dry-run Summary",
                "Parsed Frontmatter",
                "Validation",
                "Errors",
                "Normalized JSON Evidence"
            ]
        },
        "request_id": request_id,
        "scope": scope,
        "target_app": target_app,
        "source_shape": source_shape,
        "frontmatter_fields": frontmatter_fields,
        "validation": {
            "valid": valid,
            "status": if valid { "valid" } else { "invalid" },
            "error_count": errors.len(),
            "error_codes": error_codes,
        },
        "errors": errors.to_vec(),
    })
}

fn preview_markdown(
    request_id: &Option<String>,
    scope: &Option<String>,
    target_app: &Option<String>,
    source_shape: &str,
    evidence: &Value,
    errors: &[Value],
) -> String {
    let valid = errors.is_empty();
    let validation_status = if valid { "valid" } else { "invalid" };
    let mut out = String::new();
    writeln!(out, "## Request Dry-run Summary").expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(out, "- request_id: {}", inline_json(&json!(request_id))).expect("write markdown");
    writeln!(out, "- schema_version: `{REQUEST_SCHEMA_VERSION}`").expect("write markdown");
    writeln!(out, "- scope: {}", inline_json(&json!(scope))).expect("write markdown");
    writeln!(out, "- target_app: {}", inline_json(&json!(target_app))).expect("write markdown");
    writeln!(out, "- source_shape: `{source_shape}`").expect("write markdown");
    writeln!(out, "- validation_status: `{validation_status}`").expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(out, "## Parsed Frontmatter").expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(out, "| Field | Value |").expect("write markdown");
    writeln!(out, "| --- | --- |").expect("write markdown");
    for (key, value) in evidence["frontmatter_fields"]
        .as_object()
        .expect("frontmatter object")
    {
        writeln!(
            out,
            "| {} | {} |",
            markdown_cell(key),
            markdown_cell(json_compact(value))
        )
        .expect("write markdown");
    }
    writeln!(out).expect("write markdown");
    writeln!(out, "## Validation").expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(out, "- valid: `{valid}`").expect("write markdown");
    writeln!(out, "- error_count: `{}`", errors.len()).expect("write markdown");
    writeln!(
        out,
        "- error_codes: {}",
        inline_json(&json!(error_codes(errors)))
    )
    .expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(out, "## Errors").expect("write markdown");
    writeln!(out).expect("write markdown");
    if errors.is_empty() {
        writeln!(out, "- none").expect("write markdown");
    } else {
        writeln!(out, "| Code | Category | Path | Message |").expect("write markdown");
        writeln!(out, "| --- | --- | --- | --- |").expect("write markdown");
        for record in errors {
            writeln!(
                out,
                "| {} | {} | {} | {} |",
                markdown_cell(record.get("code").and_then(Value::as_str).unwrap_or("")),
                markdown_cell(record.get("category").and_then(Value::as_str).unwrap_or("")),
                markdown_cell(record.get("path").and_then(Value::as_str).unwrap_or("")),
                markdown_cell(record.get("message").and_then(Value::as_str).unwrap_or("")),
            )
            .expect("write markdown");
        }
    }
    writeln!(out).expect("write markdown");
    writeln!(out, "## Normalized JSON Evidence").expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(out, "```json").expect("write markdown");
    write!(
        out,
        "{}",
        serde_json::to_string_pretty(evidence).expect("serialize evidence")
    )
    .expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(out, "```").expect("write markdown");
    out
}

fn error(
    code: &str,
    category: &str,
    path: Option<impl Into<String>>,
    message: impl Into<String>,
) -> Value {
    json!({
        "code": code,
        "category": category,
        "severity": "error",
        "path": path.map(Into::into),
        "message": message.into(),
    })
}

fn error_codes(errors: &[Value]) -> Vec<String> {
    errors
        .iter()
        .filter_map(|error| {
            error
                .get("code")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect()
}

fn json_compact(value: &Value) -> String {
    serde_json::to_string(value).expect("serialize value")
}

fn inline_json(value: &Value) -> String {
    format!("`{}`", json_compact(value).replace('`', "\\`"))
}

fn markdown_cell(text: impl AsRef<str>) -> String {
    text.as_ref()
        .replace('`', "\\`")
        .replace('|', "\\|")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_input() -> Value {
        json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request_id": "DOT-1355",
            "scope": "agentmesh:app:request-dry-run-summary",
            "target_app": "request-dry-run-summary",
            "markdown": "---\ntitle: \"Add a deterministic request dry-run summary adapter app\"\nready_for_multica: true\nstatus: ready\nproject_key: agentmesh-private\nissue_type: AFK\nrequest_kind: app\nsource_prd: \"4_Project/OSS/agentmesh-private/Requests/App/2026-08-05-add-a-deterministic-request-dry-run-summary-adapter-app.md\"\nsource_design: 4_Project/OSS/agentmesh-private/Docs/agentmesh-request-operations-v1.md\nsource_roadmap: 4_Project/OSS/agentmesh-private/ROADMAP.md\nsequence_index: 1\nsequence_total: 1\nblocked_by: []\nunblocks: []\n---\n# Add a deterministic request dry-run summary adapter app\n\n## What to build\nBuild it.\n\n## Acceptance criteria\n- Preview is deterministic.\n"
        })
    }

    #[test]
    fn valid_summary_contains_markdown_preview_and_evidence() {
        let output = summarize_request_dry_run(&valid_input());
        assert_eq!(output["schema_version"], OUTPUT_SCHEMA_VERSION);
        assert_eq!(output["valid"], true);
        assert_eq!(output["error_count"], 0);
        assert_eq!(output["evidence"]["request_id"], "DOT-1355");
        assert_eq!(
            output["evidence"]["frontmatter_fields"]["request_kind"],
            "app"
        );
        let preview = output["preview_markdown"].as_str().unwrap();
        assert!(preview.contains("## Request Dry-run Summary"));
        assert!(preview.contains("## Normalized JSON Evidence"));
    }

    #[test]
    fn unsupported_shape_uses_deterministic_error_code() {
        let output = summarize_request_dry_run(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request_id": "DOT-1355",
            "scope": "agentmesh:app:request-dry-run-summary",
            "target_app": "request-dry-run-summary"
        }));
        assert_eq!(output["valid"], false);
        assert_eq!(
            output["errors"][0]["code"],
            "AGENTMESH_UNSUPPORTED_REQUEST_SHAPE"
        );
    }

    #[test]
    fn invalid_frontmatter_values_are_categorized() {
        let output = summarize_request_dry_run(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request_id": "DOT-1355",
            "scope": "agentmesh:app:request-dry-run-summary",
            "target_app": "request-dry-run-summary",
            "request": {
                "title": "Bad request",
                "request_kind": "app",
                "issue_type": "AFK",
                "status": "ready",
                "project_key": "agentmesh-private",
                "source_prd": "Requests/App/bad.md",
                "source_design": "Docs/design.md",
                "source_roadmap": "ROADMAP.md",
                "blocked_by": [1],
                "unblocks": [],
                "sequence_index": 2,
                "sequence_total": 1
            }
        }));
        assert_eq!(output["valid"], false);
        assert_eq!(
            output["errors"][0]["code"],
            "AGENTMESH_FRONTMATTER_VALUE_INVALID"
        );
        assert_eq!(
            output["errors"][1]["message"],
            "sequence_index must be less than or equal to sequence_total"
        );
    }

    #[test]
    fn quoted_scalars_remain_strings() {
        let output = summarize_request_dry_run(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request_id": "DOT-1355",
            "scope": "agentmesh:app:request-dry-run-summary",
            "target_app": "request-dry-run-summary",
            "markdown": "---\ntitle: Example\nready_for_multica: \"true\"\nstatus: ready\nproject_key: \"123\"\nrequest_kind: app\nissue_type: AFK\nsource_prd: docs/prd.md\nsource_design: docs/design.md\nsource_roadmap: docs/roadmap.md\nblocked_by: []\nunblocks: []\nsequence_index: 1\nsequence_total: 1\n---"
        }));

        assert_eq!(
            output["evidence"]["frontmatter_fields"]["ready_for_multica"],
            "true"
        );
        assert_eq!(
            output["evidence"]["frontmatter_fields"]["project_key"],
            "123"
        );
    }

    #[test]
    fn frontmatter_may_end_without_trailing_newline() {
        let mut input = valid_input();
        let markdown = input["markdown"].as_str().unwrap();
        let frontmatter = markdown
            .split_once("\n---\n")
            .expect("valid input frontmatter")
            .0;
        input["markdown"] = Value::String(format!("{frontmatter}\n---"));

        let output = summarize_request_dry_run(&input);
        assert_eq!(output["errors"].as_array().unwrap().len(), 0);
        assert_eq!(output["valid"], true);
    }

    #[test]
    fn recorded_fixtures_match_expected_payloads() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "request_dry_run_success_input.json",
                "expected_request_dry_run_success_payload.json",
            ),
            (
                "request_dry_run_missing_fields_input.json",
                "expected_request_dry_run_missing_fields_payload.json",
            ),
            (
                "request_dry_run_invalid_frontmatter_input.json",
                "expected_request_dry_run_invalid_frontmatter_payload.json",
            ),
            (
                "request_dry_run_unsupported_shape_input.json",
                "expected_request_dry_run_unsupported_shape_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            assert_eq!(
                summarize_request_dry_run(&input),
                expected,
                "{input_name} should match {expected_name}"
            );
        }
    }
}
