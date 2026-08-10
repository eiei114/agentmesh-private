//! Deterministic request fingerprint manifest App.
//!
//! The manifest is a JSON/Markdown hybrid for comparing `agentmesh-request.v0`
//! inputs across non-Multica adapters without parsing tracker-owned payloads.

use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// Plugin/schema version exposed in compact output.
pub const FINGERPRINT_MANIFEST_VERSION: &str = "request-fingerprint-manifest.v0";
const INPUT_SCHEMA_VERSION: &str = "request-fingerprint-manifest-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "request-fingerprint-manifest-compact.v0";
const MANIFEST_SCHEMA_VERSION: &str = "request-fingerprint-manifest-json.v0";
const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";
const GENERATED_AT: &str = "1970-01-01T00:00:00Z";
const MAX_SOURCE_BYTES: usize = 64 * 1024;

const CANONICAL_FIELD_ORDER: &[&str] = &[
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
    source_hash: Option<String>,
}

/// Build a stable request fingerprint manifest from Markdown or request JSON input.
pub fn fingerprint_request_manifest(value: &Value) -> Value {
    let mut errors = Vec::new();
    let Some(input) = value.as_object() else {
        errors.push(error(
            "AGENTMESH_REQUEST_FINGERPRINT_INPUT_SCHEMA_INVALID",
            "input_schema_invalid",
            Some("$"),
            "input must be a JSON object",
        ));
        return compact(None, None, None, unsupported_source(), errors);
    };

    let schema_version = required_string(input, "schema_version", &mut errors);
    if let Some(version) = &schema_version {
        if version != INPUT_SCHEMA_VERSION {
            errors.push(error(
                "AGENTMESH_REQUEST_FINGERPRINT_UNKNOWN_SCHEMA_VERSION",
                "unknown_schema_version",
                Some("$.schema_version"),
                format!("schema_version must be {INPUT_SCHEMA_VERSION}"),
            ));
        }
    }

    let request_schema_version = optional_string(input, "request_schema_version", &mut errors);
    if let Some(version) = &request_schema_version {
        if version != REQUEST_SCHEMA_VERSION {
            errors.push(error(
                "AGENTMESH_REQUEST_FINGERPRINT_UNKNOWN_SCHEMA_VERSION",
                "unknown_schema_version",
                Some("$.request_schema_version"),
                format!("request_schema_version must be {REQUEST_SCHEMA_VERSION}"),
            ));
        }
    }

    let request_id = required_string(input, "request_id", &mut errors);
    let scope = required_string(input, "scope", &mut errors);
    let target_app = required_string(input, "target_app", &mut errors);

    let source = parse_source(input, &mut errors);
    if source.shape != "unsupported" {
        validate_fields(&source.fields, source.shape, &mut errors);
    }

    compact(request_id, scope, target_app, source, errors)
}

fn unsupported_source() -> ParsedSource {
    ParsedSource {
        shape: "unsupported",
        fields: Map::new(),
        source_hash: None,
    }
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
                "AGENTMESH_REQUEST_FINGERPRINT_FIELD_REQUIRED",
                "missing_field",
                Some(path),
                format!("{key} is required"),
            ));
            None
        }
        Some(_) => {
            errors.push(error(
                "AGENTMESH_REQUEST_FINGERPRINT_INPUT_SCHEMA_INVALID",
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
    errors: &mut Vec<Value>,
) -> Option<String> {
    match input.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        Some(Value::String(_)) | Some(Value::Null) => {
            errors.push(error(
                "AGENTMESH_REQUEST_FINGERPRINT_INPUT_SCHEMA_INVALID",
                "input_schema_invalid",
                Some(format!("$.{key}")),
                format!("{key} must be a non-empty string when provided"),
            ));
            None
        }
        Some(_) => {
            errors.push(error(
                "AGENTMESH_REQUEST_FINGERPRINT_INPUT_SCHEMA_INVALID",
                "input_schema_invalid",
                Some(format!("$.{key}")),
                format!("{key} must be a string"),
            ));
            None
        }
        None => None,
    }
}

fn parse_source(input: &Map<String, Value>, errors: &mut Vec<Value>) -> ParsedSource {
    let has_markdown = input.contains_key("markdown");
    let has_request = input.contains_key("request");
    if has_markdown == has_request {
        errors.push(error(
            "AGENTMESH_REQUEST_FINGERPRINT_UNSUPPORTED_SHAPE",
            "unsupported_request_shape",
            Some("$"),
            "provide exactly one of markdown or request",
        ));
        return unsupported_source();
    }

    if has_markdown {
        return match input.get("markdown") {
            Some(Value::String(markdown)) => parse_markdown_source(markdown, errors),
            _ => {
                errors.push(error(
                    "AGENTMESH_REQUEST_FINGERPRINT_INPUT_SCHEMA_INVALID",
                    "input_schema_invalid",
                    Some("$.markdown"),
                    "markdown must be a string",
                ));
                ParsedSource {
                    shape: "markdown",
                    fields: Map::new(),
                    source_hash: None,
                }
            }
        };
    }

    match input.get("request") {
        Some(Value::Object(request)) => ParsedSource {
            shape: "request",
            fields: request.clone(),
            source_hash: Some(sha256_prefixed(&canonical_json_bytes(&Value::Object(
                request.clone(),
            )))),
        },
        _ => {
            errors.push(error(
                "AGENTMESH_REQUEST_FINGERPRINT_UNSUPPORTED_SHAPE",
                "unsupported_request_shape",
                Some("$.request"),
                "request must be a JSON object",
            ));
            ParsedSource {
                shape: "request",
                fields: Map::new(),
                source_hash: None,
            }
        }
    }
}

fn parse_markdown_source(markdown: &str, errors: &mut Vec<Value>) -> ParsedSource {
    if markdown.len() > MAX_SOURCE_BYTES {
        errors.push(error(
            "AGENTMESH_REQUEST_FINGERPRINT_INPUT_SCHEMA_INVALID",
            "input_schema_invalid",
            Some("$.markdown"),
            format!(
                "markdown is {} bytes; limit is {MAX_SOURCE_BYTES}",
                markdown.len()
            ),
        ));
    }

    let normalized = markdown.replace("\r\n", "\n");
    let source_hash = Some(sha256_prefixed(normalized.as_bytes()));
    let Some(frontmatter) = parse_frontmatter(&normalized) else {
        errors.push(error(
            "AGENTMESH_REQUEST_FINGERPRINT_FRONTMATTER_MALFORMED",
            "malformed_frontmatter",
            Some("$.markdown"),
            "markdown requests require a complete YAML frontmatter block",
        ));
        return ParsedSource {
            shape: "markdown",
            fields: Map::new(),
            source_hash,
        };
    };

    ParsedSource {
        shape: "markdown",
        fields: parse_frontmatter_fields(frontmatter, errors),
        source_hash,
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

fn parse_frontmatter_fields(frontmatter: &str, errors: &mut Vec<Value>) -> Map<String, Value> {
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
                "AGENTMESH_REQUEST_FINGERPRINT_FRONTMATTER_MALFORMED",
                "malformed_frontmatter",
                Some(format!("$.markdown.frontmatter.line{line_number}")),
                "frontmatter entries must use key: value syntax",
            ));
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            errors.push(error(
                "AGENTMESH_REQUEST_FINGERPRINT_FRONTMATTER_MALFORMED",
                "malformed_frontmatter",
                Some(format!("$.markdown.frontmatter.line{line_number}")),
                "frontmatter key must not be empty",
            ));
            continue;
        }
        if !seen.insert(key.to_string()) {
            errors.push(error(
                "AGENTMESH_REQUEST_FINGERPRINT_FIELD_DUPLICATE",
                "duplicate_field",
                Some(format!("$.markdown.frontmatter.{key}")),
                format!("frontmatter field {key} appears more than once"),
            ));
            continue;
        }
        let (value, parse_error) = scalar(raw.trim());
        if let Some(message) = parse_error {
            errors.push(error(
                "AGENTMESH_REQUEST_FINGERPRINT_FRONTMATTER_MALFORMED",
                "malformed_frontmatter",
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

fn validate_fields(fields: &Map<String, Value>, source_shape: &str, errors: &mut Vec<Value>) {
    for spec in FIELD_SPECS {
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
                    "AGENTMESH_REQUEST_FINGERPRINT_FIELD_REQUIRED",
                    "missing_field",
                    Some(path),
                    format!("request field {} is required", spec.key),
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
            "AGENTMESH_REQUEST_FINGERPRINT_FIELD_INVALID",
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

fn validate_request_kind(fields: &Map<String, Value>, source_shape: &str, errors: &mut Vec<Value>) {
    let Some(request_kind) = fields.get("request_kind").and_then(Value::as_str) else {
        return;
    };
    if ACCEPTED_REQUEST_KINDS.contains(&request_kind) {
        return;
    }
    errors.push(error(
        "AGENTMESH_REQUEST_FINGERPRINT_FIELD_INVALID",
        "invalid_field_value",
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
            "AGENTMESH_REQUEST_FINGERPRINT_FIELD_INVALID",
            "invalid_field_value",
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

fn compact(
    request_id: Option<String>,
    scope: Option<String>,
    target_app: Option<String>,
    source: ParsedSource,
    errors: Vec<Value>,
) -> Value {
    let canonical_fields = canonical_fields(&source.fields);
    let unknown_fields = unknown_fields(&source.fields);
    let field_hashes = field_hashes(&canonical_fields);
    let content_hashes = content_hashes(source.source_hash, &canonical_fields);
    let valid = errors.is_empty();
    let manifest_json = manifest_json(
        &request_id,
        &scope,
        &target_app,
        source.shape,
        canonical_fields,
        unknown_fields,
        field_hashes,
        content_hashes.clone(),
        &errors,
    );
    let manifest_markdown = manifest_markdown(&manifest_json);

    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "manifest_version": FINGERPRINT_MANIFEST_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "valid": valid,
        "request_id": request_id,
        "scope": scope,
        "target_app": target_app,
        "source_shape": source.shape,
        "manifest_json": manifest_json,
        "manifest_markdown": manifest_markdown,
        "content_hashes": content_hashes,
        "error_count": errors.len(),
        "errors": errors,
    })
}

fn canonical_fields(fields: &Map<String, Value>) -> Value {
    let mut canonical = Map::new();
    for key in CANONICAL_FIELD_ORDER {
        canonical.insert(
            (*key).to_string(),
            fields.get(*key).cloned().unwrap_or(Value::Null),
        );
    }
    Value::Object(canonical)
}

fn unknown_fields(fields: &Map<String, Value>) -> Vec<String> {
    fields
        .keys()
        .filter(|key| !CANONICAL_FIELD_ORDER.contains(&key.as_str()))
        .cloned()
        .collect()
}

fn field_hashes(canonical_fields: &Value) -> Value {
    let fields = canonical_fields
        .as_object()
        .expect("canonical fields are object");
    Value::Array(
        CANONICAL_FIELD_ORDER
            .iter()
            .map(|key| {
                let value = fields.get(*key).expect("canonical key present");
                json!({
                    "key": key,
                    "value_sha256": sha256_prefixed(&canonical_json_bytes(value)),
                })
            })
            .collect(),
    )
}

fn content_hashes(source_hash: Option<String>, canonical_fields: &Value) -> Value {
    json!({
        "algorithm": "sha256",
        "source_sha256": source_hash,
        "canonical_fields_sha256": sha256_prefixed(&canonical_json_bytes(canonical_fields)),
    })
}

#[allow(clippy::too_many_arguments)]
fn manifest_json(
    request_id: &Option<String>,
    scope: &Option<String>,
    target_app: &Option<String>,
    source_shape: &str,
    canonical_fields: Value,
    unknown_fields: Vec<String>,
    field_hashes: Value,
    content_hashes: Value,
    errors: &[Value],
) -> Value {
    let valid = errors.is_empty();
    json!({
        "schema_version": MANIFEST_SCHEMA_VERSION,
        "manifest_version": FINGERPRINT_MANIFEST_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "generated_at": GENERATED_AT,
        "request": {
            "request_id": request_id,
            "scope": scope,
            "target_app": target_app,
            "source_shape": source_shape,
        },
        "serialization": {
            "format": "json_markdown_hybrid",
            "object_key_order": "lexicographic",
            "array_order": "contract-defined",
            "canonical_field_order": CANONICAL_FIELD_ORDER,
            "required_fields": REQUIRED_FIELDS,
            "timestamp_policy": "generated_at is fixed for deterministic fixtures",
            "hash_algorithm": "sha256",
        },
        "canonical_fields": canonical_fields,
        "unknown_fields": unknown_fields,
        "field_hashes": field_hashes,
        "content_hashes": content_hashes,
        "validation": {
            "valid": valid,
            "status": if valid { "valid" } else { "invalid" },
            "error_count": errors.len(),
            "error_codes": error_codes(errors),
        },
        "errors": errors,
    })
}

fn manifest_markdown(manifest: &Value) -> String {
    let request = manifest["request"].as_object().expect("request object");
    let validation = manifest["validation"]
        .as_object()
        .expect("validation object");
    let content_hashes = manifest["content_hashes"]
        .as_object()
        .expect("content hashes object");
    let mut out = String::new();

    writeln!(out, "## Request Fingerprint Manifest").expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(
        out,
        "- request_id: {}",
        inline_json(request.get("request_id").unwrap_or(&Value::Null))
    )
    .expect("write markdown");
    writeln!(out, "- request_schema_version: `{REQUEST_SCHEMA_VERSION}`").expect("write markdown");
    writeln!(
        out,
        "- scope: {}",
        inline_json(request.get("scope").unwrap_or(&Value::Null))
    )
    .expect("write markdown");
    writeln!(
        out,
        "- target_app: {}",
        inline_json(request.get("target_app").unwrap_or(&Value::Null))
    )
    .expect("write markdown");
    writeln!(
        out,
        "- source_shape: {}",
        inline_json(request.get("source_shape").unwrap_or(&Value::Null))
    )
    .expect("write markdown");
    writeln!(out, "- generated_at: `{GENERATED_AT}`").expect("write markdown");
    writeln!(
        out,
        "- canonical_fields_sha256: `{}`",
        content_hashes
            .get("canonical_fields_sha256")
            .and_then(Value::as_str)
            .unwrap_or("")
    )
    .expect("write markdown");
    writeln!(
        out,
        "- source_sha256: {}",
        inline_json(content_hashes.get("source_sha256").unwrap_or(&Value::Null))
    )
    .expect("write markdown");
    writeln!(
        out,
        "- validation_status: `{}`",
        validation
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("invalid")
    )
    .expect("write markdown");
    writeln!(out).expect("write markdown");

    writeln!(out, "## Canonical Field Order").expect("write markdown");
    writeln!(out).expect("write markdown");
    for (index, key) in CANONICAL_FIELD_ORDER.iter().enumerate() {
        writeln!(out, "{}. `{key}`", index + 1).expect("write markdown");
    }
    writeln!(out).expect("write markdown");

    writeln!(out, "## Field Hashes").expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(out, "| Field | SHA-256 |").expect("write markdown");
    writeln!(out, "| --- | --- |").expect("write markdown");
    for record in manifest["field_hashes"].as_array().expect("field hashes") {
        writeln!(
            out,
            "| {} | `{}` |",
            markdown_cell(record.get("key").and_then(Value::as_str).unwrap_or("")),
            record
                .get("value_sha256")
                .and_then(Value::as_str)
                .unwrap_or("")
        )
        .expect("write markdown");
    }
    writeln!(out).expect("write markdown");

    writeln!(out, "## Validation").expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(
        out,
        "- valid: `{}`",
        validation
            .get("valid")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    )
    .expect("write markdown");
    writeln!(
        out,
        "- error_count: `{}`",
        validation
            .get("error_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )
    .expect("write markdown");
    writeln!(
        out,
        "- error_codes: {}",
        inline_json(validation.get("error_codes").unwrap_or(&Value::Null))
    )
    .expect("write markdown");
    writeln!(out).expect("write markdown");

    writeln!(out, "## Errors").expect("write markdown");
    writeln!(out).expect("write markdown");
    let errors = manifest["errors"].as_array().expect("errors array");
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

    writeln!(out, "## Normalized JSON Manifest").expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(out, "```json").expect("write markdown");
    write!(
        out,
        "{}",
        serde_json::to_string_pretty(manifest).expect("serialize manifest")
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

fn sha256_prefixed(bytes: &[u8]) -> String {
    let digest = sha256(bytes);
    let mut hex = String::with_capacity(7 + (digest.len() * 2));
    hex.push_str("sha256:");
    for byte in digest {
        write!(hex, "{byte:02x}").expect("write hash hex");
    }
    hex
}

fn sha256(input: &[u8]) -> [u8; 32] {
    const H0: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let bit_len = (input.len() as u64) * 8;
    let mut padded = Vec::with_capacity((input.len() + 9).div_ceil(64) * 64);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut h = H0;
    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (slot, word_bytes) in w.iter_mut().take(16).zip(chunk.chunks_exact(4)) {
            *slot =
                u32::from_be_bytes([word_bytes[0], word_bytes[1], word_bytes[2], word_bytes[3]]);
        }
        for index in 16..64 {
            let s0 = small_sigma0(w[index - 15]);
            let s1 = small_sigma1(w[index - 2]);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h_work] = h;
        for (&constant, &word) in K.iter().zip(w.iter()) {
            let t1 = h_work
                .wrapping_add(big_sigma1(e))
                .wrapping_add(ch(e, f, g))
                .wrapping_add(constant)
                .wrapping_add(word);
            let t2 = big_sigma0(a).wrapping_add(maj(a, b, c));
            h_work = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, h_work]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut out = [0u8; 32];
    for (chunk, value) in out.chunks_exact_mut(4).zip(h) {
        chunk.copy_from_slice(&value.to_be_bytes());
    }
    out
}

fn ch(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (!x & z)
}

fn maj(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (x & z) ^ (y & z)
}

fn big_sigma0(x: u32) -> u32 {
    x.rotate_right(2) ^ x.rotate_right(13) ^ x.rotate_right(22)
}

fn big_sigma1(x: u32) -> u32 {
    x.rotate_right(6) ^ x.rotate_right(11) ^ x.rotate_right(25)
}

fn small_sigma0(x: u32) -> u32 {
    x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3)
}

fn small_sigma1(x: u32) -> u32 {
    x.rotate_right(17) ^ x.rotate_right(19) ^ (x >> 10)
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
            "request_schema_version": REQUEST_SCHEMA_VERSION,
            "request_id": "DOT-1435",
            "scope": "agentmesh:app:request-fingerprint-manifest",
            "target_app": "request-fingerprint-manifest",
            "markdown": "---\ntitle: \"Add a deterministic request fingerprint manifest app\"\nready_for_multica: true\nstatus: ready\nproject_key: agentmesh-private\nissue_type: AFK\nrequest_kind: app\nsource_prd: \"4_Project/OSS/agentmesh-private/Requests/App/2026-08-10-add-a-deterministic-request-fingerprint-manifest-app.md\"\nsource_design: 4_Project/OSS/agentmesh-private/Docs/agentmesh-request-operations-v1.md\nsource_roadmap: 4_Project/OSS/agentmesh-private/ROADMAP.md\nsequence_index: 1\nsequence_total: 1\nblocked_by: []\nunblocks: []\n---\n# Add a deterministic request fingerprint manifest app\n\n## What to build\nBuild it.\n\n## Acceptance criteria\n- Manifest is deterministic.\n"
        })
    }

    #[test]
    fn sha256_matches_standard_test_vector() {
        assert_eq!(
            sha256_prefixed(b"abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn valid_manifest_contains_stable_hashes_and_markdown() {
        let output = fingerprint_request_manifest(&valid_input());
        assert_eq!(output["schema_version"], OUTPUT_SCHEMA_VERSION);
        assert_eq!(output["valid"], true);
        assert_eq!(output["error_count"], 0);
        assert_eq!(
            output["manifest_json"]["serialization"]["canonical_field_order"],
            json!(CANONICAL_FIELD_ORDER)
        );
        assert_eq!(
            output["manifest_json"]["generated_at"],
            "1970-01-01T00:00:00Z"
        );
        assert!(output["content_hashes"]["canonical_fields_sha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert!(output["manifest_markdown"]
            .as_str()
            .unwrap()
            .contains("## Request Fingerprint Manifest"));
    }

    #[test]
    fn hash_values_are_deterministic_across_runs() {
        let first = fingerprint_request_manifest(&valid_input());
        let second = fingerprint_request_manifest(&valid_input());
        assert_eq!(first, second);
        assert_eq!(
            first["content_hashes"]["canonical_fields_sha256"],
            "sha256:a360541d23b46ef12f75ec2dd5162b13d7ccc46a43f29de8e32f33f8199e727f"
        );
        assert_eq!(
            first["manifest_json"]["field_hashes"][0]["value_sha256"],
            "sha256:6ebd378c2cc3eb567e84cf9fbf4eada17575cfc186cd20272fa2a7fb292c0079"
        );
    }

    #[test]
    fn explicit_failure_codes_cover_contract_errors() {
        let output = fingerprint_request_manifest(&json!({
            "schema_version": "request-fingerprint-manifest-input.v1",
            "request_schema_version": "agentmesh-request.v9",
            "request_id": "DOT-1435",
            "scope": "agentmesh:app:request-fingerprint-manifest",
            "target_app": "request-fingerprint-manifest",
            "markdown": "---\ntitle: One\ntitle: Two\nrequest_kind app\n---\n# Bad\n"
        }));
        let codes: Vec<_> = output["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|error| error["code"].as_str().unwrap())
            .collect();
        assert!(codes.contains(&"AGENTMESH_REQUEST_FINGERPRINT_UNKNOWN_SCHEMA_VERSION"));
        assert!(codes.contains(&"AGENTMESH_REQUEST_FINGERPRINT_FIELD_DUPLICATE"));
        assert!(codes.contains(&"AGENTMESH_REQUEST_FINGERPRINT_FRONTMATTER_MALFORMED"));
        assert!(codes.contains(&"AGENTMESH_REQUEST_FINGERPRINT_FIELD_REQUIRED"));
    }

    #[test]
    fn recorded_fixtures_match_expected_payloads() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "request_fingerprint_manifest_success_input.json",
                "expected_request_fingerprint_manifest_success_payload.json",
            ),
            (
                "request_fingerprint_manifest_missing_fields_input.json",
                "expected_request_fingerprint_manifest_missing_fields_payload.json",
            ),
            (
                "request_fingerprint_manifest_malformed_frontmatter_input.json",
                "expected_request_fingerprint_manifest_malformed_frontmatter_payload.json",
            ),
            (
                "request_fingerprint_manifest_unknown_schema_input.json",
                "expected_request_fingerprint_manifest_unknown_schema_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            assert_eq!(
                fingerprint_request_manifest(&input),
                expected,
                "{input_name} should match {expected_name}"
            );
        }
    }
}
