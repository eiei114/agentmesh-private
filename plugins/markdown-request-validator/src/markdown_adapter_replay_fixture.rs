//! Deterministic Markdown adapter replay fixture App.
//!
//! Accepts one validated request Markdown document, a declared adapter fixture,
//! and an expected canonical replay result, then emits stable pass or mismatch
//! diagnostics without filesystem access, credentials, or orchestrator IDs.

use crate::adapter_compact_helpers::{canonical_json_bytes, error_codes, sort_json_keys};
use crate::adapter_error_contract::normalize_adapter_errors;
use crate::request_markdown_normalizer::normalize_request_markdown;
use agentmesh_evidence::sha256_prefixed;
use serde_json::{json, Map, Value};

/// Plugin/schema version exposed in compact output.
pub const REPLAY_FIXTURE_VERSION: &str = "markdown-adapter-replay-fixture.v0";
const INPUT_SCHEMA_VERSION: &str = "markdown-adapter-replay-fixture-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "markdown-adapter-replay-fixture-compact.v0";
const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";
const NORMALIZER_INPUT_SCHEMA_VERSION: &str = "request-markdown-normalizer-input.v0";

const REPLAY_PASS: &str = "pass";
const REPLAY_CANONICAL_PROJECTION_MISMATCH: &str = "canonical_projection_mismatch";
const REPLAY_ADAPTER_ERROR_MISMATCH: &str = "adapter_error_mismatch";
const REPLAY_MALFORMED_INPUT: &str = "malformed_input";

/// Evaluate a Markdown adapter replay fixture and return deterministic compact JSON.
pub fn evaluate_markdown_adapter_replay_fixture(value: &Value) -> Value {
    let mut errors = Vec::new();
    let Some(input) = value.as_object() else {
        errors.push(input_error(
            "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_INPUT_INVALID",
            "$",
            "input must be a JSON object",
        ));
        return compact(
            false,
            REPLAY_MALFORMED_INPUT,
            None,
            None,
            None,
            None,
            errors,
        );
    };

    validate_input_schema(input, &mut errors);
    let fixture = fixture_object(input, &mut errors);
    let expected = expected_object(input, &mut errors);
    let markdown = markdown_source(input, &mut errors);

    if !errors.is_empty() {
        return compact(
            false,
            REPLAY_MALFORMED_INPUT,
            fixture,
            expected,
            None,
            None,
            errors,
        );
    }

    let normalizer_input = json!({
        "schema_version": NORMALIZER_INPUT_SCHEMA_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "markdown": markdown,
    });
    let normalized = normalize_request_markdown(&normalizer_input);
    let request_summary = request_summary(&normalized);
    let projection_summary = projection_summary(&normalized);

    if normalized.get("valid").and_then(Value::as_bool) != Some(true) {
        let normalizer_errors = normalized
            .get("errors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let actual_codes = error_codes(&normalizer_errors);
        let expected_codes = string_array(expected.as_ref(), "normalizer_error_codes");
        let mut mismatches =
            compare_string_sets("normalizer_error_codes", &expected_codes, &actual_codes);
        let expected_status = expected_replay_status(expected.as_ref());
        if expected_status != REPLAY_MALFORMED_INPUT {
            mismatches.push(mismatch_record(
                "$.expected.replay_status",
                "replay_status",
                expected_status,
                REPLAY_MALFORMED_INPUT,
            ));
        }
        sort_mismatches(&mut mismatches);
        let diagnostics = build_malformed_diagnostics(&normalized);
        return build_output(OutputContext {
            valid: false,
            replay_status: REPLAY_MALFORMED_INPUT,
            fixture: fixture.as_ref(),
            expected: expected.as_ref(),
            request: request_summary.as_ref(),
            projection: projection_summary.as_ref(),
            adapter_output: None,
            diagnostics: &diagnostics,
            mismatches: &mismatches,
        });
    }

    let adapter_output = adapter_output(input, fixture.as_ref(), markdown);
    let adapter_codes = adapter_output
        .as_ref()
        .and_then(|output| output.get("errors"))
        .and_then(Value::as_array)
        .map(|errors| error_codes(errors))
        .unwrap_or_default();
    let expected_adapter_codes = string_array(expected.as_ref(), "adapter_error_codes");
    let adapter_mismatches = compare_string_sets(
        "adapter_error_codes",
        &expected_adapter_codes,
        &adapter_codes,
    );

    let projection_mismatches =
        compare_projection(expected.as_ref(), &normalized, request_summary.as_ref());
    let actual_replay_status = if !adapter_mismatches.is_empty() {
        REPLAY_ADAPTER_ERROR_MISMATCH
    } else if !projection_mismatches.is_empty() {
        REPLAY_CANONICAL_PROJECTION_MISMATCH
    } else {
        REPLAY_PASS
    };

    let expected_status = expected_replay_status(expected.as_ref());
    let mut mismatches = projection_mismatches;
    mismatches.extend(adapter_mismatches);
    if expected_status != actual_replay_status {
        mismatches.push(mismatch_record(
            "$.expected.replay_status",
            "replay_status",
            expected_status,
            actual_replay_status,
        ));
    }
    sort_mismatches(&mut mismatches);

    let replay_status = if mismatches.is_empty() {
        REPLAY_PASS
    } else {
        actual_replay_status
    };
    build_output(OutputContext {
        valid: replay_status == REPLAY_PASS,
        replay_status,
        fixture: fixture.as_ref(),
        expected: expected.as_ref(),
        request: request_summary.as_ref(),
        projection: projection_summary.as_ref(),
        adapter_output: adapter_output.as_ref(),
        diagnostics: &[],
        mismatches: &mismatches,
    })
}

fn validate_input_schema(input: &Map<String, Value>, errors: &mut Vec<Value>) {
    match input.get("schema_version").and_then(Value::as_str) {
        Some(INPUT_SCHEMA_VERSION) => {}
        Some(version) => errors.push(input_error(
            "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_INPUT_INVALID",
            "$.schema_version",
            format!("schema_version must be {INPUT_SCHEMA_VERSION} (got {version})"),
        )),
        None => errors.push(input_error(
            "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_FIELD_REQUIRED",
            "$.schema_version",
            "schema_version is required",
        )),
    }

    if let Some(version) = input.get("request_schema_version") {
        match version.as_str() {
            Some(REQUEST_SCHEMA_VERSION) => {}
            Some(other) => errors.push(input_error(
                "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_INPUT_INVALID",
                "$.request_schema_version",
                format!("request_schema_version must be {REQUEST_SCHEMA_VERSION} (got {other})"),
            )),
            None => errors.push(input_error(
                "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_INPUT_INVALID",
                "$.request_schema_version",
                "request_schema_version must be a string when provided",
            )),
        }
    }
}

fn markdown_source<'a>(input: &'a Map<String, Value>, errors: &mut Vec<Value>) -> &'a str {
    match input.get("markdown") {
        Some(Value::String(markdown)) if !markdown.trim().is_empty() => markdown,
        Some(Value::String(_)) | None | Some(Value::Null) => {
            errors.push(input_error(
                "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_FIELD_REQUIRED",
                "$.markdown",
                "markdown is required",
            ));
            ""
        }
        Some(_) => {
            errors.push(input_error(
                "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_INPUT_INVALID",
                "$.markdown",
                "markdown must be a string",
            ));
            ""
        }
    }
}

fn fixture_object(input: &Map<String, Value>, errors: &mut Vec<Value>) -> Option<Value> {
    let Some(fixture) = input.get("fixture") else {
        errors.push(input_error(
            "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_FIELD_REQUIRED",
            "$.fixture",
            "fixture is required",
        ));
        return None;
    };
    let Some(fixture) = fixture.as_object() else {
        errors.push(input_error(
            "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_INPUT_INVALID",
            "$.fixture",
            "fixture must be an object",
        ));
        return None;
    };
    if fixture
        .get("fixture_id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        errors.push(input_error(
            "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_FIELD_REQUIRED",
            "$.fixture.fixture_id",
            "fixture_id is required",
        ));
    }
    if fixture
        .get("adapter")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        errors.push(input_error(
            "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_FIELD_REQUIRED",
            "$.fixture.adapter",
            "adapter is required",
        ));
    }
    Some(Value::Object(fixture.clone()))
}

fn expected_object(input: &Map<String, Value>, errors: &mut Vec<Value>) -> Option<Value> {
    let Some(expected) = input.get("expected") else {
        errors.push(input_error(
            "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_FIELD_REQUIRED",
            "$.expected",
            "expected is required",
        ));
        return None;
    };
    let Some(expected) = expected.as_object() else {
        errors.push(input_error(
            "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_INPUT_INVALID",
            "$.expected",
            "expected must be an object",
        ));
        return None;
    };
    if expected
        .get("replay_status")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        errors.push(input_error(
            "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_FIELD_REQUIRED",
            "$.expected.replay_status",
            "expected.replay_status is required",
        ));
    }
    Some(Value::Object(expected.clone()))
}

fn adapter_output(
    input: &Map<String, Value>,
    fixture: Option<&Value>,
    markdown: &str,
) -> Option<Value> {
    let fixture = fixture?.as_object()?;
    fixture.get("adapter_failure")?;

    let mut adapter_input = Map::new();
    adapter_input.insert(
        "schema_version".to_string(),
        json!("adapter-error-contract-input.v0"),
    );
    adapter_input.insert("markdown".to_string(), json!(markdown));
    if let Some(source_adapter) = fixture.get("source_adapter") {
        adapter_input.insert("source_adapter".to_string(), source_adapter.clone());
    } else if let Some(adapter) = fixture.get("adapter") {
        adapter_input.insert("source_adapter".to_string(), adapter.clone());
    }
    if let Some(value) = fixture.get("adapter_failure") {
        adapter_input.insert("adapter_failure".to_string(), value.clone());
    }
    if let Some(value) = fixture.get("requested_capabilities") {
        adapter_input.insert("requested_capabilities".to_string(), value.clone());
    }
    if let Some(value) = fixture.get("available_capabilities") {
        adapter_input.insert("available_capabilities".to_string(), value.clone());
    }
    if let Some(value) = input.get("max_markdown_bytes") {
        adapter_input.insert("max_markdown_bytes".to_string(), value.clone());
    }
    Some(normalize_adapter_errors(&Value::Object(adapter_input)))
}

fn request_summary(normalized: &Value) -> Option<Value> {
    let projection = normalized.get("projection")?;
    let fields = projection.get("fields")?;
    Some(json!({
        "title": fields.get("title").cloned().unwrap_or(Value::Null),
        "request_kind": fields.get("request_kind").cloned().unwrap_or(Value::Null),
        "issue_type": fields.get("issue_type").cloned().unwrap_or(Value::Null),
        "project_key": fields.get("project_key").cloned().unwrap_or(Value::Null),
        "request_slug": normalized.get("request_slug").cloned().unwrap_or(Value::Null),
    }))
}

fn projection_summary(normalized: &Value) -> Option<Value> {
    Some(json!({
        "request_slug": normalized.get("request_slug").cloned().unwrap_or(Value::Null),
        "content_hashes": normalized.get("content_hashes").cloned().unwrap_or(Value::Null),
        "projection_sha256": normalized
            .get("content_hashes")
            .and_then(|hashes| hashes.get("projection_sha256"))
            .cloned()
            .unwrap_or(Value::Null),
    }))
}

fn compare_projection(
    expected: Option<&Value>,
    normalized: &Value,
    request_summary: Option<&Value>,
) -> Vec<Value> {
    let Some(expected) = expected.and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut mismatches = Vec::new();

    if let Some(expected_slug) = expected.get("request_slug").and_then(Value::as_str) {
        let actual_slug = normalized
            .get("request_slug")
            .and_then(Value::as_str)
            .unwrap_or("");
        if expected_slug != actual_slug {
            mismatches.push(mismatch_record(
                "$.expected.request_slug",
                "request_slug",
                expected_slug,
                actual_slug,
            ));
        }
    }

    if let Some(expected_fields) = expected.get("projection_fields").and_then(Value::as_object) {
        let actual_fields = normalized
            .get("projection")
            .and_then(|projection| projection.get("fields"))
            .and_then(Value::as_object);
        for (key, expected_value) in expected_fields {
            let actual_value = actual_fields
                .and_then(|fields| fields.get(key))
                .unwrap_or(&Value::Null);
            if expected_value != actual_value {
                mismatches.push(mismatch_record(
                    format!("$.expected.projection_fields.{key}"),
                    key,
                    expected_value.clone(),
                    actual_value.clone(),
                ));
            }
        }
    }

    if let (Some(expected), Some(actual)) = (
        expected.get("title").and_then(Value::as_str),
        request_summary
            .and_then(|summary| summary.get("title"))
            .and_then(Value::as_str),
    ) {
        if expected != actual {
            mismatches.push(mismatch_record(
                "$.expected.title",
                "title",
                expected,
                actual,
            ));
        }
    }

    mismatches
}

fn compare_string_sets(field: &str, expected: &[String], actual: &[String]) -> Vec<Value> {
    if expected == actual {
        return Vec::new();
    }
    vec![mismatch_record(
        format!("$.expected.{field}"),
        field,
        expected,
        actual,
    )]
}

fn string_array(expected: Option<&Value>, field: &str) -> Vec<String> {
    expected
        .and_then(Value::as_object)
        .and_then(|object| object.get(field))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn expected_replay_status(expected: Option<&Value>) -> &str {
    expected
        .and_then(Value::as_object)
        .and_then(|object| object.get("replay_status"))
        .and_then(Value::as_str)
        .unwrap_or(REPLAY_PASS)
}

fn fixture_details(fixture: Option<&Value>, adapter_output: Option<&Value>) -> Value {
    let Some(fixture) = fixture.and_then(Value::as_object) else {
        return Value::Null;
    };
    json!({
        "fixture_id": fixture.get("fixture_id").cloned().unwrap_or(Value::Null),
        "adapter": fixture.get("adapter").cloned().unwrap_or(Value::Null),
        "scenario": fixture.get("scenario").cloned().unwrap_or(Value::Null),
        "adapter_failure": fixture.get("adapter_failure").cloned().unwrap_or(Value::Null),
        "adapter_digest": adapter_output.map(adapter_digest).unwrap_or(Value::Null),
    })
}

fn adapter_digest(adapter_output: &Value) -> Value {
    json!({
        "algorithm": "sha256",
        "adapter_output_sha256": sha256_prefixed(&canonical_json_bytes(adapter_output)),
    })
}

fn adapter_section(adapter_output: Option<&Value>) -> Value {
    let Some(adapter_output) = adapter_output else {
        return json!({
            "present": false,
            "valid": null,
            "error_count": 0,
            "error_codes": [],
        });
    };
    json!({
        "present": true,
        "valid": adapter_output.get("valid").cloned().unwrap_or(Value::Null),
        "error_count": adapter_output.get("error_count").cloned().unwrap_or(json!(0)),
        "error_codes": adapter_output
            .get("errors")
            .and_then(Value::as_array)
            .map(|errors| {
                Value::Array(
                    error_codes(errors)
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                )
            })
            .unwrap_or_else(|| json!([])),
    })
}

fn sort_mismatches(mismatches: &mut [Value]) {
    mismatches.sort_by(|left, right| {
        left.get("path")
            .and_then(Value::as_str)
            .cmp(&right.get("path").and_then(Value::as_str))
            .then_with(|| {
                left.get("field")
                    .and_then(Value::as_str)
                    .cmp(&right.get("field").and_then(Value::as_str))
            })
    });
}

fn comparison(expected: Option<&Value>, replay_status: &str, mismatches: &[Value]) -> Value {
    json!({
        "expected_replay_status": expected_replay_status(expected),
        "actual_replay_status": replay_status,
        "matches": mismatches.is_empty(),
        "mismatch_count": mismatches.len(),
        "mismatches": mismatches,
    })
}

fn build_malformed_diagnostics(normalized: &Value) -> Vec<Value> {
    let mut diagnostics = normalized
        .get("errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    diagnostics.sort_by(|left, right| {
        left.get("code")
            .and_then(Value::as_str)
            .cmp(&right.get("code").and_then(Value::as_str))
            .then_with(|| {
                left.get("path")
                    .and_then(Value::as_str)
                    .cmp(&right.get("path").and_then(Value::as_str))
            })
    });
    diagnostics
}

struct OutputContext<'a> {
    valid: bool,
    replay_status: &'a str,
    fixture: Option<&'a Value>,
    expected: Option<&'a Value>,
    request: Option<&'a Value>,
    projection: Option<&'a Value>,
    adapter_output: Option<&'a Value>,
    diagnostics: &'a [Value],
    mismatches: &'a [Value],
}

fn build_output(context: OutputContext<'_>) -> Value {
    sort_json_keys(json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "app_version": REPLAY_FIXTURE_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "valid": context.valid,
        "replay_status": context.replay_status,
        "request": context.request.cloned().unwrap_or(Value::Null),
        "projection": context.projection.cloned().unwrap_or(Value::Null),
        "fixture": fixture_details(context.fixture, context.adapter_output),
        "comparison": comparison(context.expected, context.replay_status, context.mismatches),
        "adapter": adapter_section(context.adapter_output),
        "diagnostic_count": context.diagnostics.len(),
        "diagnostics": context.diagnostics,
        "error_count": context.diagnostics.len(),
        "errors": context.diagnostics,
    }))
}

fn compact(
    valid: bool,
    replay_status: &str,
    fixture: Option<Value>,
    expected: Option<Value>,
    request: Option<Value>,
    projection: Option<Value>,
    diagnostics: Vec<Value>,
) -> Value {
    build_output(OutputContext {
        valid,
        replay_status,
        fixture: fixture.as_ref(),
        expected: expected.as_ref(),
        request: request.as_ref(),
        projection: projection.as_ref(),
        adapter_output: None,
        diagnostics: &diagnostics,
        mismatches: &[],
    })
}

fn mismatch_record(
    path: impl Into<String>,
    field: impl Into<String>,
    expected: impl Into<Value>,
    actual: impl Into<Value>,
) -> Value {
    json!({
        "path": path.into(),
        "field": field.into(),
        "expected": expected.into(),
        "actual": actual.into(),
    })
}

fn input_error(code: &str, path: impl Into<String>, message: impl Into<String>) -> Value {
    json!({
        "code": code,
        "category": "input_schema_invalid",
        "severity": "error",
        "path": path.into(),
        "message": message.into(),
        "remediation_hint": remediation_hint(code),
    })
}

fn remediation_hint(code: &str) -> &'static str {
    match code {
        "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_INPUT_INVALID" => {
            "Match markdown-adapter-replay-fixture-input.v0 and keep fixture identifiers explicit."
        }
        "AGENTMESH_MARKDOWN_ADAPTER_REPLAY_FIELD_REQUIRED" => {
            "Provide markdown, fixture, and expected replay fields before adapter replay."
        }
        _ => "Repair the replay fixture input and rerun the Markdown adapter replay App.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_markdown() -> String {
        "---\ntitle: \"Add a Markdown request normalizer App\"\nrequest_kind: app\nissue_type: AFK\nready_for_multica: true\nstatus: ready\nproject_key: agentmesh-private\nsource_prd: \"synthetic://requests/request-markdown-normalizer\"\nsource_design: synthetic://docs/agentmesh-request-operations-v1\nsource_roadmap: synthetic://roadmaps/agentmesh-private\nblocked_by: []\nunblocks: []\nsequence_index: 1\nsequence_total: 1\n---\n# Add a Markdown request normalizer App\n\n## Notes\nNormalize outside Multica.\n\n## What to build\nBuild a deterministic normalizer preview.\n\n## Acceptance criteria\n- [ ] Emit stable projection payloads.\n- [ ] Sort canonical requirements.\n\n## Blocked by\n- None.\n\n## User stories covered\n- As a local runner maintainer, I can diff stable output.\n"
            .into()
    }

    fn base_input(expected: Value) -> Value {
        json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request_schema_version": REQUEST_SCHEMA_VERSION,
            "markdown": valid_markdown(),
            "fixture": {
                "fixture_id": "matching-projection-v0",
                "adapter": "local-runner-adapter",
                "scenario": "matching_result"
            },
            "expected": expected
        })
    }

    #[test]
    fn matching_projection_replay_passes() {
        let output = evaluate_markdown_adapter_replay_fixture(&base_input(json!({
            "replay_status": REPLAY_PASS,
            "request_slug": "add-a-markdown-request-normalizer-app",
            "projection_fields": {
                "title": "Add a Markdown request normalizer App",
                "request_kind": "app",
                "issue_type": "AFK",
                "project_key": "agentmesh-private"
            }
        })));
        assert_eq!(output["valid"], true);
        assert_eq!(output["replay_status"], REPLAY_PASS);
        assert_eq!(output["comparison"]["matches"], true);
    }

    #[test]
    fn canonical_projection_mismatch_is_reported() {
        let output = evaluate_markdown_adapter_replay_fixture(&base_input(json!({
            "replay_status": REPLAY_PASS,
            "request_slug": "wrong-slug",
            "projection_fields": {
                "title": "Add a Markdown request normalizer App"
            }
        })));
        assert_eq!(output["valid"], false);
        assert_eq!(
            output["replay_status"],
            REPLAY_CANONICAL_PROJECTION_MISMATCH
        );
        assert!(output["comparison"]["mismatch_count"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn adapter_error_mismatch_is_reported() {
        let output = evaluate_markdown_adapter_replay_fixture(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request_schema_version": REQUEST_SCHEMA_VERSION,
            "markdown": valid_markdown(),
            "fixture": {
                "fixture_id": "adapter-timeout-v0",
                "adapter": "non-multica-request-adapter",
                "scenario": "adapter_error_mismatch",
                "source_adapter": "non-multica-request-adapter",
                "adapter_failure": {
                    "kind": "timeout",
                    "native_code": "ETIMEDOUT",
                    "message": "adapter timed out after 1000ms",
                    "retryable": true
                }
            },
            "expected": {
                "replay_status": REPLAY_PASS,
                "adapter_error_codes": ["AGENTMESH_INPUT_SCHEMA_INVALID"]
            }
        }));
        assert_eq!(output["replay_status"], REPLAY_ADAPTER_ERROR_MISMATCH);
        assert_eq!(output["adapter"]["present"], true);
    }

    #[test]
    fn malformed_markdown_returns_normalized_errors() {
        let output = evaluate_markdown_adapter_replay_fixture(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "markdown": "# Missing frontmatter\n",
            "fixture": {
                "fixture_id": "malformed-markdown-v0",
                "adapter": "local-runner-adapter",
                "scenario": "malformed_input"
            },
            "expected": {
                "replay_status": REPLAY_MALFORMED_INPUT,
                "normalizer_error_codes": [
                    "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FRONTMATTER_MALFORMED"
                ]
            }
        }));
        assert_eq!(output["valid"], false);
        assert_eq!(output["replay_status"], REPLAY_MALFORMED_INPUT);
        assert!(output["diagnostic_count"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn malformed_markdown_with_wrong_expected_status_stays_malformed_input() {
        let output = evaluate_markdown_adapter_replay_fixture(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "markdown": "# Missing frontmatter\n",
            "fixture": {
                "fixture_id": "malformed-markdown-v0",
                "adapter": "local-runner-adapter",
                "scenario": "malformed_input"
            },
            "expected": {
                "replay_status": REPLAY_PASS,
                "normalizer_error_codes": [
                    "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FRONTMATTER_MALFORMED"
                ]
            }
        }));
        assert_eq!(output["replay_status"], REPLAY_MALFORMED_INPUT);
        assert_eq!(
            output["comparison"]["actual_replay_status"],
            REPLAY_MALFORMED_INPUT
        );
        assert_eq!(output["comparison"]["matches"], false);
        assert!(output["comparison"]["mismatches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mismatch| mismatch["field"] == "replay_status"));
    }

    #[test]
    fn malformed_markdown_keeps_mismatches_out_of_diagnostics() {
        let output = evaluate_markdown_adapter_replay_fixture(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "markdown": "# Missing frontmatter\n",
            "fixture": {
                "fixture_id": "malformed-markdown-v0",
                "adapter": "local-runner-adapter",
                "scenario": "malformed_input"
            },
            "expected": {
                "replay_status": REPLAY_MALFORMED_INPUT,
                "normalizer_error_codes": ["AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_UNEXPECTED"]
            }
        }));
        assert_eq!(output["replay_status"], REPLAY_MALFORMED_INPUT);
        assert_eq!(output["comparison"]["matches"], false);
        assert!(output["comparison"]["mismatches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mismatch| mismatch["field"] == "normalizer_error_codes"));
        for diagnostic in output["diagnostics"].as_array().unwrap() {
            assert!(
                diagnostic["code"].is_string(),
                "{diagnostic} must carry a diagnostic code"
            );
            assert!(
                diagnostic["message"].is_string(),
                "{diagnostic} must carry a diagnostic message"
            );
        }
    }

    #[test]
    #[ignore = "manual fixture regeneration helper"]
    fn dump_recorded_fixture_payloads() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "markdown_adapter_replay_fixture_success_input.json",
                "expected_markdown_adapter_replay_fixture_success_payload.json",
            ),
            (
                "markdown_adapter_replay_fixture_projection_mismatch_input.json",
                "expected_markdown_adapter_replay_fixture_projection_mismatch_payload.json",
            ),
            (
                "markdown_adapter_replay_fixture_adapter_error_mismatch_input.json",
                "expected_markdown_adapter_replay_fixture_adapter_error_mismatch_payload.json",
            ),
            (
                "markdown_adapter_replay_fixture_malformed_input.json",
                "expected_markdown_adapter_replay_fixture_malformed_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let payload = evaluate_markdown_adapter_replay_fixture(&input);
            std::fs::write(
                root.join(expected_name),
                format!("{}\n", serde_json::to_string_pretty(&payload).unwrap()),
            )
            .unwrap();
        }
    }

    #[test]
    fn recorded_fixtures_match_expected_payloads() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "markdown_adapter_replay_fixture_success_input.json",
                "expected_markdown_adapter_replay_fixture_success_payload.json",
            ),
            (
                "markdown_adapter_replay_fixture_projection_mismatch_input.json",
                "expected_markdown_adapter_replay_fixture_projection_mismatch_payload.json",
            ),
            (
                "markdown_adapter_replay_fixture_adapter_error_mismatch_input.json",
                "expected_markdown_adapter_replay_fixture_adapter_error_mismatch_payload.json",
            ),
            (
                "markdown_adapter_replay_fixture_malformed_input.json",
                "expected_markdown_adapter_replay_fixture_malformed_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            assert_eq!(
                evaluate_markdown_adapter_replay_fixture(&input),
                expected,
                "{input_name} should match {expected_name}"
            );
        }
    }
}
