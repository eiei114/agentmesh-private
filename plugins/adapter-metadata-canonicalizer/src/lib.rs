//! Adapter metadata comparison and canonicalization contract.
//!
//! Compares two request metadata payloads from different adapters, promotes only
//! equal stable common fields into a canonical object, and preserves all
//! adapter-specific or drifting fields separately for downstream adapter-owned
//! handling.

use serde::Deserialize;
use serde_json::{json, Map, Value};

/// Plugin/schema version exposed in compact output.
pub const APP_VERSION: &str = "adapter-metadata-canonicalizer.v0";
const INPUT_SCHEMA_VERSION: &str = "adapter-metadata-canonicalizer-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "adapter-metadata-canonicalizer-compact.v0";
const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request-metadata.v0";

/// Plugin/schema version exposed by the public 0.x readiness gate binary.
pub const PUBLIC_0X_READINESS_VERSION: &str = "public-0x-readiness.v0";
const READINESS_INPUT_SCHEMA_VERSION: &str = "public-0x-readiness-input.v0";
const READINESS_OUTPUT_SCHEMA_VERSION: &str = "public-0x-readiness-compact.v0";

const STABLE_FIELDS: &[&str] = &[
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
    "pr_required",
    "pr_allowed",
    "pr_mode",
    "release_allowed",
    "production_allowed",
    "version_bump_required",
    "version_bump_type",
    "package_publish_expected",
    "route_mode",
    "work_owner",
    "expected_pr_count",
];

#[derive(Debug, Deserialize)]
struct CanonicalizerInput {
    schema_version: String,
    left: AdapterPayload,
    right: AdapterPayload,
}

#[derive(Debug, Deserialize)]
struct AdapterPayload {
    adapter_id: String,
    #[serde(default)]
    request_id: Option<String>,
    metadata: Map<String, Value>,
}

/// Compare opaque plugin input and return deterministic compact JSON.
pub fn canonicalize_metadata_input(value: &Value) -> Value {
    let input: Result<CanonicalizerInput, _> = serde_json::from_value(value.clone());
    let input = match input {
        Ok(input) => input,
        Err(err) => {
            return compact(
                false,
                Map::new(),
                Vec::new(),
                Vec::new(),
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
    validate_adapter("left", &input.left, &mut issues);
    validate_adapter("right", &input.right, &mut issues);
    if input.left.adapter_id == input.right.adapter_id {
        issues.push(issue(
            "adapter_id_duplicate",
            "left.adapter_id and right.adapter_id must identify different adapters",
        ));
    }

    let mut canonical = Map::new();
    let mut mismatches = Vec::new();
    compare_request_ids(&input.left, &input.right, &mut canonical, &mut mismatches);
    compare_stable_fields(
        &input.left.metadata,
        &input.right.metadata,
        &mut canonical,
        &mut mismatches,
    );

    let adapters = vec![
        adapter_report(&input.left, &canonical),
        adapter_report(&input.right, &canonical),
    ];
    let valid = issues.is_empty() && mismatches.is_empty();
    compact(valid, canonical, mismatches, adapters, issues)
}

fn validate_adapter(side: &str, adapter: &AdapterPayload, issues: &mut Vec<Value>) {
    if adapter.adapter_id.trim().is_empty() {
        issues.push(issue(
            "adapter_id_missing",
            format!("{side}.adapter_id must not be empty"),
        ));
    }
}

fn compare_request_ids(
    left: &AdapterPayload,
    right: &AdapterPayload,
    canonical: &mut Map<String, Value>,
    mismatches: &mut Vec<Value>,
) {
    match (&left.request_id, &right.request_id) {
        (Some(left_id), Some(right_id)) if left_id == right_id => {
            canonical.insert("request_id".into(), Value::String(left_id.clone()));
        }
        (Some(left_id), Some(right_id)) => mismatches.push(json!({
            "code": "request_id_mismatch",
            "field": "request_id",
            "left": left_id,
            "right": right_id,
        })),
        (Some(left_id), None) => mismatches.push(presence_mismatch(
            "request_id",
            Some(Value::String(left_id.clone())),
            None,
        )),
        (None, Some(right_id)) => mismatches.push(presence_mismatch(
            "request_id",
            None,
            Some(Value::String(right_id.clone())),
        )),
        (None, None) => {}
    }
}

fn compare_stable_fields(
    left: &Map<String, Value>,
    right: &Map<String, Value>,
    canonical: &mut Map<String, Value>,
    mismatches: &mut Vec<Value>,
) {
    for field in STABLE_FIELDS {
        match (left.get(*field), right.get(*field)) {
            (Some(left_value), Some(right_value)) if left_value == right_value => {
                canonical.insert((*field).into(), left_value.clone());
            }
            (Some(left_value), Some(right_value)) => mismatches.push(json!({
                "code": "value_mismatch",
                "field": field,
                "left": left_value,
                "right": right_value,
            })),
            (Some(left_value), None) => {
                mismatches.push(presence_mismatch(field, Some(left_value.clone()), None))
            }
            (None, Some(right_value)) => {
                mismatches.push(presence_mismatch(field, None, Some(right_value.clone())))
            }
            (None, None) => {}
        }
    }
}

fn presence_mismatch(field: &str, left: Option<Value>, right: Option<Value>) -> Value {
    json!({
        "code": "field_presence_mismatch",
        "field": field,
        "left_present": left.is_some(),
        "right_present": right.is_some(),
        "left": left.unwrap_or(Value::Null),
        "right": right.unwrap_or(Value::Null),
    })
}

fn adapter_report(adapter: &AdapterPayload, canonical: &Map<String, Value>) -> Value {
    let mut specific = Map::new();
    for (key, value) in &adapter.metadata {
        if canonical.get(key) == Some(value) {
            continue;
        }
        specific.insert(key.clone(), value.clone());
    }

    json!({
        "adapter_id": adapter.adapter_id,
        "request_id": adapter.request_id,
        "specific": specific,
    })
}

fn compact(
    valid: bool,
    canonical: Map<String, Value>,
    mismatches: Vec<Value>,
    adapters: Vec<Value>,
    issues: Vec<Value>,
) -> Value {
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "app_version": APP_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "stable_fields": STABLE_FIELDS,
        "valid": valid,
        "canonical": canonical,
        "schema_drift": !mismatches.is_empty(),
        "mismatch_count": mismatches.len(),
        "mismatches": mismatches,
        "adapters": adapters,
        "issue_count": issues.len(),
        "issues": issues,
    })
}

fn issue(code: &str, message: impl Into<String>) -> Value {
    json!({"code": code, "message": message.into()})
}

/// Evaluate deterministic public 0.x readiness evidence.
///
/// The gate intentionally consumes compact outputs produced by the Markdown
/// validator and non-Multica request adapter instead of reparsing source
/// documents. This keeps the public-readiness claim tied to adapter evidence
/// that operators can retain and replay.
pub fn evaluate_public_0x_readiness_input(value: &Value) -> Value {
    let mut issues = Vec::new();
    let Some(object) = value.as_object() else {
        return readiness_compact(
            false,
            Vec::new(),
            vec![issue("input_invalid", "input must be a JSON object")],
        );
    };

    if object.get("schema_version").and_then(Value::as_str) != Some(READINESS_INPUT_SCHEMA_VERSION)
    {
        issues.push(issue(
            "unsupported_schema_version",
            format!("schema_version must be {READINESS_INPUT_SCHEMA_VERSION}"),
        ));
    }

    let checklist_paths = [
        ("protocol", "protocol_checkpoint_missing"),
        (
            "adapter_compatibility",
            "adapter_compatibility_checkpoint_missing",
        ),
        ("rollback", "rollback_checkpoint_missing"),
        (
            "evidence_retention",
            "evidence_retention_checkpoint_missing",
        ),
    ];
    for (field, code) in checklist_paths {
        if !bool_at(object.get("checklist"), field) {
            issues.push(issue(code, format!("checklist.{field} must be true")));
        }
    }

    let artifact_paths = [
        ("parser_snapshot", "parser_snapshot_missing"),
        ("adapter_parity", "adapter_parity_missing"),
        ("rollback_notes", "rollback_notes_missing"),
    ];
    for (field, code) in artifact_paths {
        if !bool_at(object.get("artifacts"), field) {
            issues.push(issue(code, format!("artifacts.{field} must be true")));
        }
    }

    let markdown = object.get("markdown_validator").unwrap_or(&Value::Null);
    let adapter = object.get("non_multica_adapter").unwrap_or(&Value::Null);
    if markdown.get("valid").and_then(Value::as_bool) != Some(true) {
        issues.push(issue(
            "markdown_validator_invalid",
            "markdown validator compact output must be valid",
        ));
    }
    if adapter.get("valid").and_then(Value::as_bool) != Some(true) {
        issues.push(issue(
            "non_multica_adapter_invalid",
            "non-Multica adapter compact output must be valid",
        ));
    }
    if adapter
        .get("request_schema_version")
        .and_then(Value::as_str)
        != Some("agentmesh-request.v0")
    {
        issues.push(issue(
            "request_schema_mismatch",
            "non-Multica adapter must emit agentmesh-request.v0",
        ));
    }

    let markdown_title = markdown.get("title").and_then(Value::as_str);
    let adapter_title = adapter.pointer("/canonical/title").and_then(Value::as_str);
    if markdown_title.is_none() || adapter_title.is_none() || markdown_title != adapter_title {
        issues.push(issue(
            "adapter_title_mismatch",
            "Markdown and non-Multica adapter titles must match",
        ));
    }

    for field in ["source_prd", "source_design", "source_roadmap"] {
        if adapter
            .pointer(&format!("/canonical/{field}"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            issues.push(issue(
                "source_reference_missing",
                format!("canonical.{field} must be retained"),
            ));
        }
    }

    let rollback = object.get("rollback").unwrap_or(&Value::Null);
    for field in [
        "verification_command",
        "evidence_note",
        "previous_good_artifact",
    ] {
        if rollback
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            issues.push(issue(
                "rollback_proof_missing",
                format!("rollback.{field} must be provided"),
            ));
        }
    }

    let retention = object.get("evidence_retention").unwrap_or(&Value::Null);
    if retention
        .get("location")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        issues.push(issue(
            "evidence_retention_location_missing",
            "evidence_retention.location must be provided",
        ));
    }
    if retention
        .get("retention_days")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        < 30
    {
        issues.push(issue(
            "evidence_retention_window_too_short",
            "evidence_retention.retention_days must be at least 30",
        ));
    }

    let assertions = vec![
        "protocol_checkpoint",
        "adapter_compatibility_checkpoint",
        "rollback_checkpoint",
        "evidence_retention_checkpoint",
        "parser_snapshot_artifact",
        "adapter_parity_artifact",
        "rollback_notes_artifact",
        "markdown_validator_valid",
        "non_multica_adapter_valid",
        "adapter_titles_aligned",
        "source_references_retained",
    ];
    readiness_compact(issues.is_empty(), assertions, issues)
}

fn bool_at(parent: Option<&Value>, field: &str) -> bool {
    parent
        .and_then(|value| value.get(field))
        .and_then(Value::as_bool)
        == Some(true)
}

fn readiness_compact(valid: bool, assertions: Vec<&str>, issues: Vec<Value>) -> Value {
    json!({
        "schema_version": READINESS_OUTPUT_SCHEMA_VERSION,
        "app_version": PUBLIC_0X_READINESS_VERSION,
        "readiness_target": "public-0.x",
        "valid": valid,
        "assertions": assertions,
        "issue_count": issues.len(),
        "issues": issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn equal_stable_fields_are_promoted_and_extensions_are_preserved() {
        let output = canonicalize_metadata_input(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "left": {
                "adapter_id": "multica",
                "request_id": "DOT-1048",
                "metadata": {
                    "title": "Add app",
                    "request_kind": "app",
                    "issue_type": "AFK",
                    "blocked_by": [],
                    "multica_status": "todo"
                }
            },
            "right": {
                "adapter_id": "markdown",
                "request_id": "DOT-1048",
                "metadata": {
                    "title": "Add app",
                    "request_kind": "app",
                    "issue_type": "AFK",
                    "blocked_by": [],
                    "frontmatter_span": {"start": 0, "end": 120}
                }
            }
        }));

        assert_eq!(output["valid"], true);
        assert_eq!(output["canonical"]["request_id"], "DOT-1048");
        assert_eq!(output["canonical"]["title"], "Add app");
        assert_eq!(output["mismatch_count"], 0);
        assert_eq!(output["adapters"][0]["specific"]["multica_status"], "todo");
        assert_eq!(
            output["adapters"][1]["specific"]["frontmatter_span"]["end"],
            120
        );
    }

    #[test]
    fn drift_is_reported_and_drifting_fields_stay_adapter_specific() {
        let output = canonicalize_metadata_input(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "left": {
                "adapter_id": "a",
                "metadata": {
                    "title": "Add app",
                    "status": "ready",
                    "sequence_index": 1
                }
            },
            "right": {
                "adapter_id": "b",
                "metadata": {
                    "title": "Add app",
                    "status": "todo"
                }
            }
        }));

        assert_eq!(output["valid"], false);
        assert_eq!(output["schema_drift"], true);
        assert_eq!(output["canonical"].get("status"), None);
        assert_eq!(output["mismatches"][0]["code"], "value_mismatch");
        assert_eq!(output["mismatches"][0]["field"], "status");
        assert_eq!(output["mismatches"][1]["code"], "field_presence_mismatch");
        assert_eq!(output["adapters"][0]["specific"]["status"], "ready");
        assert_eq!(output["adapters"][1]["specific"]["status"], "todo");
    }

    #[test]
    fn empty_metadata_payloads_are_deterministic() {
        let output = canonicalize_metadata_input(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "left": {"adapter_id": "a", "metadata": {}},
            "right": {"adapter_id": "b", "metadata": {}}
        }));

        assert_eq!(output["valid"], true);
        assert_eq!(output["canonical"], json!({}));
        assert_eq!(output["mismatches"], json!([]));
        assert_eq!(output["adapters"][0]["specific"], json!({}));
        assert_eq!(output["adapters"][1]["specific"], json!({}));
    }

    #[test]
    fn recorded_fixtures_match_expected_payloads() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "matching_metadata_input.json",
                "expected_matching_compact_payload.json",
            ),
            (
                "drift_metadata_input.json",
                "expected_drift_compact_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            assert_eq!(
                canonicalize_metadata_input(&input),
                expected,
                "{input_name} should match {expected_name}"
            );
        }
    }

    #[test]
    fn recorded_public_readiness_fixture_matches_expected_payload() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let input: Value = serde_json::from_slice(
            &std::fs::read(root.join("public_0x_readiness_input.json")).unwrap(),
        )
        .unwrap();
        let expected: Value = serde_json::from_slice(
            &std::fs::read(root.join("expected_public_0x_readiness_compact_payload.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(evaluate_public_0x_readiness_input(&input), expected);
    }
}
