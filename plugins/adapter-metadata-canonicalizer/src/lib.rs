//! Adapter metadata comparison and canonicalization contract.
//!
//! Compares two request metadata payloads from different adapters, promotes only
//! equal stable common fields into a canonical object, and preserves all
//! adapter-specific or drifting fields separately for downstream adapter-owned
//! handling.

use agentmesh_markdown_request_validator::adapter_error_contract::normalize_adapter_errors;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// Plugin/schema version exposed in compact output.
pub const APP_VERSION: &str = "adapter-metadata-canonicalizer.v0";
const INPUT_SCHEMA_VERSION: &str = "adapter-metadata-canonicalizer-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "adapter-metadata-canonicalizer-compact.v0";
const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request-metadata.v0";

/// Plugin/schema version exposed by the public 0.x readiness gate binary.
pub const PUBLIC_0X_READINESS_VERSION: &str = "public-0x-readiness.v0";
const READINESS_INPUT_SCHEMA_VERSION: &str = "public-0x-readiness-input.v0";
const READINESS_OUTPUT_SCHEMA_VERSION: &str = "public-0x-readiness-compact.v0";

/// Plugin/schema version exposed by the adapter evidence envelope binary.
pub const ADAPTER_EVIDENCE_ENVELOPE_VERSION: &str = "adapter-evidence-envelope.v0";
const EVIDENCE_ENVELOPE_INPUT_SCHEMA_VERSION: &str = "adapter-evidence-envelope-input.v0";
const EVIDENCE_ENVELOPE_OUTPUT_SCHEMA_VERSION: &str = "adapter-evidence-envelope-compact.v0";
const EVIDENCE_ENVELOPE_ALLOWED_PHASES: &[&str] = &["validation", "execution"];
const EVIDENCE_ENVELOPE_ALLOWED_RESULT_CLASSES: &[&str] = &[
    "success",
    "malformed_input",
    "adapter_parity_mismatch",
    "adapter_error",
    "execution_error",
];

/// Plugin/schema version exposed by the deterministic rollback replay gate binary.
pub const PUBLIC_0X_ROLLBACK_REPLAY_VERSION: &str = "public-0x-rollback-replay.v0";
const ROLLBACK_REPLAY_INPUT_SCHEMA_VERSION: &str = "public-0x-rollback-replay-input.v0";
const ROLLBACK_REPLAY_OUTPUT_SCHEMA_VERSION: &str = "public-0x-rollback-replay-compact.v0";

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

/// Build a deterministic adapter evidence envelope for validation/execution evidence.
///
/// The envelope intentionally normalizes adapter identity, capability facts,
/// result class, diagnostics, and replay transcript digest into one stable shape
/// so downstream adapters can compare parity without adapter-specific keys.
pub fn build_adapter_evidence_envelope_input(value: &Value) -> Value {
    let mut diagnostics = Vec::new();
    let Some(object) = value.as_object() else {
        push_diag(
            &mut diagnostics,
            "input_invalid",
            "input must be a JSON object",
            "error",
            Some("/"),
        );
        return evidence_envelope_compact(None, None, None, None, Vec::new(), diagnostics, None);
    };

    if object.get("schema_version").and_then(Value::as_str)
        != Some(EVIDENCE_ENVELOPE_INPUT_SCHEMA_VERSION)
    {
        push_diag(
            &mut diagnostics,
            "unsupported_schema_version",
            format!("schema_version must be {EVIDENCE_ENVELOPE_INPUT_SCHEMA_VERSION}"),
            "error",
            Some("/schema_version"),
        );
    }

    let request_id = required_string(object, "request_id", &mut diagnostics);
    let phase = required_string(object, "phase", &mut diagnostics).filter(|phase| {
        if EVIDENCE_ENVELOPE_ALLOWED_PHASES.contains(&phase.as_str()) {
            true
        } else {
            push_diag(
                &mut diagnostics,
                "phase_unsupported",
                "phase must be validation or execution",
                "error",
                Some("/phase"),
            );
            false
        }
    });

    let adapter_object = object.get("adapter").and_then(Value::as_object);
    if adapter_object.is_none() {
        push_diag(
            &mut diagnostics,
            "adapter_missing",
            "adapter object is required",
            "error",
            Some("/adapter"),
        );
    }
    let adapter_id = adapter_object
        .and_then(|adapter| required_string_at(adapter, "id", "/adapter/id", &mut diagnostics));
    let adapter_version = adapter_object
        .and_then(|adapter| adapter.get("version"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let adapter_capabilities = adapter_object
        .and_then(|adapter| adapter.get("capabilities"))
        .map_or_else(Vec::new, |value| {
            string_array(value, "/adapter/capabilities", &mut diagnostics)
        });

    let capability_object = object.get("capability").and_then(Value::as_object);
    if capability_object.is_none() {
        push_diag(
            &mut diagnostics,
            "capability_missing",
            "capability object is required",
            "error",
            Some("/capability"),
        );
    }
    let capability_name = capability_object.and_then(|capability| {
        required_string_at(capability, "name", "/capability/name", &mut diagnostics)
    });
    let capability_schema_version = capability_object.and_then(|capability| {
        required_string_at(
            capability,
            "schema_version",
            "/capability/schema_version",
            &mut diagnostics,
        )
    });
    let capability_operation = capability_object.and_then(|capability| {
        required_string_at(
            capability,
            "operation",
            "/capability/operation",
            &mut diagnostics,
        )
    });

    let result_class = object
        .get("result")
        .and_then(Value::as_object)
        .and_then(|result| required_string_at(result, "class", "/result/class", &mut diagnostics))
        .filter(|class| {
            if EVIDENCE_ENVELOPE_ALLOWED_RESULT_CLASSES.contains(&class.as_str()) {
                true
            } else {
                push_diag(
                    &mut diagnostics,
                    "result_class_unsupported",
                    "result.class must be one of success, malformed_input, adapter_parity_mismatch, adapter_error, execution_error",
                    "error",
                    Some("/result/class"),
                );
                false
            }
        });

    if !object.contains_key("result") {
        push_diag(
            &mut diagnostics,
            "result_missing",
            "result object is required",
            "error",
            Some("/result"),
        );
    }

    append_input_diagnostics(object.get("diagnostics"), &mut diagnostics);
    let transcript = match object.get("transcript") {
        Some(value @ Value::Array(_)) => Some(value.clone()),
        Some(_) => {
            push_diag(
                &mut diagnostics,
                "transcript_invalid",
                "transcript must be an array when provided",
                "error",
                Some("/transcript"),
            );
            None
        }
        None => Some(json!([])),
    };

    diagnostics.sort_by_key(diagnostic_sort_key);

    let capability = json!({
        "adapter_capabilities": adapter_capabilities,
        "name": capability_name,
        "operation": capability_operation,
        "schema_version": capability_schema_version,
    });
    let adapter = json!({
        "id": adapter_id,
        "version": adapter_version,
        "capabilities": capability["adapter_capabilities"].clone(),
    });

    evidence_envelope_compact(
        request_id,
        phase,
        Some(adapter),
        result_class,
        diagnostics,
        Vec::new(),
        transcript.map(|transcript| (capability, transcript)),
    )
}

fn evidence_envelope_compact(
    request_id: Option<String>,
    phase: Option<String>,
    adapter: Option<Value>,
    result_class: Option<String>,
    mut diagnostics: Vec<Value>,
    additional_diagnostics: Vec<Value>,
    capability_and_transcript: Option<(Value, Value)>,
) -> Value {
    diagnostics.extend(additional_diagnostics);
    diagnostics.sort_by_key(diagnostic_sort_key);
    let result_class = if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.get("severity").and_then(Value::as_str) == Some("error"))
    {
        result_class.unwrap_or_else(|| "malformed_input".to_string())
    } else {
        result_class.unwrap_or_else(|| "success".to_string())
    };
    let (capability_hash, transcript_digest) = capability_and_transcript.map_or_else(
        || (digest_value(&json!({})), digest_value(&json!([]))),
        |(capability, transcript)| (digest_value(&capability), digest_value(&transcript)),
    );
    json!({
        "schema_version": EVIDENCE_ENVELOPE_OUTPUT_SCHEMA_VERSION,
        "app_version": ADAPTER_EVIDENCE_ENVELOPE_VERSION,
        "valid": diagnostics.is_empty() && result_class == "success",
        "request_id": request_id,
        "phase": phase,
        "capability_hash": {
            "algorithm": "sha256",
            "value": capability_hash,
        },
        "adapter": adapter.unwrap_or_else(|| json!({
            "id": Value::Null,
            "version": Value::Null,
            "capabilities": [],
        })),
        "result_class": result_class,
        "deterministic_diagnostics": {
            "ordering": ["code", "field", "severity", "message"],
            "items": diagnostics,
        },
        "replay_transcript_digest": {
            "algorithm": "sha256",
            "source": "canonical JSON transcript array",
            "value": transcript_digest,
        },
        "serialization": {
            "format": "json",
            "object_key_order": "lexicographic",
            "array_order": "contract-defined; diagnostics sorted by code/field/severity/message; adapter capabilities preserve supplied order",
            "null_policy": "missing optional scalar fields serialize as null; malformed inputs retain a deterministic envelope"
        },
        "retention": {
            "class": "owner_local",
            "path_policy": "agentmesh app run writes host sidecars under --sidecar-dir/YYYY-MM-DD/<run-id>/full.json; this compact payload retains only transcript digests, not raw transcripts",
        },
    })
}

fn required_string(
    object: &Map<String, Value>,
    field: &str,
    diagnostics: &mut Vec<Value>,
) -> Option<String> {
    required_string_at(object, field, &format!("/{field}"), diagnostics)
}

fn required_string_at(
    object: &Map<String, Value>,
    field: &str,
    pointer: &str,
    diagnostics: &mut Vec<Value>,
) -> Option<String> {
    match object.get(field).and_then(Value::as_str) {
        Some(value) if !value.trim().is_empty() => Some(value.to_string()),
        _ => {
            push_diag(
                diagnostics,
                format!("{}_missing", field.replace('-', "_")),
                format!("{pointer} must be a non-empty string"),
                "error",
                Some(pointer),
            );
            None
        }
    }
}

fn string_array(value: &Value, pointer: &str, diagnostics: &mut Vec<Value>) -> Vec<String> {
    let Some(items) = value.as_array() else {
        push_diag(
            diagnostics,
            "string_array_invalid",
            format!("{pointer} must be an array of strings"),
            "error",
            Some(pointer),
        );
        return Vec::new();
    };
    let mut strings = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if let Some(value) = item.as_str() {
            strings.push(value.to_string());
        } else {
            push_diag(
                diagnostics,
                "string_array_item_invalid",
                format!("{pointer}/{index} must be a string"),
                "error",
                Some(&format!("{pointer}/{index}")),
            );
        }
    }
    strings
}

fn append_input_diagnostics(value: Option<&Value>, diagnostics: &mut Vec<Value>) {
    let Some(value) = value else {
        return;
    };
    let Some(items) = value.as_array() else {
        push_diag(
            diagnostics,
            "diagnostics_invalid",
            "diagnostics must be an array when provided",
            "error",
            Some("/diagnostics"),
        );
        return;
    };
    for (index, item) in items.iter().enumerate() {
        let Some(object) = item.as_object() else {
            push_diag(
                diagnostics,
                "diagnostic_invalid",
                format!("/diagnostics/{index} must be an object"),
                "error",
                Some(&format!("/diagnostics/{index}")),
            );
            continue;
        };
        let code = object
            .get("code")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("diagnostic_code_missing");
        let message = object
            .get("message")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("diagnostic message missing");
        let severity = object
            .get("severity")
            .and_then(Value::as_str)
            .filter(|value| ["error", "warning", "info"].contains(value))
            .unwrap_or("error");
        let field = object.get("field").and_then(Value::as_str);
        push_diag(diagnostics, code, message, severity, field);
    }
}

fn push_diag(
    diagnostics: &mut Vec<Value>,
    code: impl Into<String>,
    message: impl Into<String>,
    severity: &str,
    field: Option<&str>,
) {
    diagnostics.push(json!({
        "code": code.into(),
        "field": field,
        "severity": severity,
        "message": message.into(),
    }));
}

fn diagnostic_sort_key(value: &Value) -> (String, String, String, String) {
    (
        value
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        value
            .get("field")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        value
            .get("severity")
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

fn digest_value(value: &Value) -> String {
    let bytes =
        serde_json::to_vec(&canonical_json(value)).expect("canonical digest input serializes");
    sha256_hex(&bytes)
}

fn sha256_hex(input: &[u8]) -> String {
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

    let mut hash = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (input.len() as u64) * 8;
    let mut message = input.to_vec();
    message.push(0x80);
    while (message.len() + 8) % 64 != 0 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks_exact(64) {
        let mut words = [0u32; 64];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            let start = index * 4;
            *word = u32::from_be_bytes([
                chunk[start],
                chunk[start + 1],
                chunk[start + 2],
                chunk[start + 3],
            ]);
        }
        for index in 16..64 {
            words[index] = small_sigma1(words[index - 2])
                .wrapping_add(words[index - 7])
                .wrapping_add(small_sigma0(words[index - 15]))
                .wrapping_add(words[index - 16]);
        }

        let mut a = hash[0];
        let mut b = hash[1];
        let mut c = hash[2];
        let mut d = hash[3];
        let mut e = hash[4];
        let mut f = hash[5];
        let mut g = hash[6];
        let mut h = hash[7];

        for index in 0..64 {
            let t1 = h
                .wrapping_add(big_sigma1(e))
                .wrapping_add((e & f) ^ ((!e) & g))
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let t2 = big_sigma0(a).wrapping_add((a & b) ^ (a & c) ^ (b & c));
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        for (slot, value) in hash.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut out = String::with_capacity(64);
    for word in hash {
        use std::fmt::Write as _;
        write!(&mut out, "{word:08x}").expect("writing to string cannot fail");
    }
    out
}

fn big_sigma0(value: u32) -> u32 {
    value.rotate_right(2) ^ value.rotate_right(13) ^ value.rotate_right(22)
}

fn big_sigma1(value: u32) -> u32 {
    value.rotate_right(6) ^ value.rotate_right(11) ^ value.rotate_right(25)
}

fn small_sigma0(value: u32) -> u32 {
    value.rotate_right(7) ^ value.rotate_right(18) ^ (value >> 3)
}

fn small_sigma1(value: u32) -> u32 {
    value.rotate_right(17) ^ value.rotate_right(19) ^ (value >> 10)
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

/// Build a deterministic rollback replay evidence bundle from a shared parser payload.
///
/// The App intentionally accepts only `agentmesh request parse` compact output for
/// `agentmesh-request.v0` plus already-retained adapter/protocol artifacts. It does
/// not reparse Markdown or call Multica, so the same input can be replayed by any
/// adapter runner and compared by hash.
pub fn evaluate_public_0x_rollback_replay_input(value: &Value) -> Value {
    let mut issues = Vec::new();
    let Some(object) = value.as_object() else {
        return rollback_replay_compact(
            None,
            vec![issue("input_invalid", "input must be a JSON object")],
        );
    };

    if object.get("schema_version").and_then(Value::as_str)
        != Some(ROLLBACK_REPLAY_INPUT_SCHEMA_VERSION)
    {
        issues.push(issue(
            "unsupported_schema_version",
            format!("schema_version must be {ROLLBACK_REPLAY_INPUT_SCHEMA_VERSION}"),
        ));
    }

    let request_parse = object.get("request_parse").unwrap_or(&Value::Null);
    if request_parse
        .get("request_schema_version")
        .and_then(Value::as_str)
        != Some("agentmesh-request.v0")
    {
        issues.push(issue(
            "request_schema_mismatch",
            "request_parse.request_schema_version must be agentmesh-request.v0",
        ));
    }
    if request_parse.get("valid").and_then(Value::as_bool) != Some(true) {
        issues.push(issue(
            "request_parse_invalid",
            "request_parse must be a valid shared parser output",
        ));
    }
    let canonical = request_parse.get("canonical").unwrap_or(&Value::Null);
    if canonical.get("request_kind").and_then(Value::as_str) != Some("app") {
        issues.push(issue(
            "request_kind_unsupported",
            "rollback replay accepts only request.v0 app payloads",
        ));
    }

    let manifest_hash = required_rollback_string(object, "manifest_hash", &mut issues);
    let adapter_digest = object.get("adapter_digest").unwrap_or(&Value::Null);
    if adapter_digest
        .get("request_schema_version")
        .and_then(Value::as_str)
        != Some("agentmesh-request.v0")
    {
        issues.push(issue(
            "adapter_digest_schema_mismatch",
            "adapter_digest must cover agentmesh-request.v0",
        ));
    }
    match adapter_digest.get("sections").and_then(Value::as_array) {
        Some(sections) if !sections.is_empty() => {}
        _ => issues.push(issue(
            "adapter_digest_missing",
            "adapter_digest.sections must be a non-empty array for parity replay",
        )),
    }

    let protocol_replay = object.get("protocol_replay").unwrap_or(&Value::Null);
    match protocol_replay.as_array() {
        Some(steps) if !steps.is_empty() => {
            for (index, step) in steps.iter().enumerate() {
                if step
                    .get("step")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    issues.push(issue(
                        "protocol_replay_step_missing",
                        format!("protocol_replay[{index}].step must be provided"),
                    ));
                }
                if step
                    .get("artifact")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    issues.push(issue(
                        "protocol_replay_artifact_missing",
                        format!("protocol_replay[{index}].artifact must be provided"),
                    ));
                }
            }
        }
        _ => issues.push(issue(
            "protocol_replay_missing",
            "protocol_replay must contain at least one retained replay artifact",
        )),
    }

    let rollback = object.get("rollback").unwrap_or(&Value::Null);
    for field in [
        "previous_good_artifact",
        "rollback_command",
        "verification_command",
    ] {
        if rollback
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            issues.push(issue(
                "rollback_field_missing",
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

    if !issues.is_empty() {
        return rollback_replay_compact(None, issues);
    }

    let bundle = json!({
        "manifest_hash": manifest_hash,
        "adapter_digest_hash": sha256_json(adapter_digest),
        "replay_transcript_hash": sha256_json(protocol_replay),
        "request_hash": sha256_json(canonical),
        "protocol_replay": protocol_replay,
        "rollback": rollback,
        "evidence_retention": retention,
    });
    rollback_replay_compact(Some(bundle), Vec::new())
}

fn required_rollback_string(
    object: &Map<String, Value>,
    field: &str,
    issues: &mut Vec<Value>,
) -> Option<String> {
    let value = object
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if value.is_empty() {
        issues.push(issue(
            "required_field_missing",
            format!("{field} must be provided"),
        ));
        None
    } else {
        Some(value.to_string())
    }
}

fn rollback_replay_compact(bundle: Option<Value>, issues: Vec<Value>) -> Value {
    let valid = issues.is_empty();
    let adapter_error = (!valid).then(|| {
        normalize_adapter_errors(&json!({
            "schema_version": "adapter-error-contract-input.v0",
            "source_adapter": PUBLIC_0X_ROLLBACK_REPLAY_VERSION,
            "adapter_failure": {
                "kind": "contract",
                "native_code": "rollback_replay_input_invalid",
                "message": "rollback replay input failed deterministic contract validation",
                "retryable": false
            }
        }))
    });
    json!({
        "schema_version": ROLLBACK_REPLAY_OUTPUT_SCHEMA_VERSION,
        "app_version": PUBLIC_0X_ROLLBACK_REPLAY_VERSION,
        "request_schema_version": "agentmesh-request.v0",
        "valid": valid,
        "rollback_bundle": bundle,
        "issue_count": issues.len(),
        "issues": issues,
        "adapter_error": adapter_error,
    })
}

fn sha256_json(value: &Value) -> String {
    let bytes = serde_json::to_vec(&canonical_json(value)).expect("canonical JSON serializes");
    hex::encode(Sha256::digest(bytes))
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            let mut sorted = Map::new();
            for (key, value) in entries {
                sorted.insert(key.clone(), canonical_json(value));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_json).collect()),
        _ => value.clone(),
    }
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

    #[test]
    fn adapter_evidence_envelope_fixtures_are_deterministic() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "evidence_envelope_success_input.json",
                "expected_evidence_envelope_success_payload.json",
            ),
            (
                "evidence_envelope_malformed_input.json",
                "expected_evidence_envelope_malformed_payload.json",
            ),
            (
                "evidence_envelope_parity_mismatch_input.json",
                "expected_evidence_envelope_parity_mismatch_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            assert_eq!(
                build_adapter_evidence_envelope_input(&input),
                expected,
                "{input_name} should match {expected_name}"
            );
        }
    }

    #[test]
    fn adapter_evidence_digest_uses_canonical_object_order() {
        assert_eq!(
            digest_value(&json!({"b": 1, "a": {"d": 2, "c": 3}})),
            digest_value(&json!({"a": {"c": 3, "d": 2}, "b": 1}))
        );
    }

    #[test]
    fn adapter_evidence_envelope_covers_validation_and_execution_phases() {
        let validation = build_adapter_evidence_envelope_input(&json!({
            "schema_version": EVIDENCE_ENVELOPE_INPUT_SCHEMA_VERSION,
            "request_id": "DOT-1279",
            "phase": "validation",
            "adapter": {"id": "a", "capabilities": []},
            "capability": {"name": "cap", "schema_version": "cap.v0", "operation": "validate"},
            "result": {"class": "success"},
            "transcript": []
        }));
        let execution = build_adapter_evidence_envelope_input(&json!({
            "schema_version": EVIDENCE_ENVELOPE_INPUT_SCHEMA_VERSION,
            "request_id": "DOT-1279",
            "phase": "execution",
            "adapter": {"id": "a", "capabilities": []},
            "capability": {"name": "cap", "schema_version": "cap.v0", "operation": "execute"},
            "result": {"class": "success"},
            "transcript": []
        }));

        assert_eq!(validation["valid"], true);
        assert_eq!(execution["valid"], true);
        assert_eq!(validation["phase"], "validation");
        assert_eq!(execution["phase"], "execution");
        assert_ne!(validation["capability_hash"], execution["capability_hash"]);
    }

    #[test]
    fn rollback_replay_fixture_is_stable_and_hashes_expected_artifacts() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let input: Value = serde_json::from_slice(
            &std::fs::read(root.join("public_0x_rollback_replay_input.json")).unwrap(),
        )
        .unwrap();
        let output = evaluate_public_0x_rollback_replay_input(&input);

        assert_eq!(output["valid"], true);
        assert_eq!(output["issue_count"], 0);
        assert_eq!(output["adapter_error"], Value::Null);
        assert_eq!(
            output["rollback_bundle"]["manifest_hash"],
            "sha256:agentmesh-app-manifest-fixture"
        );
        assert_eq!(
            output["rollback_bundle"]["adapter_digest_hash"],
            "4e400b93bdf59876ee0eadf5df12ca3a830e7040484bf4db3087785500dd5259"
        );
        assert_eq!(
            output["rollback_bundle"]["replay_transcript_hash"],
            "705bb873f99257f8951ec72d2a0767811a8dd346fb42da7ca1d92bb6eb1a63eb"
        );
        assert_eq!(
            output["rollback_bundle"]["request_hash"],
            "f9ca45f75596bb88bc8d3dee0ded3b2cd3e94575838539aec2de9023a108e868"
        );
    }

    #[test]
    fn rollback_replay_rejects_empty_digest_sections_and_missing_step() {
        let output = evaluate_public_0x_rollback_replay_input(&json!({
            "schema_version": "public-0x-rollback-replay-input.v0",
            "request_parse": {"request_schema_version": "agentmesh-request.v0", "valid": true, "canonical": {"request_kind": "app"}},
            "manifest_hash": "sha256:manifest",
            "adapter_digest": {"request_schema_version": "agentmesh-request.v0", "sections": []},
            "protocol_replay": [{"artifact": "rollback.log"}],
            "rollback": {
                "previous_good_artifact": "agentmesh-v0.2.0-dev.1",
                "rollback_command": "git revert <sha>",
                "verification_command": "cargo test --workspace"
            },
            "evidence_retention": {"location": "durable-review", "retention_days": 30}
        }));

        assert_eq!(output["valid"], false);
        assert_eq!(output["issues"][0]["code"], "adapter_digest_missing");
        assert_eq!(output["issues"][1]["code"], "protocol_replay_step_missing");
    }

    #[test]
    fn rollback_replay_rejects_non_request_v0_with_normalized_adapter_error() {
        let output = evaluate_public_0x_rollback_replay_input(&json!({
            "schema_version": "public-0x-rollback-replay-input.v0",
            "request_parse": {"request_schema_version": "agentmesh-request.v1", "valid": true, "canonical": {"request_kind": "app"}},
            "manifest_hash": "sha256:manifest",
            "adapter_digest": {"request_schema_version": "agentmesh-request.v1"},
            "protocol_replay": [],
            "rollback": {},
            "evidence_retention": {"retention_days": 7}
        }));

        assert_eq!(output["valid"], false);
        assert_eq!(
            output["adapter_error"]["schema_version"],
            "adapter-error-contract-compact.v0"
        );
        assert_eq!(
            output["adapter_error"]["errors"][0]["taxonomy_code"],
            "request.field_required"
        );
    }
}
