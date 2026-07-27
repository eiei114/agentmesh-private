//! Normalized adapter error contract App.
//!
//! This module keeps request validation and adapter execution failures in a
//! tool-neutral taxonomy so Markdown-first adapters can compare outcomes without
//! copying Multica-specific fields into their source contracts.

use serde_json::{json, Map, Value};

/// Plugin/schema version exposed in compact output.
pub const ERROR_CONTRACT_VERSION: &str = "adapter-error-contract.v0";
const INPUT_SCHEMA_VERSION: &str = "adapter-error-contract-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "adapter-error-contract-compact.v0";
const TAXONOMY_VERSION: &str = "agentmesh-adapter-error-taxonomy.v0";
const MAX_MARKDOWN_BYTES: usize = 64 * 1024;
const REQUIRED_FIELDS: &[(&str, &str)] = &[
    ("title", "$.markdown.frontmatter.title"),
    ("request_kind", "$.markdown.frontmatter.request_kind"),
    ("issue_type", "$.markdown.frontmatter.issue_type"),
];
const REQUIRED_MARKDOWN_SECTIONS: &[&str] = &["What to build", "Acceptance criteria"];

/// Normalize opaque plugin input into deterministic adapter-neutral error records.
pub fn normalize_adapter_errors(value: &Value) -> Value {
    let mut errors = Vec::new();
    let Some(input) = value.as_object() else {
        errors.push(record(
            "AGENTMESH_INPUT_SCHEMA_INVALID",
            "request.input_schema_invalid",
            "request_validation",
            Some("$"),
            "input must be a JSON object".to_string(),
            None,
        ));
        return compact(errors);
    };

    validate_schema_version(input, &mut errors);
    validate_markdown(input, &mut errors);
    validate_capabilities(input, &mut errors);
    validate_adapter_failure(input, &mut errors);

    compact(errors)
}

fn validate_schema_version(input: &Map<String, Value>, errors: &mut Vec<Value>) {
    match input.get("schema_version").and_then(Value::as_str) {
        Some(INPUT_SCHEMA_VERSION) => {}
        Some(version) => errors.push(record(
            "AGENTMESH_INPUT_SCHEMA_INVALID",
            "request.input_schema_invalid",
            "request_validation",
            Some("$.schema_version"),
            format!("schema_version must be {INPUT_SCHEMA_VERSION} (got {version})"),
            None,
        )),
        None => errors.push(record(
            "AGENTMESH_FIELD_REQUIRED",
            "request.field_required",
            "request_validation",
            Some("$.schema_version"),
            "schema_version is required".to_string(),
            None,
        )),
    }
}

fn validate_markdown(input: &Map<String, Value>, errors: &mut Vec<Value>) {
    let max_markdown_bytes = input
        .get("max_markdown_bytes")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(MAX_MARKDOWN_BYTES);

    let Some(markdown_value) = input.get("markdown") else {
        errors.push(record(
            "AGENTMESH_FIELD_REQUIRED",
            "request.field_required",
            "request_validation",
            Some("$.markdown"),
            "markdown is required".to_string(),
            None,
        ));
        return;
    };
    let Some(markdown) = markdown_value.as_str() else {
        errors.push(record(
            "AGENTMESH_INPUT_SCHEMA_INVALID",
            "request.input_schema_invalid",
            "request_validation",
            Some("$.markdown"),
            "markdown must be a string".to_string(),
            None,
        ));
        return;
    };

    if max_markdown_bytes == 0 {
        errors.push(record(
            "AGENTMESH_INPUT_SCHEMA_INVALID",
            "request.input_schema_invalid",
            "request_validation",
            Some("$.max_markdown_bytes"),
            "max_markdown_bytes must be at least 1".to_string(),
            None,
        ));
    } else if markdown.len() > max_markdown_bytes {
        errors.push(record(
            "AGENTMESH_BOUNDARY_EXCEEDED",
            "request.boundary_exceeded",
            "request_validation",
            Some("$.markdown"),
            format!(
                "markdown is {} bytes; limit is {max_markdown_bytes}",
                markdown.len()
            ),
            None,
        ));
    }

    let normalized = markdown.replace("\r\n", "\n");
    let Some(frontmatter) = parse_frontmatter(&normalized) else {
        errors.push(record(
            "AGENTMESH_MARKDOWN_INVALID",
            "request.markdown_invalid",
            "request_validation",
            Some("$.markdown"),
            "YAML frontmatter block is required".to_string(),
            None,
        ));
        return;
    };

    for section in REQUIRED_MARKDOWN_SECTIONS {
        if !has_markdown_section(&normalized, section) {
            errors.push(record(
                "AGENTMESH_MARKDOWN_INVALID",
                "request.markdown_invalid",
                "request_validation",
                Some("$.markdown"),
                format!("markdown section {section:?} is required"),
                None,
            ));
        }
    }

    let fields = fields_from_frontmatter(frontmatter);
    for (field, path) in REQUIRED_FIELDS {
        if fields
            .get(*field)
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        {
            errors.push(record(
                "AGENTMESH_FIELD_REQUIRED",
                "request.field_required",
                "request_validation",
                Some(path),
                format!("frontmatter field {field} is required"),
                None,
            ));
        }
    }
}

fn validate_capabilities(input: &Map<String, Value>, errors: &mut Vec<Value>) {
    let requested = string_array(input.get("requested_capabilities"));
    let available = string_array(input.get("available_capabilities"));
    if let Err(message) = &requested {
        errors.push(record(
            "AGENTMESH_INPUT_SCHEMA_INVALID",
            "request.input_schema_invalid",
            "request_validation",
            Some("$.requested_capabilities"),
            message.clone(),
            None,
        ));
    }
    if let Err(message) = &available {
        errors.push(record(
            "AGENTMESH_INPUT_SCHEMA_INVALID",
            "request.input_schema_invalid",
            "request_validation",
            Some("$.available_capabilities"),
            message.clone(),
            None,
        ));
    }
    let (Ok(mut requested), Ok(available)) = (requested, available) else {
        return;
    };
    requested.sort();
    requested.dedup();
    for capability in requested {
        if !available.iter().any(|candidate| candidate == &capability) {
            errors.push(record(
                "AGENTMESH_CAPABILITY_UNKNOWN",
                "request.capability_unknown",
                "request_validation",
                Some("$.requested_capabilities"),
                format!("capability {capability:?} is not available"),
                None,
            ));
        }
    }
}

fn validate_adapter_failure(input: &Map<String, Value>, errors: &mut Vec<Value>) {
    let Some(adapter_failure) = input.get("adapter_failure") else {
        return;
    };
    let Some(adapter_failure) = adapter_failure.as_object() else {
        errors.push(record(
            "AGENTMESH_INPUT_SCHEMA_INVALID",
            "request.input_schema_invalid",
            "request_validation",
            Some("$.adapter_failure"),
            "adapter_failure must be an object".to_string(),
            None,
        ));
        return;
    };
    let kind = adapter_failure
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let native_code = adapter_failure
        .get("native_code")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let message = adapter_failure
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("external adapter failed");
    let source_adapter = input
        .get("source_adapter")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let retryable = adapter_failure
        .get("retryable")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    errors.push(record(
        "AGENTMESH_EXTERNAL_ADAPTER_FAILURE",
        external_taxonomy_code(kind),
        "adapter_execution",
        Some("$.adapter_failure"),
        message.to_string(),
        Some(json!({
            "adapter": source_adapter,
            "kind": kind,
            "native_code": native_code,
            "retryable": retryable
        })),
    ));
}

fn compact(errors: Vec<Value>) -> Value {
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "contract_version": ERROR_CONTRACT_VERSION,
        "taxonomy_version": TAXONOMY_VERSION,
        "valid": errors.is_empty(),
        "error_count": errors.len(),
        "errors": errors,
        "error_schema": error_schema(),
    })
}

fn error_schema() -> Value {
    json!({
        "schema_version": "agentmesh-adapter-error-schema.v0",
        "code_namespace": "agentmesh.adapter_error",
        "stable_codes": [
            code_schema("AGENTMESH_INPUT_SCHEMA_INVALID", "request.input_schema_invalid", "error", "request_validation"),
            code_schema("AGENTMESH_MARKDOWN_INVALID", "request.markdown_invalid", "error", "request_validation"),
            code_schema("AGENTMESH_FIELD_REQUIRED", "request.field_required", "error", "request_validation"),
            code_schema("AGENTMESH_CAPABILITY_UNKNOWN", "request.capability_unknown", "error", "request_validation"),
            code_schema("AGENTMESH_BOUNDARY_EXCEEDED", "request.boundary_exceeded", "error", "request_validation"),
            code_schema("AGENTMESH_EXTERNAL_ADAPTER_FAILURE", "adapter.external_failure", "error", "adapter_execution")
        ],
        "supported_taxonomy_codes": [
            "request.input_schema_invalid",
            "request.markdown_invalid",
            "request.field_required",
            "request.capability_unknown",
            "request.boundary_exceeded",
            "adapter.external_failure",
            "adapter.auth_failed",
            "adapter.rate_limited",
            "adapter.timeout",
            "adapter.transport_failed"
        ],
        "required_record_fields": ["code", "taxonomy_code", "severity", "source", "message", "remediation_hint"]
    })
}

fn code_schema(code: &str, taxonomy_code: &str, severity: &str, source: &str) -> Value {
    json!({
        "code": code,
        "taxonomy_code": taxonomy_code,
        "severity": severity,
        "source": source,
        "remediation_hint": remediation_hint(code)
    })
}

fn record(
    code: &str,
    taxonomy_code: &str,
    source: &str,
    path: Option<&str>,
    message: String,
    native: Option<Value>,
) -> Value {
    json!({
        "code": code,
        "taxonomy_code": taxonomy_code,
        "severity": "error",
        "source": source,
        "path": path,
        "message": message,
        "remediation_hint": remediation_hint(code),
        "native": native,
    })
}

fn remediation_hint(code: &str) -> &'static str {
    match code {
        "AGENTMESH_INPUT_SCHEMA_INVALID" => {
            "Match adapter-error-contract-input.v0 and keep adapter details in adapter_failure."
        }
        "AGENTMESH_MARKDOWN_INVALID" => {
            "Provide Markdown with YAML frontmatter and the required request sections."
        }
        "AGENTMESH_FIELD_REQUIRED" => {
            "Add the missing request field to the Markdown frontmatter before adapter handoff."
        }
        "AGENTMESH_CAPABILITY_UNKNOWN" => {
            "Route to an adapter that advertises the requested capability or remove it."
        }
        "AGENTMESH_BOUNDARY_EXCEEDED" => {
            "Reduce the request payload below the documented byte limit."
        }
        "AGENTMESH_EXTERNAL_ADAPTER_FAILURE" => {
            "Inspect the native adapter failure, then retry only when retryable is true."
        }
        _ => "Use the normalized taxonomy code to choose adapter-neutral remediation.",
    }
}

fn external_taxonomy_code(kind: &str) -> &'static str {
    match kind {
        "auth" => "adapter.auth_failed",
        "rate_limit" => "adapter.rate_limited",
        "timeout" => "adapter.timeout",
        "transport" => "adapter.transport_failed",
        _ => "adapter.external_failure",
    }
}

fn parse_frontmatter(markdown: &str) -> Option<&str> {
    let rest = markdown.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some(&rest[..end])
}

fn has_markdown_section(markdown: &str, section: &str) -> bool {
    markdown.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed
            .strip_prefix("## ")
            .is_some_and(|heading| heading.trim() == section)
    })
}

fn fields_from_frontmatter(frontmatter: &str) -> Map<String, Value> {
    let mut fields = Map::new();
    for line in frontmatter.lines() {
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        fields.insert(key.trim().to_string(), scalar(raw.trim()));
    }
    fields
}

fn scalar(raw: &str) -> Value {
    let trimmed = raw.trim().trim_matches('"');
    match trimmed {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => trimmed
            .parse::<u64>()
            .map_or_else(|_| Value::String(trimmed.to_string()), |n| json!(n)),
    }
}

fn string_array(value: Option<&Value>) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(values) = value.as_array() else {
        return Err("value must be an array of strings".to_string());
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .ok_or_else(|| "value must be an array of strings".to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_markdown() -> String {
        "---\ntitle: \"Add app\"\nrequest_kind: app\nissue_type: AFK\n---\n# Add app\n\n## What to build\nBuild it.\n\n## Acceptance criteria\n- Pass.\n"
            .into()
    }

    #[test]
    fn valid_input_has_empty_error_records() {
        let output = normalize_adapter_errors(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "markdown": valid_markdown(),
            "requested_capabilities": ["markdown_request_validation"],
            "available_capabilities": ["markdown_request_validation"]
        }));
        assert_eq!(output["valid"], true);
        assert_eq!(output["error_count"], 0);
        assert_eq!(
            output["error_schema"]["stable_codes"][0]["code"],
            "AGENTMESH_INPUT_SCHEMA_INVALID"
        );
    }

    #[test]
    fn unknown_capabilities_are_sorted_for_reproducible_ordering() {
        let output = normalize_adapter_errors(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "markdown": valid_markdown(),
            "requested_capabilities": ["zeta", "alpha", "zeta"],
            "available_capabilities": []
        }));
        assert_eq!(
            output["errors"][0]["message"],
            "capability \"alpha\" is not available"
        );
        assert_eq!(
            output["errors"][1]["message"],
            "capability \"zeta\" is not available"
        );
    }

    #[test]
    fn recorded_fixtures_match_expected_payloads() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "adapter_error_invalid_markdown_input.json",
                "expected_adapter_error_invalid_markdown_payload.json",
            ),
            (
                "adapter_error_missing_field_input.json",
                "expected_adapter_error_missing_field_payload.json",
            ),
            (
                "adapter_error_unknown_capability_input.json",
                "expected_adapter_error_unknown_capability_payload.json",
            ),
            (
                "adapter_error_boundary_input.json",
                "expected_adapter_error_boundary_payload.json",
            ),
            (
                "adapter_error_external_failure_input.json",
                "expected_adapter_error_external_failure_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            assert_eq!(
                normalize_adapter_errors(&input),
                expected,
                "{input_name} should match {expected_name}"
            );
        }
    }
}
