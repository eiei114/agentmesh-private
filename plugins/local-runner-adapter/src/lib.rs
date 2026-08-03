//! Local-runner adapter compatibility contract.
//!
//! Converts `agentmesh-request.v0` Markdown or JSON-compatible request sources
//! into a deterministic local-runner envelope. Stable request fields are emitted
//! in a contract-defined order, while adapter-only metadata is preserved only in
//! the runner envelope's `adapter_metadata.passthrough` object.

use serde_json::{json, Map, Value};

/// Plugin/schema version exposed in compact output.
pub const ADAPTER_VERSION: &str = "local-runner-adapter.v0";
const INPUT_SCHEMA_VERSION: &str = "local-runner-adapter-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "local-runner-adapter-compact.v0";
const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";
const CANONICAL_SCHEMA_VERSION: &str = "local-runner-canonical-request.v0";
const ENVELOPE_SCHEMA_VERSION: &str = "local-runner-envelope.v0";
const DIAGNOSTIC_SCHEMA_VERSION: &str = "local-runner-diagnostic.v0";
const ADAPTER_METADATA_SCHEMA_VERSION: &str = "local-runner-adapter-metadata.v0";
const LOCAL_RUNNER_VERSION: &str = "local-runner.v0";
const MAX_SOURCE_BYTES: usize = 64 * 1024;

const TOP_LEVEL_FIELDS: &[&str] = &["adapter", "markdown", "request", "schema_version"];
const REQUEST_FIELDS: &[&str] = &[
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
const CANONICAL_FIELD_ORDER: &[&str] = &[
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
const RUNNER_FIELD_ORDER: &[&str] = &[
    "id",
    "title",
    "request_kind",
    "issue_type",
    "project",
    "state",
    "sources",
    "dependencies",
    "sequence",
    "adapter_metadata",
];
const ACCEPTED_REQUEST_KINDS: &[&str] = &["app", "repair"];

#[derive(Debug, Clone, Copy)]
enum SourceKind {
    Markdown,
    Request,
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
    let mut diagnostics = Vec::new();
    let Some(object) = value.as_object() else {
        diagnostics.push(diagnostic(
            "input_not_object",
            "input",
            "$",
            "input must be a JSON object",
            "object matching local-runner-adapter-input.v0",
        ));
        return compact(None, None, diagnostics);
    };

    validate_top_level(object, &mut diagnostics);
    validate_schema_version(object, &mut diagnostics);
    let adapter_passthrough = adapter_passthrough(object.get("adapter"), &mut diagnostics);

    let source = source_fields(object, &mut diagnostics);
    if let Some((fields, source_kind)) = &source {
        validate_request_fields(fields, *source_kind, &mut diagnostics);
    }

    diagnostics.sort_by_key(diagnostic_sort_key);
    let has_errors = diagnostics.iter().any(is_error_diagnostic);
    let canonical = source.as_ref().map(|(fields, _)| canonical_payload(fields));
    let local_runner_envelope = if has_errors {
        None
    } else {
        source
            .as_ref()
            .map(|(fields, _)| local_runner_envelope(fields, adapter_passthrough))
    };

    compact(canonical, local_runner_envelope, diagnostics)
}

fn validate_top_level(object: &Map<String, Value>, diagnostics: &mut Vec<Value>) {
    for key in object.keys() {
        if !TOP_LEVEL_FIELDS.contains(&key.as_str()) {
            diagnostics.push(diagnostic(
                "top_level_field_extra",
                key,
                format!("$.{key}"),
                "top-level field is not part of local-runner-adapter-input.v0",
                "remove the field or move adapter-only data under $.adapter.passthrough",
            ));
        }
    }
}

fn validate_schema_version(object: &Map<String, Value>, diagnostics: &mut Vec<Value>) {
    match object.get("schema_version") {
        None => diagnostics.push(diagnostic(
            "schema_version_missing",
            "schema_version",
            "$.schema_version",
            "schema_version is required",
            INPUT_SCHEMA_VERSION,
        )),
        Some(Value::String(version)) if version == INPUT_SCHEMA_VERSION => {}
        Some(Value::String(_)) => diagnostics.push(diagnostic(
            "schema_version_unsupported",
            "schema_version",
            "$.schema_version",
            format!("schema_version must be {INPUT_SCHEMA_VERSION}"),
            INPUT_SCHEMA_VERSION,
        )),
        Some(_) => diagnostics.push(diagnostic(
            "input_field_incompatible",
            "schema_version",
            "$.schema_version",
            "schema_version must be a string",
            INPUT_SCHEMA_VERSION,
        )),
    }
}

fn adapter_passthrough(value: Option<&Value>, diagnostics: &mut Vec<Value>) -> Value {
    let Some(value) = value else {
        return json!({});
    };
    let Some(object) = value.as_object() else {
        diagnostics.push(diagnostic(
            "input_field_incompatible",
            "adapter",
            "$.adapter",
            "adapter must be an object when provided",
            "object with optional passthrough object",
        ));
        return json!({});
    };

    for key in object.keys() {
        if key != "passthrough" {
            diagnostics.push(diagnostic(
                "adapter_field_extra",
                key,
                format!("$.adapter.{key}"),
                "adapter field is not supported by local-runner-adapter-input.v0",
                "only adapter.passthrough is supported",
            ));
        }
    }

    match object.get("passthrough") {
        None => json!({}),
        Some(Value::Object(_)) => object["passthrough"].clone(),
        Some(_) => {
            diagnostics.push(diagnostic(
                "adapter_passthrough_incompatible",
                "passthrough",
                "$.adapter.passthrough",
                "adapter.passthrough must be an object when provided",
                "object containing adapter-only metadata",
            ));
            json!({})
        }
    }
}

fn source_fields(
    object: &Map<String, Value>,
    diagnostics: &mut Vec<Value>,
) -> Option<(RequestFields, SourceKind)> {
    let has_markdown = object.contains_key("markdown");
    let has_request = object.contains_key("request");
    if has_markdown == has_request {
        diagnostics.push(diagnostic(
            "source_shape_invalid",
            "source",
            "$",
            "provide exactly one of markdown or request",
            "one markdown string or one request object",
        ));
        return None;
    }

    if has_markdown {
        let Some(markdown) = object.get("markdown").and_then(Value::as_str) else {
            diagnostics.push(diagnostic(
                "input_field_incompatible",
                "markdown",
                "$.markdown",
                "markdown must be a string",
                "Markdown request source with YAML frontmatter",
            ));
            return None;
        };
        return parse_markdown(markdown, diagnostics).map(|fields| (fields, SourceKind::Markdown));
    }

    let Some(request) = object.get("request").and_then(Value::as_object) else {
        diagnostics.push(diagnostic(
            "input_field_incompatible",
            "request",
            "$.request",
            "request must be a JSON object",
            "Markdown-compatible request.v0 object",
        ));
        return None;
    };
    Some((
        fields_from_object(request, SourceKind::Request, diagnostics),
        SourceKind::Request,
    ))
}

fn parse_markdown(markdown: &str, diagnostics: &mut Vec<Value>) -> Option<RequestFields> {
    if markdown.len() > MAX_SOURCE_BYTES {
        diagnostics.push(diagnostic(
            "source_too_large",
            "markdown",
            "$.markdown",
            format!(
                "markdown is {} bytes; limit is {MAX_SOURCE_BYTES}",
                markdown.len()
            ),
            format!("at most {MAX_SOURCE_BYTES} bytes"),
        ));
    }

    let normalized = markdown.replace("\r\n", "\n");
    let Some(frontmatter) = parse_frontmatter(&normalized) else {
        diagnostics.push(diagnostic(
            "frontmatter_missing",
            "frontmatter",
            "$.markdown",
            "YAML frontmatter block is required for markdown sources",
            "frontmatter delimited by --- lines",
        ));
        return None;
    };
    Some(fields_from_frontmatter(frontmatter, diagnostics))
}

fn fields_from_frontmatter(frontmatter: &str, diagnostics: &mut Vec<Value>) -> RequestFields {
    let mut fields = RequestFields::default();
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, raw)) = trimmed.split_once(':') else {
            diagnostics.push(diagnostic(
                "frontmatter_line_incompatible",
                "frontmatter",
                "$.markdown.frontmatter",
                "frontmatter lines must use key: value syntax",
                "key: value",
            ));
            continue;
        };
        let key = key.trim();
        if !REQUEST_FIELDS.contains(&key) {
            diagnostics.push(diagnostic(
                "request_field_extra",
                key,
                field_path(SourceKind::Markdown, key),
                "request field is not part of agentmesh-request.v0 for local-runner adapters",
                "remove the field or move adapter-only data under $.adapter.passthrough",
            ));
            continue;
        }
        set_field(
            &mut fields,
            key,
            scalar(raw.trim()),
            SourceKind::Markdown,
            diagnostics,
        );
    }
    fields
}

fn fields_from_object(
    object: &Map<String, Value>,
    source_kind: SourceKind,
    diagnostics: &mut Vec<Value>,
) -> RequestFields {
    let mut fields = RequestFields::default();
    for (key, value) in object {
        if !REQUEST_FIELDS.contains(&key.as_str()) {
            diagnostics.push(diagnostic(
                "request_field_extra",
                key,
                field_path(source_kind, key),
                "request field is not part of agentmesh-request.v0 for local-runner adapters",
                "remove the field or move adapter-only data under $.adapter.passthrough",
            ));
            continue;
        }
        set_field(&mut fields, key, value.clone(), source_kind, diagnostics);
    }
    fields
}

fn set_field(
    fields: &mut RequestFields,
    key: &str,
    value: Value,
    source_kind: SourceKind,
    diagnostics: &mut Vec<Value>,
) {
    match key {
        "title" => set_string(&mut fields.title, key, value, source_kind, diagnostics),
        "request_kind" => set_string(
            &mut fields.request_kind,
            key,
            value,
            source_kind,
            diagnostics,
        ),
        "issue_type" => set_string(&mut fields.issue_type, key, value, source_kind, diagnostics),
        "status" => set_string(&mut fields.status, key, value, source_kind, diagnostics),
        "project_key" => set_string(
            &mut fields.project_key,
            key,
            value,
            source_kind,
            diagnostics,
        ),
        "source_prd" => set_string(&mut fields.source_prd, key, value, source_kind, diagnostics),
        "source_design" => set_string(
            &mut fields.source_design,
            key,
            value,
            source_kind,
            diagnostics,
        ),
        "source_roadmap" => set_string(
            &mut fields.source_roadmap,
            key,
            value,
            source_kind,
            diagnostics,
        ),
        "ready_for_multica" => match value.as_bool() {
            Some(value) => fields.ready_for_multica = Some(value),
            None => incompatible(key, source_kind, "boolean", diagnostics),
        },
        "blocked_by" => set_string_array(
            &mut fields.blocked_by,
            key,
            &value,
            source_kind,
            diagnostics,
        ),
        "unblocks" => set_string_array(&mut fields.unblocks, key, &value, source_kind, diagnostics),
        "sequence_index" => set_u64(
            &mut fields.sequence_index,
            key,
            &value,
            source_kind,
            diagnostics,
        ),
        "sequence_total" => set_u64(
            &mut fields.sequence_total,
            key,
            &value,
            source_kind,
            diagnostics,
        ),
        _ => {}
    }
}

fn set_string(
    target: &mut Option<String>,
    key: &str,
    value: Value,
    source_kind: SourceKind,
    diagnostics: &mut Vec<Value>,
) {
    match value.as_str() {
        Some(value) => *target = Some(value.to_string()),
        None => incompatible(key, source_kind, "string", diagnostics),
    }
}

fn set_string_array(
    target: &mut Vec<String>,
    key: &str,
    value: &Value,
    source_kind: SourceKind,
    diagnostics: &mut Vec<Value>,
) {
    let Some(items) = value.as_array() else {
        incompatible(key, source_kind, "array of strings", diagnostics);
        return;
    };
    if !items.iter().all(Value::is_string) {
        incompatible(key, source_kind, "array of strings", diagnostics);
        return;
    }
    *target = items
        .iter()
        .filter_map(|item| item.as_str().map(ToString::to_string))
        .collect();
}

fn set_u64(
    target: &mut Option<u64>,
    key: &str,
    value: &Value,
    source_kind: SourceKind,
    diagnostics: &mut Vec<Value>,
) {
    match value.as_u64() {
        Some(value) => *target = Some(value),
        None => incompatible(key, source_kind, "unsigned integer", diagnostics),
    }
}

fn validate_request_fields(
    fields: &RequestFields,
    source_kind: SourceKind,
    diagnostics: &mut Vec<Value>,
) {
    if fields.title.as_deref().unwrap_or("").trim().is_empty() {
        missing("title", source_kind, diagnostics);
    }
    if fields.issue_type.as_deref().unwrap_or("").trim().is_empty() {
        missing("issue_type", source_kind, diagnostics);
    }
    match fields.request_kind.as_deref() {
        Some(kind) if ACCEPTED_REQUEST_KINDS.contains(&kind) => {}
        Some(_) => diagnostics.push(diagnostic(
            "request_kind_unsupported",
            "request_kind",
            field_path(source_kind, "request_kind"),
            "request_kind must be one of app, repair",
            "app or repair",
        )),
        None => missing("request_kind", source_kind, diagnostics),
    }
    if fields.sequence_index.is_some() != fields.sequence_total.is_some() {
        diagnostics.push(diagnostic(
            "request_sequence_incomplete",
            "sequence",
            field_path(source_kind, "sequence"),
            "sequence_index and sequence_total must be provided together",
            "both sequence_index and sequence_total, or neither",
        ));
    }
}

fn missing(field: &str, source_kind: SourceKind, diagnostics: &mut Vec<Value>) {
    diagnostics.push(diagnostic(
        "request_field_missing",
        field,
        field_path(source_kind, field),
        format!("request field {field} is required"),
        "non-empty value in the request source",
    ));
}

fn incompatible(
    field: &str,
    source_kind: SourceKind,
    expected: &str,
    diagnostics: &mut Vec<Value>,
) {
    diagnostics.push(diagnostic(
        "request_field_incompatible",
        field,
        field_path(source_kind, field),
        format!("request field {field} is incompatible with local-runner-adapter-input.v0"),
        expected,
    ));
}

fn compact(
    canonical: Option<Value>,
    local_runner_envelope: Option<Value>,
    diagnostics: Vec<Value>,
) -> Value {
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "adapter_version": ADAPTER_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "valid": diagnostics.iter().all(|diagnostic| !is_error_diagnostic(diagnostic)),
        "canonical": canonical,
        "local_runner_envelope": local_runner_envelope,
        "diagnostic_model": {
            "schema_version": DIAGNOSTIC_SCHEMA_VERSION,
            "severity_order": ["error", "warning"],
            "sort_order": ["code", "path", "field", "message"],
            "rerun_policy": "Diagnostics are deterministic for the same request artifact; fix the reported input path and rerun the same request."
        },
        "diagnostic_count": diagnostics.len(),
        "diagnostics": diagnostics,
    })
}

fn canonical_payload(fields: &RequestFields) -> Value {
    json!({
        "schema_version": CANONICAL_SCHEMA_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "field_order": CANONICAL_FIELD_ORDER,
        "fields": {
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
        }
    })
}

fn local_runner_envelope(fields: &RequestFields, adapter_passthrough: Value) -> Value {
    let project = fields.project_key.as_deref().unwrap_or("unassigned");
    let title = fields.title.as_deref().unwrap_or("untitled");
    json!({
        "schema_version": ENVELOPE_SCHEMA_VERSION,
        "runner": LOCAL_RUNNER_VERSION,
        "field_order": RUNNER_FIELD_ORDER,
        "id": format!("local-runner://{project}/{}", slug(title)),
        "title": fields.title,
        "request_kind": fields.request_kind,
        "issue_type": fields.issue_type,
        "project": fields.project_key,
        "state": fields.status.as_deref().unwrap_or("draft"),
        "sources": {
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
        "adapter_metadata": {
            "schema_version": ADAPTER_METADATA_SCHEMA_VERSION,
            "passthrough": adapter_passthrough,
        }
    })
}

fn diagnostic(
    code: &str,
    field: impl Into<String>,
    path: impl Into<String>,
    message: impl Into<String>,
    expected: impl Into<String>,
) -> Value {
    json!({
        "code": code,
        "field": field.into(),
        "path": path.into(),
        "severity": "error",
        "message": message.into(),
        "expected": expected.into(),
        "rerun_action": "fix_input_and_rerun",
    })
}

fn diagnostic_sort_key(value: &Value) -> (String, String, String, String) {
    (
        value
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        value
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        value
            .get("field")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

fn is_error_diagnostic(value: &Value) -> bool {
    value.get("severity").and_then(Value::as_str) == Some("error")
}

fn field_path(source_kind: SourceKind, field: &str) -> String {
    match source_kind {
        SourceKind::Markdown => format!("$.markdown.frontmatter.{field}"),
        SourceKind::Request => format!("$.request.{field}"),
    }
}

fn parse_frontmatter(markdown: &str) -> Option<&str> {
    let rest = markdown.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some(&rest[..end])
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
        "[]" => Value::Array(Vec::new()),
        _ => trimmed.parse::<u64>().map_or_else(
            |_| Value::String(trimmed.to_string()),
            |number| json!(number),
        ),
    }
}

fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut previous_dash = false;
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            out.push(character);
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
                "malformed_request_input.json",
                "expected_malformed_compact_payload.json",
            ),
            (
                "missing_field_request_input.json",
                "expected_missing_field_compact_payload.json",
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
    fn local_runner_payload_excludes_multica_readiness() {
        let output = adapt_request_input(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request": {
                "title": "Run without Multica fields",
                "request_kind": "app",
                "issue_type": "AFK",
                "ready_for_multica": true,
                "status": "ready",
                "blocked_by": [],
                "unblocks": []
            }
        }));

        assert_eq!(output["valid"], true);
        assert!(output["canonical"]["fields"]
            .get("ready_for_multica")
            .is_none());
        assert!(output["local_runner_envelope"]
            .get("ready_for_multica")
            .is_none());
    }

    #[test]
    fn slug_is_stable_and_ascii_only() {
        assert_eq!(
            slug("Add a local-runner adapter compatibility App!"),
            "add-a-local-runner-adapter-compatibility-app"
        );
        assert_eq!(slug("---"), "untitled");
    }
}
