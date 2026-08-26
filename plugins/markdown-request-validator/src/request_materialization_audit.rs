//! Deterministic request materialization and same-scope dedupe audit App.
//!
//! The audit accepts validated AgentMesh request sources or retained canonical
//! summaries, projects only stable request fields into a common materialization,
//! fingerprints adapter-owned details separately, and groups valid inputs by
//! stable scope so local/non-Multica runners can suppress equivalent duplicates
//! while surfacing conflicting same-scope edits.

use agentmesh_evidence::sha256_prefixed;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Plugin/schema version exposed in compact output.
pub const MATERIALIZATION_AUDIT_VERSION: &str = "request-materialization-audit.v0";
const INPUT_SCHEMA_VERSION: &str = "request-materialization-audit-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "request-materialization-audit-compact.v0";
const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";
const MATERIALIZATION_SCHEMA_VERSION: &str = "agentmesh-request-materialization.v0";
const MAX_SOURCE_BYTES: usize = 64 * 1024;

const COMMON_FIELD_ORDER: &[&str] = &[
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

const REQUIRED_FIELDS: &[&str] = &[
    "title",
    "request_kind",
    "issue_type",
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

const ACCEPTED_REQUEST_KINDS: &[&str] = &["app", "repair"];

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

const FIELD_SPECS: &[FieldSpec] = &[
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
];

const fn field_spec(key: &'static str, kind: FieldKind, required: bool) -> FieldSpec {
    FieldSpec {
        key,
        kind,
        required,
    }
}

#[derive(Debug)]
struct ParsedSource {
    shape: &'static str,
    fields: Map<String, Value>,
    source_sha256: Option<String>,
}

#[derive(Debug)]
struct SourceAudit {
    input_index: usize,
    source_key: String,
    source_id: Option<String>,
    request_id: Option<String>,
    scope: Option<String>,
    target_app: Option<String>,
    shape: &'static str,
    source_sha256: Option<String>,
    common_materialization: Option<Value>,
    common_fields_sha256: Option<String>,
    materialization_sha256: Option<String>,
    adapter_specific: Value,
    errors: Vec<Value>,
}

impl SourceAudit {
    fn valid(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Produce a deterministic compact materialization and dedupe audit.
pub fn audit_request_materialization(value: &Value) -> Value {
    let mut errors = Vec::new();
    let Some(input) = value.as_object() else {
        errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_INPUT_SCHEMA_INVALID",
            "input_schema_invalid",
            Some("$"),
            "input must be a JSON object",
        ));
        return compact(Vec::new(), Vec::new(), errors);
    };

    let schema_version = required_string(input, "schema_version", "$", &mut errors);
    if let Some(version) = &schema_version {
        if version != INPUT_SCHEMA_VERSION {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_UNKNOWN_SCHEMA_VERSION",
                "unknown_schema_version",
                Some("$.schema_version"),
                format!("schema_version must be {INPUT_SCHEMA_VERSION}"),
            ));
        }
    }

    let request_schema_version = optional_string(
        input,
        "request_schema_version",
        "$.request_schema_version",
        &mut errors,
    );
    if let Some(version) = &request_schema_version {
        if version != REQUEST_SCHEMA_VERSION {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_UNKNOWN_SCHEMA_VERSION",
                "unknown_schema_version",
                Some("$.request_schema_version"),
                format!("request_schema_version must be {REQUEST_SCHEMA_VERSION}"),
            ));
        }
    }

    let mut sources = Vec::new();
    match input.get("requests") {
        Some(Value::Array(items)) if !items.is_empty() => {
            for (index, item) in items.iter().enumerate() {
                let source = audit_source(index, item);
                errors.extend(source.errors.clone());
                sources.push(source);
            }
        }
        Some(Value::Array(_)) => errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FIELD_REQUIRED",
            "missing_field",
            Some("$.requests"),
            "requests must contain at least one source",
        )),
        Some(_) => errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_INPUT_SCHEMA_INVALID",
            "input_schema_invalid",
            Some("$.requests"),
            "requests must be an array",
        )),
        None => errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FIELD_REQUIRED",
            "missing_field",
            Some("$.requests"),
            "requests is required",
        )),
    }

    let (scope_groups, conflict_errors) = scope_groups(&sources);
    errors.extend(conflict_errors);
    compact(sources, scope_groups, errors)
}

fn audit_source(index: usize, value: &Value) -> SourceAudit {
    let source_key = format!("source-{index:04}", index = index + 1);
    let mut errors = Vec::new();
    let path = format!("$.requests[{index}]");
    let Some(input) = value.as_object() else {
        errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_INPUT_SCHEMA_INVALID",
            "input_schema_invalid",
            Some(path),
            "request source must be a JSON object",
        ));
        return invalid_source(index, source_key, errors);
    };

    let source_id = optional_string(
        input,
        "source_id",
        &format!("{path}.source_id"),
        &mut errors,
    );
    let request_id = optional_string(
        input,
        "request_id",
        &format!("{path}.request_id"),
        &mut errors,
    );
    let scope = required_string(input, "scope", &path, &mut errors);
    let target_app = required_string(input, "target_app", &path, &mut errors);

    let parsed = parse_source(input, &path, &mut errors);
    if parsed.shape != "unsupported" {
        validate_fields(&parsed.fields, parsed.shape, &path, &mut errors);
    }

    let canonical_fields = canonical_fields(&parsed.fields);
    let common_fields_sha256 = sha256_prefixed(&canonical_json_bytes(&canonical_fields));
    let common_materialization =
        common_materialization(&scope, &target_app, canonical_fields.clone());
    let materialization_sha256 = sha256_prefixed(&canonical_json_bytes(&common_materialization));
    let adapter_specific = adapter_specific_summary(input, &parsed.fields, &path, &mut errors);

    let valid = errors.is_empty();
    SourceAudit {
        input_index: index,
        source_key,
        source_id,
        request_id,
        scope,
        target_app,
        shape: parsed.shape,
        source_sha256: parsed.source_sha256,
        common_materialization: valid.then_some(common_materialization),
        common_fields_sha256: valid.then_some(common_fields_sha256),
        materialization_sha256: valid.then_some(materialization_sha256),
        adapter_specific,
        errors,
    }
}

fn invalid_source(index: usize, source_key: String, errors: Vec<Value>) -> SourceAudit {
    SourceAudit {
        input_index: index,
        source_key,
        source_id: None,
        request_id: None,
        scope: None,
        target_app: None,
        shape: "unsupported",
        source_sha256: None,
        common_materialization: None,
        common_fields_sha256: None,
        materialization_sha256: None,
        adapter_specific: empty_adapter_specific(),
        errors,
    }
}

fn required_string(
    input: &Map<String, Value>,
    key: &str,
    base_path: &str,
    errors: &mut Vec<Value>,
) -> Option<String> {
    let path = format!("{base_path}.{key}");
    match input.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        Some(Value::String(_)) | None | Some(Value::Null) => {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FIELD_REQUIRED",
                "missing_field",
                Some(path),
                format!("{key} is required"),
            ));
            None
        }
        Some(_) => {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_INPUT_SCHEMA_INVALID",
                "input_schema_invalid",
                Some(path),
                format!("{key} must be a string"),
            ));
            None
        }
    }
}

fn optional_string(
    input: &Map<String, Value>,
    key: &str,
    path: &str,
    errors: &mut Vec<Value>,
) -> Option<String> {
    match input.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        Some(Value::String(_)) | Some(Value::Null) => {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_INPUT_SCHEMA_INVALID",
                "input_schema_invalid",
                Some(path.to_string()),
                format!("{key} must be a non-empty string when provided"),
            ));
            None
        }
        Some(_) => {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_INPUT_SCHEMA_INVALID",
                "input_schema_invalid",
                Some(path.to_string()),
                format!("{key} must be a string"),
            ));
            None
        }
        None => None,
    }
}

fn parse_source(input: &Map<String, Value>, path: &str, errors: &mut Vec<Value>) -> ParsedSource {
    let shape_keys = ["markdown", "request", "canonical", "summary"];
    let present_shapes: Vec<_> = shape_keys
        .iter()
        .filter(|key| input.contains_key(**key))
        .collect();
    if present_shapes.len() != 1 {
        errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_UNSUPPORTED_SHAPE",
            "unsupported_request_shape",
            Some(path.to_string()),
            "provide exactly one of markdown, request, canonical, or summary",
        ));
        return unsupported_source();
    }

    match *present_shapes[0] {
        "markdown" => match input.get("markdown") {
            Some(Value::String(markdown)) => parse_markdown_source(markdown, path, errors),
            _ => {
                errors.push(error(
                    "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_INPUT_SCHEMA_INVALID",
                    "input_schema_invalid",
                    Some(format!("{path}.markdown")),
                    "markdown must be a string",
                ));
                ParsedSource {
                    shape: "markdown",
                    fields: Map::new(),
                    source_sha256: None,
                }
            }
        },
        "request" => parse_object_source(input, "request", path, errors),
        "canonical" => parse_object_source(input, "canonical", path, errors),
        "summary" => parse_summary_source(input, path, errors),
        _ => unsupported_source(),
    }
}

fn unsupported_source() -> ParsedSource {
    ParsedSource {
        shape: "unsupported",
        fields: Map::new(),
        source_sha256: None,
    }
}

fn parse_object_source(
    input: &Map<String, Value>,
    key: &'static str,
    path: &str,
    errors: &mut Vec<Value>,
) -> ParsedSource {
    match input.get(key) {
        Some(Value::Object(request)) => ParsedSource {
            shape: if key == "request" {
                "request"
            } else {
                "canonical_summary"
            },
            fields: request.clone(),
            source_sha256: Some(sha256_prefixed(&canonical_json_bytes(&Value::Object(
                request.clone(),
            )))),
        },
        _ => {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_UNSUPPORTED_SHAPE",
                "unsupported_request_shape",
                Some(format!("{path}.{key}")),
                format!("{key} must be a JSON object"),
            ));
            ParsedSource {
                shape: if key == "request" {
                    "request"
                } else {
                    "canonical_summary"
                },
                fields: Map::new(),
                source_sha256: None,
            }
        }
    }
}

fn parse_summary_source(
    input: &Map<String, Value>,
    path: &str,
    errors: &mut Vec<Value>,
) -> ParsedSource {
    let Some(Value::Object(summary)) = input.get("summary") else {
        errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_UNSUPPORTED_SHAPE",
            "unsupported_request_shape",
            Some(format!("{path}.summary")),
            "summary must be a JSON object",
        ));
        return ParsedSource {
            shape: "canonical_summary",
            fields: Map::new(),
            source_sha256: None,
        };
    };

    match summary.get("valid") {
        Some(Value::Bool(true)) => {}
        Some(Value::Bool(false)) => errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_SUMMARY_INVALID",
            "invalid_summary",
            Some(format!("{path}.summary.valid")),
            "summary.valid must be true before materialization audit",
        )),
        Some(_) => errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_INPUT_SCHEMA_INVALID",
            "input_schema_invalid",
            Some(format!("{path}.summary.valid")),
            "summary.valid must be a boolean",
        )),
        None => errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FIELD_REQUIRED",
            "missing_field",
            Some(format!("{path}.summary.valid")),
            "summary.valid is required",
        )),
    }

    let fields = summary
        .get("canonical")
        .and_then(Value::as_object)
        .or_else(|| {
            summary
                .get("manifest_json")
                .and_then(Value::as_object)
                .and_then(|manifest| manifest.get("canonical_fields"))
                .and_then(Value::as_object)
        })
        .or_else(|| {
            summary
                .get("evidence")
                .and_then(Value::as_object)
                .and_then(|evidence| evidence.get("frontmatter_fields"))
                .and_then(Value::as_object)
        });

    let fields = match fields {
        Some(fields) => fields.clone(),
        None => {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_UNSUPPORTED_SHAPE",
                "unsupported_request_shape",
                Some(format!("{path}.summary")),
                "summary must contain canonical, manifest_json.canonical_fields, or evidence.frontmatter_fields",
            ));
            Map::new()
        }
    };

    ParsedSource {
        shape: "canonical_summary",
        fields,
        source_sha256: Some(sha256_prefixed(&canonical_json_bytes(&Value::Object(
            summary.clone(),
        )))),
    }
}

fn parse_markdown_source(markdown: &str, path: &str, errors: &mut Vec<Value>) -> ParsedSource {
    if markdown.len() > MAX_SOURCE_BYTES {
        errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_INPUT_SCHEMA_INVALID",
            "input_schema_invalid",
            Some(format!("{path}.markdown")),
            format!(
                "markdown is {} bytes; limit is {MAX_SOURCE_BYTES}",
                markdown.len()
            ),
        ));
    }

    let normalized = markdown.replace("\r\n", "\n");
    let source_sha256 = Some(sha256_prefixed(normalized.as_bytes()));
    let Some(frontmatter) = parse_frontmatter(&normalized) else {
        errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FRONTMATTER_MALFORMED",
            "malformed_frontmatter",
            Some(format!("{path}.markdown")),
            "markdown requests require a complete YAML frontmatter block",
        ));
        return ParsedSource {
            shape: "markdown",
            fields: Map::new(),
            source_sha256,
        };
    };

    ParsedSource {
        shape: "markdown",
        fields: parse_frontmatter_fields(frontmatter, path, errors),
        source_sha256,
    }
}

fn parse_frontmatter(markdown: &str) -> Option<&str> {
    let rest = markdown.strip_prefix("---\n")?;
    if let Some(end) = rest.find("\n---\n") {
        return Some(&rest[..end]);
    }

    let end = rest.strip_suffix("\n---")?.len();
    Some(&rest[..end])
}

fn parse_frontmatter_fields(
    frontmatter: &str,
    path: &str,
    errors: &mut Vec<Value>,
) -> Map<String, Value> {
    let mut fields = Map::new();
    let mut seen = BTreeSet::new();
    for (line_index, line) in frontmatter.lines().enumerate() {
        let line_number = line_index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, raw)) = trimmed.split_once(':') else {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FRONTMATTER_MALFORMED",
                "malformed_frontmatter",
                Some(format!("{path}.markdown.frontmatter.line{line_number}")),
                "frontmatter entries must use key: value syntax",
            ));
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FRONTMATTER_MALFORMED",
                "malformed_frontmatter",
                Some(format!("{path}.markdown.frontmatter.line{line_number}")),
                "frontmatter key must not be empty",
            ));
            continue;
        }
        if !seen.insert(key.to_string()) {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FIELD_DUPLICATE",
                "duplicate_field",
                Some(format!("{path}.markdown.frontmatter.{key}")),
                format!("frontmatter field {key} appears more than once"),
            ));
            continue;
        }
        let (value, parse_error) = scalar(raw.trim());
        if let Some(message) = parse_error {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FRONTMATTER_MALFORMED",
                "malformed_frontmatter",
                Some(format!("{path}.markdown.frontmatter.{key}")),
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
                    "frontmatter array value is not valid JSON (at line {}, column {})",
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

fn validate_fields(
    fields: &Map<String, Value>,
    source_shape: &str,
    path: &str,
    errors: &mut Vec<Value>,
) {
    for spec in FIELD_SPECS {
        validate_field(spec, fields, source_shape, path, errors);
    }
    validate_request_kind(fields, source_shape, path, errors);
    validate_sequence(fields, source_shape, path, errors);
}

fn validate_field(
    spec: &FieldSpec,
    fields: &Map<String, Value>,
    source_shape: &str,
    path: &str,
    errors: &mut Vec<Value>,
) {
    let field_path = field_path(path, source_shape, spec.key);
    match fields.get(spec.key) {
        None | Some(Value::Null) => {
            if spec.required {
                errors.push(error(
                    "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FIELD_REQUIRED",
                    "missing_field",
                    Some(field_path),
                    format!("request field {} is required", spec.key),
                ));
            }
        }
        Some(value) => validate_field_value(spec, value, &field_path, errors),
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
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FIELD_INVALID",
            "invalid_field_value",
            Some(path.to_string()),
            format!(
                "request field {} must be {}",
                spec.key,
                field_kind_name(spec.kind)
            ),
        ));
    }
}

fn validate_request_kind(
    fields: &Map<String, Value>,
    source_shape: &str,
    path: &str,
    errors: &mut Vec<Value>,
) {
    let Some(request_kind) = fields.get("request_kind").and_then(Value::as_str) else {
        return;
    };
    if ACCEPTED_REQUEST_KINDS.contains(&request_kind) {
        return;
    }
    errors.push(error(
        "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FIELD_INVALID",
        "invalid_field_value",
        Some(field_path(path, source_shape, "request_kind")),
        "request_kind must be one of app, repair",
    ));
}

fn validate_sequence(
    fields: &Map<String, Value>,
    source_shape: &str,
    path: &str,
    errors: &mut Vec<Value>,
) {
    let sequence_index = fields.get("sequence_index").and_then(Value::as_u64);
    let sequence_total = fields.get("sequence_total").and_then(Value::as_u64);
    let (Some(sequence_index), Some(sequence_total)) = (sequence_index, sequence_total) else {
        return;
    };
    if sequence_index > sequence_total {
        errors.push(error(
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FIELD_INVALID",
            "invalid_field_value",
            Some(field_path(path, source_shape, "sequence_index")),
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

fn field_path(path: &str, source_shape: &str, key: &str) -> String {
    match source_shape {
        "markdown" => format!("{path}.markdown.frontmatter.{key}"),
        "request" => format!("{path}.request.{key}"),
        "canonical_summary" => format!("{path}.canonical.{key}"),
        _ => format!("{path}.{key}"),
    }
}

fn canonical_fields(fields: &Map<String, Value>) -> Value {
    let mut canonical = Map::new();
    for key in COMMON_FIELD_ORDER {
        canonical.insert(
            (*key).to_string(),
            fields.get(*key).cloned().unwrap_or(Value::Null),
        );
    }
    Value::Object(canonical)
}

fn common_materialization(
    scope: &Option<String>,
    target_app: &Option<String>,
    canonical_fields: Value,
) -> Value {
    json!({
        "schema_version": MATERIALIZATION_SCHEMA_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "scope": scope,
        "target_app": target_app,
        "canonical_fields": canonical_fields,
    })
}

fn adapter_specific_summary(
    input: &Map<String, Value>,
    fields: &Map<String, Value>,
    path: &str,
    errors: &mut Vec<Value>,
) -> Value {
    let mut detail_keys = BTreeSet::new();
    let mut payload = Map::new();
    let mut adapter_id = None;

    let unknown_fields = unknown_fields(fields);
    if !unknown_fields.is_empty() {
        for key in &unknown_fields {
            detail_keys.insert(format!("canonical.{key}"));
        }
        let mut unknown_payload = Map::new();
        for key in unknown_fields {
            if let Some(value) = fields.get(&key) {
                unknown_payload.insert(key, value.clone());
            }
        }
        payload.insert(
            "canonical_unknown_fields".to_string(),
            Value::Object(unknown_payload),
        );
    }

    for key in ["adapter", "adapter_specific"] {
        if let Some(value) = input.get(key) {
            match value {
                Value::Object(object) => {
                    adapter_id = adapter_id.or_else(|| adapter_id_from(object));
                    for detail_key in object
                        .keys()
                        .filter(|detail_key| !is_adapter_identity_key(detail_key))
                    {
                        detail_keys.insert(format!("{key}.{detail_key}"));
                    }
                    payload.insert(key.to_string(), Value::Object(object.clone()));
                }
                _ => errors.push(error(
                    "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_INPUT_SCHEMA_INVALID",
                    "input_schema_invalid",
                    Some(format!("{path}.{key}")),
                    format!("{key} must be a JSON object when provided"),
                )),
            }
        }
    }

    if payload.is_empty() {
        return empty_adapter_specific();
    }

    json!({
        "present": true,
        "adapter_id": adapter_id,
        "detail_keys": detail_keys.into_iter().collect::<Vec<_>>(),
        "payload_sha256": sha256_prefixed(&canonical_json_bytes(&Value::Object(payload))),
        "payload_policy": "payload values are fingerprinted, not echoed, to keep adapter-owned metadata outside common audit fields",
    })
}

fn adapter_id_from(object: &Map<String, Value>) -> Option<String> {
    object
        .get("adapter_id")
        .or_else(|| object.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

fn is_adapter_identity_key(key: &str) -> bool {
    matches!(key, "adapter_id" | "id")
}

fn empty_adapter_specific() -> Value {
    json!({
        "present": false,
        "adapter_id": null,
        "detail_keys": [],
        "payload_sha256": null,
        "payload_policy": "payload values are fingerprinted, not echoed, to keep adapter-owned metadata outside common audit fields",
    })
}

fn unknown_fields(fields: &Map<String, Value>) -> Vec<String> {
    fields
        .keys()
        .filter(|key| !COMMON_FIELD_ORDER.contains(&key.as_str()))
        .cloned()
        .collect()
}

fn scope_groups(sources: &[SourceAudit]) -> (Vec<Value>, Vec<Value>) {
    let mut by_scope: BTreeMap<String, Vec<&SourceAudit>> = BTreeMap::new();
    for source in sources.iter().filter(|source| source.valid()) {
        let Some(scope) = &source.scope else {
            continue;
        };
        by_scope.entry(scope.clone()).or_default().push(source);
    }

    let mut groups = Vec::new();
    let mut errors = Vec::new();
    for (scope, mut scoped_sources) in by_scope {
        scoped_sources.sort_by(|left, right| left.source_key.cmp(&right.source_key));
        let mut by_materialization: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut source_keys = Vec::new();
        for source in scoped_sources {
            source_keys.push(source.source_key.clone());
            let materialization_sha256 = source
                .materialization_sha256
                .as_ref()
                .expect("valid source has materialization hash")
                .clone();
            by_materialization
                .entry(materialization_sha256)
                .or_default()
                .push(source.source_key.clone());
        }

        let duplicate = source_keys.len() > 1;
        let dedupe_class = match (duplicate, by_materialization.len()) {
            (false, _) => "unique",
            (true, 1) => "equivalent_duplicate",
            (true, _) => "conflicting_duplicate",
        };
        if dedupe_class == "conflicting_duplicate" {
            errors.push(error(
                "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_SCOPE_CONFLICT",
                "scope_conflict",
                Some("$.requests"),
                format!(
                    "scope {scope} has {} distinct materializations",
                    by_materialization.len()
                ),
            ));
        }

        let equivalence_sets: Vec<_> = by_materialization
            .into_iter()
            .map(|(materialization_sha256, mut source_keys)| {
                source_keys.sort();
                json!({
                    "materialization_sha256": materialization_sha256,
                    "request_count": source_keys.len(),
                    "source_keys": source_keys,
                })
            })
            .collect();

        groups.push(json!({
            "scope": scope,
            "dedupe_class": dedupe_class,
            "duplicate": duplicate,
            "request_count": source_keys.len(),
            "distinct_materialization_count": equivalence_sets.len(),
            "source_keys": source_keys,
            "equivalence_sets": equivalence_sets,
        }));
    }
    (groups, errors)
}

fn compact(sources: Vec<SourceAudit>, scope_groups: Vec<Value>, errors: Vec<Value>) -> Value {
    let summary = audit_summary(&sources, &scope_groups, &errors);
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "audit_version": MATERIALIZATION_AUDIT_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "valid": errors.is_empty(),
        "serialization": {
            "format": "json",
            "object_key_order": "lexicographic",
            "array_order": "scope/source-key/materialization-hash sorted where order is not part of input identity",
            "common_field_order": COMMON_FIELD_ORDER,
            "required_fields": REQUIRED_FIELDS,
            "hash_algorithm": "sha256",
        },
        "summary": summary,
        "common_field_order": COMMON_FIELD_ORDER,
        "sources": source_values(&sources),
        "scope_groups": scope_groups,
        "error_count": errors.len(),
        "errors": errors,
    })
}

fn audit_summary(sources: &[SourceAudit], scope_groups: &[Value], errors: &[Value]) -> Value {
    let valid_source_count = sources.iter().filter(|source| source.valid()).count();
    let duplicate_scope_count = scope_groups
        .iter()
        .filter(|group| {
            group
                .get("duplicate")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .count();
    let equivalent_duplicate_scope_count = scope_groups
        .iter()
        .filter(|group| {
            group.get("dedupe_class").and_then(Value::as_str) == Some("equivalent_duplicate")
        })
        .count();
    let conflicting_duplicate_scope_count = scope_groups
        .iter()
        .filter(|group| {
            group.get("dedupe_class").and_then(Value::as_str) == Some("conflicting_duplicate")
        })
        .count();
    let unique_scope_count = scope_groups
        .iter()
        .filter(|group| group.get("dedupe_class").and_then(Value::as_str) == Some("unique"))
        .count();

    json!({
        "status": if errors.is_empty() { "passed" } else { "failed" },
        "source_count": sources.len(),
        "valid_source_count": valid_source_count,
        "invalid_source_count": sources.len().saturating_sub(valid_source_count),
        "scope_count": scope_groups.len(),
        "unique_scope_count": unique_scope_count,
        "duplicate_scope_count": duplicate_scope_count,
        "equivalent_duplicate_scope_count": equivalent_duplicate_scope_count,
        "conflicting_duplicate_scope_count": conflicting_duplicate_scope_count,
    })
}

fn source_values(sources: &[SourceAudit]) -> Vec<Value> {
    sources
        .iter()
        .map(|source| {
            json!({
                "input_index": source.input_index,
                "source_key": source.source_key,
                "source_id": source.source_id,
                "request_id": source.request_id,
                "scope": source.scope,
                "target_app": source.target_app,
                "source_shape": source.shape,
                "valid": source.valid(),
                "source_sha256": source.source_sha256,
                "common_fields_sha256": source.common_fields_sha256,
                "materialization_sha256": source.materialization_sha256,
                "common_materialization": source.common_materialization,
                "adapter_specific": source.adapter_specific,
                "error_count": source.errors.len(),
                "errors": source.errors,
            })
        })
        .collect()
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

fn sort_json_keys(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.into_iter().collect();
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));

            let mut sorted = Map::new();
            for (key, value) in entries {
                sorted.insert(key, sort_json_keys(value));
            }
            Value::Object(sorted)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(sort_json_keys).collect()),
        value => value,
    }
}

fn canonical_json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(&sort_json_keys(value.clone())).expect("serialize canonical json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(title: &str) -> Value {
        json!({
            "title": title,
            "request_kind": "app",
            "issue_type": "AFK",
            "ready_for_multica": true,
            "status": "ready",
            "project_key": "agentmesh-private",
            "source_prd": "synthetic://requests/materialization-audit",
            "source_design": "synthetic://docs/agentmesh-request-operations-v1",
            "source_roadmap": "synthetic://roadmaps/agentmesh-private",
            "blocked_by": [],
            "unblocks": [],
            "sequence_index": 1,
            "sequence_total": 1
        })
    }

    fn markdown(title: &str) -> String {
        format!(
            "---\ntitle: \"{title}\"\nrequest_kind: app\nissue_type: AFK\nready_for_multica: true\nstatus: ready\nproject_key: agentmesh-private\nsource_prd: synthetic://requests/materialization-audit\nsource_design: synthetic://docs/agentmesh-request-operations-v1\nsource_roadmap: synthetic://roadmaps/agentmesh-private\nblocked_by: []\nunblocks: []\nsequence_index: 1\nsequence_total: 1\n---\n# {title}\n\n## What to build\nBuild it.\n"
        )
    }

    #[test]
    fn equivalent_same_scope_sources_are_stable_duplicates() {
        let input = json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request_schema_version": REQUEST_SCHEMA_VERSION,
            "requests": [
                {
                    "source_id": "markdown-runner",
                    "scope": "agentmesh:app:request-materialization-audit",
                    "target_app": "request-materialization-audit",
                    "markdown": markdown("Add a deterministic materialization audit")
                },
                {
                    "source_id": "parse-summary",
                    "scope": "agentmesh:app:request-materialization-audit",
                    "target_app": "request-materialization-audit",
                    "summary": {
                        "schema_version": "agentmesh-request-parse-output.v0",
                        "request_schema_version": REQUEST_SCHEMA_VERSION,
                        "valid": true,
                        "canonical": request("Add a deterministic materialization audit"),
                        "error_count": 0,
                        "errors": []
                    },
                    "adapter_specific": {
                        "adapter_id": "local-markdown",
                        "path": "Requests/App/a.md"
                    }
                }
            ]
        });

        let first = audit_request_materialization(&input);
        let second = audit_request_materialization(&input);
        assert_eq!(first, second);
        assert_eq!(first["valid"], true);
        assert_eq!(first["summary"]["duplicate_scope_count"], 1);
        assert_eq!(first["summary"]["equivalent_duplicate_scope_count"], 1);
        assert_eq!(
            first["scope_groups"][0]["dedupe_class"],
            "equivalent_duplicate"
        );
        assert_eq!(
            first["scope_groups"][0]["distinct_materialization_count"],
            1
        );
        assert_eq!(first["sources"][1]["adapter_specific"]["present"], true);
        assert!(first["sources"][1]["adapter_specific"]["payload_sha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
    }

    #[test]
    fn conflicting_same_scope_sources_fail_the_audit() {
        let input = json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "requests": [
                {
                    "scope": "agentmesh:app:request-materialization-audit",
                    "target_app": "request-materialization-audit",
                    "request": request("Add materialization audit")
                },
                {
                    "scope": "agentmesh:app:request-materialization-audit",
                    "target_app": "request-materialization-audit",
                    "request": request("Add conflicting materialization audit")
                }
            ]
        });

        let output = audit_request_materialization(&input);
        assert_eq!(output["valid"], false);
        assert_eq!(output["summary"]["conflicting_duplicate_scope_count"], 1);
        assert_eq!(
            output["scope_groups"][0]["dedupe_class"],
            "conflicting_duplicate"
        );
        assert_eq!(
            output["errors"][0]["code"],
            "AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_SCOPE_CONFLICT"
        );
    }

    #[test]
    fn invalid_inputs_use_normalized_errors_without_echoing_adapter_payload_values() {
        let input = json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "requests": [
                {
                    "scope": "agentmesh:app:bad",
                    "target_app": "request-materialization-audit",
                    "markdown": "---\ntitle: Bad\nrequest_kind app\n---\n# Bad\n",
                    "adapter": {
                        "adapter_id": "multica-shadow",
                        "token": "SECRET_DO_NOT_LEAK",
                        "orchestrator_metadata": {"run_id": "MUL-RUN-SECRET"}
                    }
                }
            ]
        });

        let output = audit_request_materialization(&input);
        assert_eq!(output["valid"], false);
        let rendered = serde_json::to_string(&output).unwrap();
        assert!(!rendered.contains("SECRET_DO_NOT_LEAK"));
        assert!(!rendered.contains("MUL-RUN-SECRET"));
        let codes: Vec<_> = output["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|error| error["code"].as_str().unwrap())
            .collect();
        assert!(codes.contains(&"AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FRONTMATTER_MALFORMED"));
        assert!(codes.contains(&"AGENTMESH_REQUEST_MATERIALIZATION_AUDIT_FIELD_REQUIRED"));
    }
}
