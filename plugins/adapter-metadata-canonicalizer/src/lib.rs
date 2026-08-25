//! Adapter metadata comparison and canonicalization contract.
//!
//! Compares two request metadata payloads from different adapters, promotes only
//! equal stable common fields into a canonical object, and preserves all
//! adapter-specific or drifting fields separately for downstream adapter-owned
//! handling.

use agentmesh_markdown_request_validator::adapter_error_contract::normalize_adapter_errors;
use chrono::{DateTime, FixedOffset};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

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

/// Plugin/schema version exposed by the adapter evidence traceability binary.
pub const ADAPTER_EVIDENCE_TRACEABILITY_VERSION: &str = "adapter-evidence-traceability.v0";
const TRACEABILITY_INPUT_SCHEMA_VERSION: &str = "adapter-evidence-traceability-input.v0";
const TRACEABILITY_OUTPUT_SCHEMA_VERSION: &str = "adapter-evidence-traceability-compact.v0";
const TRACEABILITY_STAGE_ORDER: &[&str] = &["request", "parser", "adapter", "evidence"];

/// Plugin/schema version exposed by the deterministic rollback replay gate binary.
pub const PUBLIC_0X_ROLLBACK_REPLAY_VERSION: &str = "public-0x-rollback-replay.v0";
const ROLLBACK_REPLAY_INPUT_SCHEMA_VERSION: &str = "public-0x-rollback-replay-input.v0";
const ROLLBACK_REPLAY_OUTPUT_SCHEMA_VERSION: &str = "public-0x-rollback-replay-compact.v0";

/// Plugin/schema version exposed by the post-dogfood public 0.x readiness report binary.
pub const PUBLIC_0X_READINESS_REPORT_VERSION: &str = "public-0x-readiness-report.v0";
const READINESS_REPORT_INPUT_SCHEMA_VERSION: &str = "public-0x-readiness-report-input.v0";
const READINESS_REPORT_OUTPUT_SCHEMA_VERSION: &str = "public-0x-readiness-report-compact.v0";
const READINESS_REPORT_EVIDENCE_DIGEST_SCHEMA_VERSION: &str =
    "agentmesh-adapter-evidence-digest.v0";
const READINESS_REPORT_REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";
const READINESS_REPORT_DEFAULT_MINIMUM_EVIDENCE_COUNT: u64 = 2;
const READINESS_REPORT_DEFAULT_FIELDS: &[&str] = &[
    "title",
    "request_kind",
    "source_prd",
    "source_design",
    "source_roadmap",
];

/// Plugin/schema version exposed by the deterministic adapter parity report binary.
pub const ADAPTER_PARITY_REPORT_VERSION: &str = "adapter-parity-report.v0";
const PARITY_REPORT_INPUT_SCHEMA_VERSION: &str = "adapter-parity-report-input.v0";
const PARITY_REPORT_OUTPUT_SCHEMA_VERSION: &str = "adapter-parity-report-compact.v0";
const PARITY_REPORT_REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";
const PARITY_CANONICAL_FIELDS: &[&str] = &[
    "title",
    "request_kind",
    "issue_type",
    "status",
    "project_key",
    "source_prd",
    "source_design",
    "source_roadmap",
    "ready_for_multica",
    "sequence_index",
    "sequence_total",
    "blocked_by",
    "unblocks",
];
const PARITY_COMMON_RESULT_FIELDS: &[&str] = &[
    "schema_version",
    "app_version",
    "adapter_version",
    "request_schema_version",
    "valid",
    "canonical",
    "evidence_digest",
    "error_count",
    "errors",
    "issue_count",
    "issues",
    "diagnostic_model",
    "diagnostic_count",
    "diagnostics",
    "deterministic_diagnostics",
    "adapter_error",
    "result_class",
];

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

#[derive(Debug, Clone)]
struct ParitySide {
    side: &'static str,
    adapter_id: String,
    request_id: Option<String>,
    result: Map<String, Value>,
}

#[derive(Debug)]
struct ParityReportParts {
    request_id: Option<String>,
    canonical_field_order: Vec<String>,
    matching_canonical_fields: Vec<Value>,
    canonical_mismatches: Vec<Value>,
    matching_extension_paths: Vec<String>,
    extension_mismatches: Vec<Value>,
    adapters: Vec<Value>,
    normalized_errors: Vec<Value>,
    error_mismatches: Vec<Value>,
    diagnostics: Vec<Value>,
}

impl ParityReportParts {
    fn empty(
        request_id: Option<String>,
        canonical_field_order: Vec<String>,
        diagnostics: Vec<Value>,
    ) -> Self {
        Self {
            request_id,
            canonical_field_order,
            matching_canonical_fields: Vec::new(),
            canonical_mismatches: Vec::new(),
            matching_extension_paths: Vec::new(),
            extension_mismatches: Vec::new(),
            adapters: Vec::new(),
            normalized_errors: Vec::new(),
            error_mismatches: Vec::new(),
            diagnostics,
        }
    }
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

/// Build a deterministic parity report for two validated adapter result payloads.
///
/// The report compares only the shared canonical projection as common contract
/// data. Everything outside that projection is retained under adapter
/// extensions and compared separately, so local and non-Multica runners can
/// identify drift without importing tracker-specific types.
pub fn build_adapter_parity_report_input(value: &Value) -> Value {
    let mut diagnostics = Vec::new();
    let mut request_id = None;
    let mut canonical_field_order: Vec<String> = PARITY_CANONICAL_FIELDS
        .iter()
        .map(|field| (*field).to_string())
        .collect();

    let Some(object) = value.as_object() else {
        diagnostics.push(parity_diagnostic(
            "input_invalid",
            "input must be a JSON object",
            Some("/"),
        ));
        return adapter_parity_compact(ParityReportParts::empty(
            request_id,
            canonical_field_order,
            diagnostics,
        ));
    };

    if object.get("schema_version").and_then(Value::as_str)
        != Some(PARITY_REPORT_INPUT_SCHEMA_VERSION)
    {
        diagnostics.push(parity_diagnostic(
            "unsupported_schema_version",
            format!("schema_version must be {PARITY_REPORT_INPUT_SCHEMA_VERSION}"),
            Some("/schema_version"),
        ));
    }

    request_id = required_parity_string(object, "request_id", "/request_id", &mut diagnostics);
    if let Some(fields) = object.get("canonical_fields") {
        canonical_field_order = parse_parity_field_order(fields, &mut diagnostics);
    }

    let left = parse_parity_side("left", object.get("left"), &mut diagnostics);
    let right = parse_parity_side("right", object.get("right"), &mut diagnostics);

    let Some(left) = left else {
        return adapter_parity_compact(ParityReportParts::empty(
            request_id,
            canonical_field_order,
            diagnostics,
        ));
    };
    let Some(right) = right else {
        return adapter_parity_compact(ParityReportParts::empty(
            request_id,
            canonical_field_order,
            diagnostics,
        ));
    };

    if left.adapter_id == right.adapter_id {
        diagnostics.push(parity_diagnostic(
            "adapter_id_duplicate",
            "left.adapter_id and right.adapter_id must identify different adapters",
            Some("/right/adapter_id"),
        ));
    }

    let left_canonical =
        parity_canonical_projection(&left, &canonical_field_order, &mut diagnostics);
    let right_canonical =
        parity_canonical_projection(&right, &canonical_field_order, &mut diagnostics);
    let (matching_canonical_fields, canonical_mismatches) = compare_parity_canonical_fields(
        request_id.as_deref(),
        &left,
        &right,
        &left_canonical,
        &right_canonical,
        &canonical_field_order,
    );

    let left_extensions = parity_extension_projection(&left, &canonical_field_order);
    let right_extensions = parity_extension_projection(&right, &canonical_field_order);
    let left_extension_paths = parity_flatten_paths(&left_extensions);
    let right_extension_paths = parity_flatten_paths(&right_extensions);
    let (matching_extension_paths, extension_mismatches) =
        compare_parity_extensions(&left_extension_paths, &right_extension_paths);

    let (left_errors, left_error_signature) = parity_error_summary(&left);
    let (right_errors, right_error_signature) = parity_error_summary(&right);
    let mut error_mismatches = Vec::new();
    if left_error_signature != right_error_signature {
        error_mismatches.push(json!({
            "code": "error_class_mismatch",
            "left_signature": left_error_signature,
            "right_signature": right_error_signature,
        }));
    }

    let adapters = vec![
        parity_adapter_summary(
            &left,
            left_extensions,
            left_extension_paths.keys().cloned().collect(),
        ),
        parity_adapter_summary(
            &right,
            right_extensions,
            right_extension_paths.keys().cloned().collect(),
        ),
    ];

    adapter_parity_compact(ParityReportParts {
        request_id,
        canonical_field_order,
        matching_canonical_fields,
        canonical_mismatches,
        matching_extension_paths,
        extension_mismatches,
        adapters,
        normalized_errors: vec![left_errors, right_errors],
        error_mismatches,
        diagnostics,
    })
}

fn parse_parity_side(
    side: &'static str,
    value: Option<&Value>,
    diagnostics: &mut Vec<Value>,
) -> Option<ParitySide> {
    let path = format!("/{side}");
    let Some(value) = value else {
        diagnostics.push(parity_diagnostic(
            "adapter_result_missing",
            format!("{side} adapter result is required"),
            Some(&path),
        ));
        return None;
    };
    let Some(object) = value.as_object() else {
        diagnostics.push(parity_diagnostic(
            "adapter_result_invalid",
            format!("{side} must be a JSON object"),
            Some(&path),
        ));
        return None;
    };

    let adapter_id = required_parity_string(
        object,
        "adapter_id",
        &format!("/{side}/adapter_id"),
        diagnostics,
    );
    let result_value = object.get("result");
    let Some(result) = result_value.and_then(Value::as_object) else {
        diagnostics.push(parity_diagnostic(
            "adapter_result_invalid",
            format!("{side}.result must be a JSON object"),
            Some(&format!("/{side}/result")),
        ));
        return None;
    };
    let request_id = optional_parity_string(
        object,
        "request_id",
        &format!("/{side}/request_id"),
        diagnostics,
    )
    .or_else(|| adapter_result_request_id(result));

    adapter_id.map(|adapter_id| ParitySide {
        side,
        adapter_id,
        request_id,
        result: result.clone(),
    })
}

fn required_parity_string(
    object: &Map<String, Value>,
    field: &str,
    path: &str,
    diagnostics: &mut Vec<Value>,
) -> Option<String> {
    match object.get(field) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        Some(_) => {
            diagnostics.push(parity_diagnostic(
                "required_field_invalid",
                format!("{path} must be a non-empty string"),
                Some(path),
            ));
            None
        }
        None => {
            diagnostics.push(parity_diagnostic(
                "required_field_missing",
                format!("{path} is required"),
                Some(path),
            ));
            None
        }
    }
}

fn optional_parity_string(
    object: &Map<String, Value>,
    field: &str,
    path: &str,
    diagnostics: &mut Vec<Value>,
) -> Option<String> {
    match object.get(field) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        Some(_) => {
            diagnostics.push(parity_diagnostic(
                "optional_field_invalid",
                format!("{path} must be a non-empty string when provided"),
                Some(path),
            ));
            None
        }
        None => None,
    }
}

fn parse_parity_field_order(value: &Value, diagnostics: &mut Vec<Value>) -> Vec<String> {
    let Some(items) = value.as_array() else {
        diagnostics.push(parity_diagnostic(
            "canonical_fields_invalid",
            "canonical_fields must be an array of non-empty strings",
            Some("/canonical_fields"),
        ));
        return PARITY_CANONICAL_FIELDS
            .iter()
            .map(|field| (*field).to_string())
            .collect();
    };

    let mut fields = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let Some(field) = item.as_str().filter(|field| !field.trim().is_empty()) else {
            diagnostics.push(parity_diagnostic(
                "canonical_fields_invalid",
                format!("canonical_fields[{index}] must be a non-empty string"),
                Some(&format!("/canonical_fields/{index}")),
            ));
            continue;
        };
        if seen.insert(field.to_string()) {
            fields.push(field.to_string());
        } else {
            diagnostics.push(parity_diagnostic(
                "canonical_fields_duplicate",
                format!("canonical field {field:?} is duplicated"),
                Some(&format!("/canonical_fields/{index}")),
            ));
        }
    }

    if fields.is_empty() {
        diagnostics.push(parity_diagnostic(
            "canonical_fields_invalid",
            "canonical_fields must contain at least one field",
            Some("/canonical_fields"),
        ));
        PARITY_CANONICAL_FIELDS
            .iter()
            .map(|field| (*field).to_string())
            .collect()
    } else {
        fields
    }
}

fn adapter_result_request_id(result: &Map<String, Value>) -> Option<String> {
    result
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            result
                .get("canonical")
                .and_then(|canonical| canonical.get("request_id"))
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            result
                .get("canonical")
                .and_then(|canonical| canonical.get("fields"))
                .and_then(|fields| fields.get("id"))
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
        })
}

fn parity_canonical_projection(
    side: &ParitySide,
    field_order: &[String],
    diagnostics: &mut Vec<Value>,
) -> BTreeMap<String, Value> {
    let mut projection = BTreeMap::new();
    let Some(canonical) = side.result.get("canonical") else {
        diagnostics.push(parity_diagnostic(
            "canonical_missing",
            format!(
                "{}.result.canonical is required for canonical parity",
                side.side
            ),
            Some(&format!("/{}/result/canonical", side.side)),
        ));
        return projection;
    };
    let Some(canonical_object) = canonical.as_object() else {
        diagnostics.push(parity_diagnostic(
            "canonical_invalid",
            format!("{}.result.canonical must be a JSON object", side.side),
            Some(&format!("/{}/result/canonical", side.side)),
        ));
        return projection;
    };

    let field_source = canonical_object
        .get("fields")
        .and_then(Value::as_object)
        .unwrap_or(canonical_object);
    for field in field_order {
        if let Some(value) = field_source.get(field) {
            projection.insert(field.clone(), canonical_json(value));
        }
    }
    projection
}

fn parity_canonical_extra(side: &ParitySide, field_order: &[String]) -> Map<String, Value> {
    let mut extras = Map::new();
    let field_set: BTreeSet<&str> = field_order.iter().map(String::as_str).collect();
    let Some(canonical_object) = side.result.get("canonical").and_then(Value::as_object) else {
        return extras;
    };
    let field_source = canonical_object
        .get("fields")
        .and_then(Value::as_object)
        .unwrap_or(canonical_object);
    for (key, value) in field_source {
        if key == "schema_version" || key == "request_schema_version" || key == "field_order" {
            continue;
        }
        if !field_set.contains(key.as_str()) {
            extras.insert(key.clone(), canonical_json(value));
        }
    }
    extras
}

fn compare_parity_canonical_fields(
    request_id: Option<&str>,
    left: &ParitySide,
    right: &ParitySide,
    left_canonical: &BTreeMap<String, Value>,
    right_canonical: &BTreeMap<String, Value>,
    field_order: &[String],
) -> (Vec<Value>, Vec<Value>) {
    let mut matches = Vec::new();
    let mut mismatches = Vec::new();

    compare_parity_request_ids(request_id, left, right, &mut mismatches);

    for field in field_order {
        match (left_canonical.get(field), right_canonical.get(field)) {
            (Some(left_value), Some(right_value)) if left_value == right_value => {
                matches.push(json!({"field": field, "value": left_value}));
            }
            (Some(left_value), Some(right_value)) => mismatches.push(json!({
                "code": "canonical_value_mismatch",
                "field": field,
                "left": left_value,
                "right": right_value,
            })),
            (Some(left_value), None) => mismatches.push(json!({
                "code": "canonical_presence_mismatch",
                "field": field,
                "left_present": true,
                "right_present": false,
                "left": left_value,
                "right": Value::Null,
            })),
            (None, Some(right_value)) => mismatches.push(json!({
                "code": "canonical_presence_mismatch",
                "field": field,
                "left_present": false,
                "right_present": true,
                "left": Value::Null,
                "right": right_value,
            })),
            (None, None) => {}
        }
    }

    (matches, mismatches)
}

fn compare_parity_request_ids(
    request_id: Option<&str>,
    left: &ParitySide,
    right: &ParitySide,
    mismatches: &mut Vec<Value>,
) {
    if let (Some(left_id), Some(right_id)) = (&left.request_id, &right.request_id) {
        if left_id != right_id {
            mismatches.push(json!({
                "code": "request_id_mismatch",
                "field": "request_id",
                "left": left_id,
                "right": right_id,
            }));
        }
    }

    for side in [left, right] {
        let Some(expected) = request_id else {
            continue;
        };
        if let Some(actual) = &side.request_id {
            if actual != expected {
                mismatches.push(json!({
                    "code": "request_id_mismatch",
                    "field": "request_id",
                    "side": side.side,
                    "expected": expected,
                    "actual": actual,
                }));
            }
        }
    }
}

fn parity_extension_projection(side: &ParitySide, field_order: &[String]) -> Value {
    let common_fields: BTreeSet<&str> = PARITY_COMMON_RESULT_FIELDS.iter().copied().collect();
    let mut extensions = Map::new();
    for (key, value) in &side.result {
        if common_fields.contains(key.as_str()) {
            continue;
        }
        extensions.insert(key.clone(), canonical_json(value));
    }

    let canonical_extra = parity_canonical_extra(side, field_order);
    if !canonical_extra.is_empty() {
        extensions.insert("canonical_extra".into(), Value::Object(canonical_extra));
    }

    Value::Object(extensions)
}

fn parity_flatten_paths(value: &Value) -> BTreeMap<String, Value> {
    let mut paths = BTreeMap::new();
    if let Some(object) = value.as_object() {
        for (key, child) in object {
            flatten_extension_path(key, child, &mut paths);
        }
    }
    paths
}

fn flatten_extension_path(path: &str, value: &Value, paths: &mut BTreeMap<String, Value>) {
    match value {
        Value::Object(object) if !object.is_empty() => {
            for (key, child) in object {
                flatten_extension_path(&format!("{path}.{key}"), child, paths);
            }
        }
        _ => {
            paths.insert(path.to_string(), canonical_json(value));
        }
    }
}

fn compare_parity_extensions(
    left: &BTreeMap<String, Value>,
    right: &BTreeMap<String, Value>,
) -> (Vec<String>, Vec<Value>) {
    let mut matches = Vec::new();
    let mut mismatches = Vec::new();
    let keys: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
    for key in keys {
        match (left.get(key), right.get(key)) {
            (Some(left_value), Some(right_value)) if left_value == right_value => {
                matches.push(key.clone());
            }
            (Some(left_value), Some(right_value)) => mismatches.push(json!({
                "code": "extension_value_mismatch",
                "path": key,
                "left": left_value,
                "right": right_value,
            })),
            (Some(left_value), None) => mismatches.push(json!({
                "code": "extension_presence_mismatch",
                "path": key,
                "left_present": true,
                "right_present": false,
                "left": left_value,
                "right": Value::Null,
            })),
            (None, Some(right_value)) => mismatches.push(json!({
                "code": "extension_presence_mismatch",
                "path": key,
                "left_present": false,
                "right_present": true,
                "left": Value::Null,
                "right": right_value,
            })),
            (None, None) => {}
        }
    }
    (matches, mismatches)
}

fn parity_error_summary(side: &ParitySide) -> (Value, Vec<String>) {
    let result_class = parity_result_class(&side.result);
    let valid = side.result.get("valid").and_then(Value::as_bool);
    let mut records: BTreeMap<(String, String, String, String), u64> = BTreeMap::new();
    collect_error_records(side.result.get("errors"), "adapter", &mut records);
    collect_error_records(side.result.get("issues"), "adapter", &mut records);
    collect_error_records(side.result.get("diagnostics"), "adapter", &mut records);
    collect_error_records(
        side.result
            .get("deterministic_diagnostics")
            .and_then(|diagnostics| diagnostics.get("items")),
        "adapter",
        &mut records,
    );
    collect_error_records(
        side.result
            .get("adapter_error")
            .and_then(|adapter_error| adapter_error.get("errors")),
        "adapter",
        &mut records,
    );

    let records: Vec<Value> = records
        .iter()
        .map(|((taxonomy_code, code, severity, source), count)| {
            json!({
                "taxonomy_code": taxonomy_code,
                "code": code,
                "severity": severity,
                "source": source,
                "count": count,
            })
        })
        .collect();
    let mut signature = vec![format!("result_class={result_class}")];
    signature.extend(records.iter().map(|record| {
        format!(
            "{}|{}|{}|{}|{}",
            record["taxonomy_code"].as_str().unwrap_or("unknown"),
            record["code"].as_str().unwrap_or("unknown"),
            record["severity"].as_str().unwrap_or("error"),
            record["source"].as_str().unwrap_or("adapter"),
            record["count"].as_u64().unwrap_or(0),
        )
    }));

    (
        json!({
            "side": side.side,
            "adapter_id": side.adapter_id,
            "valid": valid,
            "result_class": result_class,
            "error_count": records.iter().map(|record| record["count"].as_u64().unwrap_or(0)).sum::<u64>(),
            "records": records,
        }),
        signature,
    )
}

fn parity_result_class(result: &Map<String, Value>) -> String {
    if let Some(class) = result
        .get("result_class")
        .and_then(Value::as_str)
        .filter(|class| !class.trim().is_empty())
    {
        return class.to_string();
    }
    match result.get("valid").and_then(Value::as_bool) {
        Some(true) => "success".into(),
        Some(false) if has_error_records(result) => "adapter_error".into(),
        Some(false) => "malformed_input".into(),
        None => "unknown".into(),
    }
}

fn has_error_records(result: &Map<String, Value>) -> bool {
    ["errors", "issues", "diagnostics"].iter().any(|key| {
        result
            .get(*key)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    }) || result
        .get("deterministic_diagnostics")
        .and_then(|diagnostics| diagnostics.get("items"))
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
        || result
            .get("adapter_error")
            .and_then(|adapter_error| adapter_error.get("errors"))
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
}

fn collect_error_records(
    value: Option<&Value>,
    default_source: &str,
    records: &mut BTreeMap<(String, String, String, String), u64>,
) {
    let Some(items) = value.and_then(Value::as_array) else {
        return;
    };
    for item in items {
        let Some(record) = item.as_object() else {
            continue;
        };
        let code = record
            .get("code")
            .and_then(Value::as_str)
            .filter(|code| !code.trim().is_empty())
            .unwrap_or("unknown")
            .to_string();
        let taxonomy_code = record
            .get("taxonomy_code")
            .and_then(Value::as_str)
            .filter(|code| !code.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| parity_taxonomy_code(&code));
        let severity = record
            .get("severity")
            .and_then(Value::as_str)
            .filter(|severity| !severity.trim().is_empty())
            .unwrap_or("error")
            .to_string();
        let source = record
            .get("source")
            .and_then(Value::as_str)
            .filter(|source| !source.trim().is_empty())
            .unwrap_or(default_source)
            .to_string();
        *records
            .entry((taxonomy_code, code, severity, source))
            .or_insert(0) += 1;
    }
}

fn parity_taxonomy_code(code: &str) -> String {
    match code {
        "AGENTMESH_INPUT_SCHEMA_INVALID" | "input_invalid" | "unsupported_schema_version" => {
            "request.input_schema_invalid".into()
        }
        "AGENTMESH_MARKDOWN_INVALID" | "markdown_invalid" => "request.markdown_invalid".into(),
        "AGENTMESH_FIELD_REQUIRED" | "field_required" | "required_field_missing" => {
            "request.field_required".into()
        }
        "AGENTMESH_CAPABILITY_UNKNOWN" | "capability_unknown" => {
            "request.capability_unknown".into()
        }
        "AGENTMESH_BOUNDARY_EXCEEDED" | "boundary_exceeded" => "request.boundary_exceeded".into(),
        "adapter_parity_mismatch" | "canonical_value_mismatch" | "extension_value_mismatch" => {
            "adapter.parity_mismatch".into()
        }
        "adapter_timeout" => "adapter.timeout".into(),
        "adapter_rate_limited" => "adapter.rate_limited".into(),
        "adapter_auth_failed" => "adapter.auth_failed".into(),
        "AGENTMESH_EXTERNAL_ADAPTER_FAILURE" | "adapter_external_failure" => {
            "adapter.external_failure".into()
        }
        other => other.to_ascii_lowercase().replace('_', "."),
    }
}

fn parity_adapter_summary(
    side: &ParitySide,
    extensions: Value,
    extension_paths: Vec<String>,
) -> Value {
    json!({
        "side": side.side,
        "adapter_id": side.adapter_id,
        "request_id": side.request_id,
        "schema_version": side.result.get("schema_version").and_then(Value::as_str),
        "adapter_version": side
            .result
            .get("adapter_version")
            .or_else(|| side.result.get("app_version"))
            .and_then(Value::as_str),
        "extension_paths": extension_paths,
        "extensions": extensions,
    })
}

fn adapter_parity_compact(parts: ParityReportParts) -> Value {
    let mismatch_count = parts.canonical_mismatches.len()
        + parts.extension_mismatches.len()
        + parts.error_mismatches.len();
    let valid = parts.diagnostics.is_empty() && mismatch_count == 0;
    let parity_status = if parts.diagnostics.is_empty() {
        if mismatch_count == 0 {
            "match"
        } else {
            "mismatch"
        }
    } else {
        "input_invalid"
    };

    json!({
        "schema_version": PARITY_REPORT_OUTPUT_SCHEMA_VERSION,
        "app_version": ADAPTER_PARITY_REPORT_VERSION,
        "request_schema_version": PARITY_REPORT_REQUEST_SCHEMA_VERSION,
        "valid": valid,
        "parity_status": parity_status,
        "request_id": parts.request_id,
        "canonical_field_order": parts.canonical_field_order,
        "matching_canonical_fields": parts.matching_canonical_fields,
        "canonical_mismatch_count": parts.canonical_mismatches.len(),
        "canonical_mismatches": parts.canonical_mismatches,
        "matching_extension_paths": parts.matching_extension_paths,
        "extension_mismatch_count": parts.extension_mismatches.len(),
        "extension_mismatches": parts.extension_mismatches,
        "normalized_errors": parts.normalized_errors,
        "error_mismatch_count": parts.error_mismatches.len(),
        "error_mismatches": parts.error_mismatches,
        "adapters": parts.adapters,
        "mismatch_count": mismatch_count,
        "diagnostic_count": parts.diagnostics.len(),
        "diagnostics": parts.diagnostics,
    })
}

fn parity_diagnostic(code: &str, message: impl Into<String>, path: Option<&str>) -> Value {
    json!({
        "code": code,
        "severity": "error",
        "path": path,
        "message": message.into(),
    })
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

/// Build a deterministic request-to-evidence correlation graph.
///
/// The traceability App intentionally consumes retained artifacts supplied by
/// the caller. It never reads tracker state or replay files itself; instead it
/// normalizes artifact references, canonical payload digests, and explicit
/// missing-data diagnostics into one stable graph that downstream tooling can
/// compare across adapters.
pub fn build_adapter_evidence_traceability_input(value: &Value) -> Value {
    let mut diagnostics = Vec::new();
    let Some(object) = value.as_object() else {
        trace_diag(
            &mut diagnostics,
            "input_invalid",
            "input",
            Some("/"),
            "input must be a JSON object",
            "error",
        );
        let request = trace_request(None, None, None, None);
        let stages = trace_stage_values(&request, None, None, None, None, None, None);
        return traceability_compact(request, stages, Vec::new(), diagnostics);
    };

    if object.get("schema_version").and_then(Value::as_str)
        != Some(TRACEABILITY_INPUT_SCHEMA_VERSION)
    {
        trace_diag(
            &mut diagnostics,
            "unsupported_schema_version",
            "input",
            Some("/schema_version"),
            format!("schema_version must be {TRACEABILITY_INPUT_SCHEMA_VERSION}"),
            "error",
        );
    }

    let request_object = trace_required_object(
        object.get("request"),
        "request",
        "/request",
        &mut diagnostics,
    );
    let request_id = request_object.as_ref().and_then(|request| {
        trace_required_string_at(request, "id", "request", "/request/id", &mut diagnostics)
    });
    let request_title = request_object.as_ref().and_then(|request| {
        trace_optional_string_at(
            request,
            "title",
            "request",
            "/request/title",
            &mut diagnostics,
        )
    });
    let request_source_file = request_object.as_ref().and_then(|request| {
        trace_required_string_at(
            request,
            "source_file",
            "request",
            "/request/source_file",
            &mut diagnostics,
        )
    });
    let request_source_line = request_object.as_ref().and_then(|request| {
        trace_optional_u64_at(
            request,
            "source_line",
            "request",
            "/request/source_line",
            &mut diagnostics,
        )
    });
    let request = trace_request(
        request_id.clone(),
        request_title,
        request_source_file.clone(),
        request_source_line,
    );

    let parser_object =
        trace_required_object(object.get("parser"), "parser", "/parser", &mut diagnostics);
    let parser_artifact = parser_object.as_ref().and_then(|parser| {
        trace_required_string_at(
            parser,
            "artifact",
            "parser",
            "/parser/artifact",
            &mut diagnostics,
        )
    });
    let parser_output = parser_object.as_ref().and_then(|parser| {
        trace_required_json_object_at(
            parser,
            "output",
            "parser",
            "/parser/output",
            &mut diagnostics,
        )
    });
    let parser_digest = parser_output
        .as_ref()
        .map(|payload| trace_digest("canonical JSON parser output", payload));
    let parser_correlation_id = trace_stage_correlation_id(
        "parser",
        &request_id,
        &parser_artifact,
        parser_digest.as_ref(),
        json!({}),
    );

    let adapter_object = trace_required_object(
        object.get("adapter"),
        "adapter",
        "/adapter",
        &mut diagnostics,
    );
    let adapter_id = adapter_object.as_ref().and_then(|adapter| {
        trace_required_string_at(adapter, "id", "adapter", "/adapter/id", &mut diagnostics)
    });
    let adapter_version = adapter_object.as_ref().and_then(|adapter| {
        trace_optional_string_at(
            adapter,
            "version",
            "adapter",
            "/adapter/version",
            &mut diagnostics,
        )
    });
    let adapter_artifact = adapter_object.as_ref().and_then(|adapter| {
        trace_required_string_at(
            adapter,
            "artifact",
            "adapter",
            "/adapter/artifact",
            &mut diagnostics,
        )
    });
    let adapter_output = adapter_object.as_ref().and_then(|adapter| {
        trace_required_json_object_at(
            adapter,
            "output",
            "adapter",
            "/adapter/output",
            &mut diagnostics,
        )
    });
    let adapter_identity = json!({
        "id": adapter_id,
        "version": adapter_version,
    });
    let adapter_digest = adapter_output
        .as_ref()
        .map(|payload| trace_digest("canonical JSON adapter output", payload));
    let adapter_correlation_id = trace_stage_correlation_id(
        "adapter",
        &request_id,
        &adapter_artifact,
        adapter_digest.as_ref(),
        json!({
            "adapter": adapter_identity.clone(),
            "parser_digest": parser_digest.as_ref().and_then(trace_digest_value),
        }),
    );

    let evidence_object = trace_required_object(
        object.get("evidence"),
        "evidence",
        "/evidence",
        &mut diagnostics,
    );
    let evidence_artifact = evidence_object.as_ref().and_then(|evidence| {
        trace_required_string_at(
            evidence,
            "artifact",
            "evidence",
            "/evidence/artifact",
            &mut diagnostics,
        )
    });
    let evidence_envelope = evidence_object.as_ref().and_then(|evidence| {
        trace_required_json_object_at(
            evidence,
            "envelope",
            "evidence",
            "/evidence/envelope",
            &mut diagnostics,
        )
    });
    let evidence_digest = evidence_envelope
        .as_ref()
        .map(|payload| trace_digest("canonical JSON evidence envelope", payload));
    let evidence_correlation_id = trace_stage_correlation_id(
        "evidence",
        &request_id,
        &evidence_artifact,
        evidence_digest.as_ref(),
        json!({
            "adapter": adapter_identity.clone(),
        }),
    );

    let parser_stage = json!({
        "stage": "parser",
        "status": trace_payload_stage_status(&parser_artifact, parser_digest.as_ref()),
        "correlation_id": parser_correlation_id,
        "artifact_ref": parser_artifact,
        "source": {
            "file": request_source_file,
        },
        "digest": parser_digest.clone(),
    });
    let adapter_stage = json!({
        "stage": "adapter",
        "status": trace_payload_stage_status(&adapter_artifact, adapter_digest.as_ref()),
        "correlation_id": adapter_correlation_id,
        "artifact_ref": adapter_artifact,
        "adapter": adapter_identity.clone(),
        "digest": adapter_digest.clone(),
    });
    let evidence_stage = json!({
        "stage": "evidence",
        "status": trace_payload_stage_status(&evidence_artifact, evidence_digest.as_ref()),
        "correlation_id": evidence_correlation_id,
        "artifact_ref": evidence_artifact,
        "adapter": adapter_identity.clone(),
        "digest": evidence_digest.clone(),
    });

    let stages = trace_stage_values(
        &request,
        Some(parser_stage),
        Some(adapter_stage),
        Some(evidence_stage),
        parser_digest,
        adapter_digest,
        evidence_digest,
    );
    let replay_references = trace_replay_references(object.get("replay"), &mut diagnostics);

    traceability_compact(request, stages, replay_references, diagnostics)
}

fn trace_request(
    id: Option<String>,
    title: Option<String>,
    source_file: Option<String>,
    source_line: Option<u64>,
) -> Value {
    let correlation_id = match (&id, &source_file) {
        (Some(id), Some(source_file)) => Some(trace_correlation_id(
            "request",
            &json!({
                "id": id,
                "source_file": source_file,
                "source_line": source_line,
                "title": title,
            }),
        )),
        _ => None,
    };
    json!({
        "id": id,
        "title": title,
        "source_file": source_file,
        "source_line": source_line,
        "correlation_id": correlation_id,
    })
}

fn trace_stage_values(
    request: &Value,
    parser_stage: Option<Value>,
    adapter_stage: Option<Value>,
    evidence_stage: Option<Value>,
    parser_digest: Option<Value>,
    adapter_digest: Option<Value>,
    evidence_digest: Option<Value>,
) -> Vec<Value> {
    let request_correlation_id = request
        .get("correlation_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let request_digest = if request_correlation_id.is_some() {
        trace_digest(
            "canonical JSON request trace identity",
            &json!({
                "id": request.get("id").cloned().unwrap_or(Value::Null),
                "title": request.get("title").cloned().unwrap_or(Value::Null),
                "source_file": request.get("source_file").cloned().unwrap_or(Value::Null),
                "source_line": request.get("source_line").cloned().unwrap_or(Value::Null),
                "parser_digest": parser_digest.as_ref().and_then(trace_digest_value),
                "adapter_digest": adapter_digest.as_ref().and_then(trace_digest_value),
                "evidence_digest": evidence_digest.as_ref().and_then(trace_digest_value),
            }),
        )
    } else {
        Value::Null
    };
    let request_stage = json!({
        "stage": "request",
        "status": trace_stage_status(&request_correlation_id),
        "correlation_id": request_correlation_id,
        "artifact_ref": Value::Null,
        "digest": request_digest,
    });
    vec![
        request_stage,
        parser_stage.unwrap_or_else(trace_missing_parser_stage),
        adapter_stage.unwrap_or_else(trace_missing_adapter_stage),
        evidence_stage.unwrap_or_else(trace_missing_evidence_stage),
    ]
}

fn trace_missing_parser_stage() -> Value {
    json!({
        "stage": "parser",
        "status": "missing",
        "correlation_id": Value::Null,
        "artifact_ref": Value::Null,
        "source": {"file": Value::Null},
        "digest": Value::Null,
    })
}

fn trace_missing_adapter_stage() -> Value {
    json!({
        "stage": "adapter",
        "status": "missing",
        "correlation_id": Value::Null,
        "artifact_ref": Value::Null,
        "adapter": {"id": Value::Null, "version": Value::Null},
        "digest": Value::Null,
    })
}

fn trace_missing_evidence_stage() -> Value {
    json!({
        "stage": "evidence",
        "status": "missing",
        "correlation_id": Value::Null,
        "artifact_ref": Value::Null,
        "adapter": {"id": Value::Null, "version": Value::Null},
        "digest": Value::Null,
    })
}

fn traceability_compact(
    request: Value,
    stages: Vec<Value>,
    replay_references: Vec<Value>,
    mut diagnostics: Vec<Value>,
) -> Value {
    diagnostics.sort_by_key(trace_diagnostic_sort_key);
    let missing_conditions = trace_missing_conditions(&diagnostics);
    let valid = diagnostics
        .iter()
        .all(|diagnostic| diagnostic.get("severity").and_then(Value::as_str) != Some("error"));
    let graph_basis = json!({
        "request": request,
        "stages": stages,
        "replay_references": replay_references,
        "missing_conditions": missing_conditions,
    });
    let graph_correlation_id = trace_correlation_id("graph", &graph_basis);
    json!({
        "schema_version": TRACEABILITY_OUTPUT_SCHEMA_VERSION,
        "app_version": ADAPTER_EVIDENCE_TRACEABILITY_VERSION,
        "valid": valid,
        "request": graph_basis["request"].clone(),
        "graph": {
            "correlation_id": graph_correlation_id,
            "node_order": TRACEABILITY_STAGE_ORDER,
            "edges": [
                {"from": "request", "to": "parser", "relationship": "parsed_by"},
                {"from": "parser", "to": "adapter", "relationship": "materialized_by"},
                {"from": "adapter", "to": "evidence", "relationship": "evidenced_by"},
            ],
        },
        "stages": graph_basis["stages"].clone(),
        "replay_references": graph_basis["replay_references"].clone(),
        "missing_condition_count": missing_conditions.len(),
        "missing_conditions": missing_conditions,
        "diagnostic_count": diagnostics.len(),
        "deterministic_diagnostics": {
            "ordering": ["code", "stage", "field", "severity", "message"],
            "items": diagnostics,
        },
        "serialization": {
            "format": "json",
            "object_key_order": "lexicographic before hashing",
            "array_order": "contract-defined stage order; replay references sorted by kind/path/digest; diagnostics sorted by code/stage/field/severity/message",
            "digest_algorithm": "sha256 over canonical JSON",
            "missing_data_policy": "missing required trace inputs emit deterministic diagnostics and null stage correlation IDs instead of reading live tracker state",
        },
    })
}

fn trace_required_object(
    value: Option<&Value>,
    stage: &str,
    pointer: &str,
    diagnostics: &mut Vec<Value>,
) -> Option<Map<String, Value>> {
    match value.and_then(Value::as_object) {
        Some(object) => Some(object.clone()),
        None if value.is_some() => {
            trace_diag(
                diagnostics,
                format!("{stage}_invalid"),
                stage,
                Some(pointer),
                format!("{pointer} must be a JSON object"),
                "error",
            );
            None
        }
        None => {
            trace_diag(
                diagnostics,
                format!("{stage}_missing"),
                stage,
                Some(pointer),
                format!("{pointer} is required"),
                "error",
            );
            None
        }
    }
}

fn trace_required_json_object_at(
    object: &Map<String, Value>,
    field: &str,
    stage: &str,
    pointer: &str,
    diagnostics: &mut Vec<Value>,
) -> Option<Value> {
    match object.get(field) {
        Some(value) if value.is_object() => Some(value.clone()),
        Some(_) => {
            trace_diag(
                diagnostics,
                format!("{stage}_{field}_invalid"),
                stage,
                Some(pointer),
                format!("{pointer} must be a JSON object"),
                "error",
            );
            None
        }
        None => {
            trace_diag(
                diagnostics,
                format!("{stage}_{field}_missing"),
                stage,
                Some(pointer),
                format!("{pointer} is required"),
                "error",
            );
            None
        }
    }
}

fn trace_required_string_at(
    object: &Map<String, Value>,
    field: &str,
    stage: &str,
    pointer: &str,
    diagnostics: &mut Vec<Value>,
) -> Option<String> {
    match object.get(field) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.to_string()),
        None => {
            trace_diag(
                diagnostics,
                format!("{stage}_{field}_missing"),
                stage,
                Some(pointer),
                format!("{pointer} must be a non-empty string"),
                "error",
            );
            None
        }
        Some(_) => {
            trace_diag(
                diagnostics,
                format!("{stage}_{field}_invalid"),
                stage,
                Some(pointer),
                format!("{pointer} must be a non-empty string"),
                "error",
            );
            None
        }
    }
}

fn trace_optional_string_at(
    object: &Map<String, Value>,
    field: &str,
    stage: &str,
    pointer: &str,
    diagnostics: &mut Vec<Value>,
) -> Option<String> {
    match object.get(field) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        Some(_) => {
            trace_diag(
                diagnostics,
                format!("{stage}_{field}_invalid"),
                stage,
                Some(pointer),
                format!("{pointer} must be a non-empty string when provided"),
                "error",
            );
            None
        }
        None => None,
    }
}

fn trace_optional_u64_at(
    object: &Map<String, Value>,
    field: &str,
    stage: &str,
    pointer: &str,
    diagnostics: &mut Vec<Value>,
) -> Option<u64> {
    match object.get(field) {
        Some(Value::Number(value)) => match value.as_u64() {
            Some(value) if value > 0 => Some(value),
            _ => {
                trace_diag(
                    diagnostics,
                    format!("{stage}_{field}_invalid"),
                    stage,
                    Some(pointer),
                    format!(
                        "{pointer} must be an unsigned integer greater than zero when provided"
                    ),
                    "error",
                );
                None
            }
        },
        Some(_) => {
            trace_diag(
                diagnostics,
                format!("{stage}_{field}_invalid"),
                stage,
                Some(pointer),
                format!("{pointer} must be an unsigned integer greater than zero when provided"),
                "error",
            );
            None
        }
        None => None,
    }
}

fn trace_replay_references(value: Option<&Value>, diagnostics: &mut Vec<Value>) -> Vec<Value> {
    let Some(replay) = trace_required_object(value, "replay", "/replay", diagnostics) else {
        return Vec::new();
    };
    let Some(artifacts) = replay.get("artifacts") else {
        trace_diag(
            diagnostics,
            "replay_artifacts_missing",
            "replay",
            Some("/replay/artifacts"),
            "/replay/artifacts is required",
            "error",
        );
        return Vec::new();
    };
    let Some(artifacts) = artifacts.as_array() else {
        trace_diag(
            diagnostics,
            "replay_artifacts_invalid",
            "replay",
            Some("/replay/artifacts"),
            "/replay/artifacts must be an array",
            "error",
        );
        return Vec::new();
    };

    let mut references = Vec::new();
    for (index, artifact) in artifacts.iter().enumerate() {
        let pointer = format!("/replay/artifacts/{index}");
        let Some(artifact_object) = artifact.as_object() else {
            trace_diag(
                diagnostics,
                "replay_artifact_invalid",
                "replay",
                Some(&pointer),
                format!("{pointer} must be a JSON object"),
                "error",
            );
            continue;
        };
        let kind = trace_required_string_at(
            artifact_object,
            "kind",
            "replay",
            &format!("{pointer}/kind"),
            diagnostics,
        );
        let path = trace_required_string_at(
            artifact_object,
            "path",
            "replay",
            &format!("{pointer}/path"),
            diagnostics,
        );
        let digest = match artifact_object.get("digest") {
            Some(value) => trace_digest_string(value, &format!("{pointer}/digest"), diagnostics),
            None => Value::Null,
        };
        if let (Some(kind), Some(path)) = (kind, path) {
            references.push(json!({
                "kind": kind,
                "path": path,
                "digest": digest,
            }));
        }
    }
    references.sort_by_key(trace_replay_sort_key);
    references
}

fn trace_digest(source: &str, value: &Value) -> Value {
    json!({
        "algorithm": "sha256",
        "source": source,
        "value": sha256_json(value),
    })
}

fn trace_digest_string(value: &Value, pointer: &str, diagnostics: &mut Vec<Value>) -> Value {
    let Some(raw) = value.as_str() else {
        trace_diag(
            diagnostics,
            "replay_digest_invalid",
            "replay",
            Some(pointer),
            format!("{pointer} must be sha256:<64 lowercase hex> when provided"),
            "error",
        );
        return Value::Null;
    };
    let digest = raw.strip_prefix("sha256:").unwrap_or(raw);
    if digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        json!({"algorithm": "sha256", "value": digest.to_ascii_lowercase()})
    } else {
        trace_diag(
            diagnostics,
            "replay_digest_invalid",
            "replay",
            Some(pointer),
            format!("{pointer} must be sha256:<64 lowercase hex> when provided"),
            "error",
        );
        Value::Null
    }
}

fn trace_digest_value(value: &Value) -> Option<String> {
    value
        .get("value")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn trace_stage_correlation_id(
    stage: &str,
    request_id: &Option<String>,
    artifact: &Option<String>,
    digest: Option<&Value>,
    extra: Value,
) -> Option<String> {
    match (request_id, artifact, digest.and_then(trace_digest_value)) {
        (Some(request_id), Some(artifact), Some(digest)) => Some(trace_correlation_id(
            stage,
            &json!({
                "request_id": request_id,
                "artifact": artifact,
                "digest": digest,
                "extra": extra,
            }),
        )),
        _ => None,
    }
}

fn trace_correlation_id(stage: &str, value: &Value) -> String {
    format!("trace:{stage}:{}", sha256_json(value))
}

fn trace_stage_status(correlation_id: &Option<String>) -> &'static str {
    if correlation_id.is_some() {
        "present"
    } else {
        "missing"
    }
}

fn trace_payload_stage_status(artifact: &Option<String>, digest: Option<&Value>) -> &'static str {
    if artifact.is_some()
        && digest
            .and_then(|value| value.get("value"))
            .and_then(Value::as_str)
            .is_some()
    {
        "present"
    } else {
        "missing"
    }
}

fn trace_diag(
    diagnostics: &mut Vec<Value>,
    code: impl Into<String>,
    stage: &str,
    field: Option<&str>,
    message: impl Into<String>,
    severity: &str,
) {
    diagnostics.push(json!({
        "code": code.into(),
        "stage": stage,
        "field": field,
        "severity": severity,
        "message": message.into(),
    }));
}

fn trace_missing_conditions(diagnostics: &[Value]) -> Vec<Value> {
    diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .get("code")
                .and_then(Value::as_str)
                .is_some_and(|code| code.ends_with("_missing"))
        })
        .map(|diagnostic| {
            json!({
                "code": diagnostic.get("code").cloned().unwrap_or(Value::Null),
                "stage": diagnostic.get("stage").cloned().unwrap_or(Value::Null),
                "field": diagnostic.get("field").cloned().unwrap_or(Value::Null),
                "message": diagnostic.get("message").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn trace_diagnostic_sort_key(value: &Value) -> (String, String, String, String, String) {
    (
        value
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        value
            .get("stage")
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

fn trace_replay_sort_key(value: &Value) -> (String, String, String) {
    (
        value
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        value
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        value
            .get("digest")
            .and_then(|digest| digest.get("value"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
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

#[derive(Debug, Clone)]
struct ReadinessReportReason {
    code: String,
    message: String,
    request_id: Option<String>,
    artifact_id: Option<String>,
    field: Option<String>,
}

impl ReadinessReportReason {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            request_id: None,
            artifact_id: None,
            field: None,
        }
    }

    fn request(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    fn artifact(mut self, artifact_id: impl Into<String>) -> Self {
        self.artifact_id = Some(artifact_id.into());
        self
    }

    fn field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    fn sort_key(&self) -> (String, String, String, String, String) {
        (
            self.code.clone(),
            self.request_id.clone().unwrap_or_default(),
            self.artifact_id.clone().unwrap_or_default(),
            self.field.clone().unwrap_or_default(),
            self.message.clone(),
        )
    }

    fn to_value(&self) -> Value {
        json!({
            "code": self.code,
            "message": self.message,
            "request_id": self.request_id,
            "artifact_id": self.artifact_id,
            "field": self.field,
        })
    }
}

#[derive(Debug, Clone)]
struct ReadinessReportEvidence {
    artifact_id: String,
    adapter_id: Option<String>,
    captured_at: Option<String>,
    fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
struct ReadinessReportEnvelope {
    artifact_id: String,
    adapter_id: Option<String>,
    phase: Option<String>,
    captured_at: Option<String>,
    result_class: Option<String>,
}

#[derive(Debug, Default)]
struct ReadinessReportRequest {
    evidence: Vec<ReadinessReportEvidence>,
    envelopes: Vec<ReadinessReportEnvelope>,
}

#[derive(Debug)]
struct RequiredReportEnvelope {
    adapter_id: String,
    phase: String,
}

/// Build a deterministic post-dogfood public 0.x readiness report.
///
/// The report consumes retained request evidence digests plus normalized adapter
/// evidence envelopes. It does not call Multica or inspect live state; callers
/// provide the freshness cutoff so local and non-Multica runners can replay the
/// same packet and compare the compact output by value.
pub fn evaluate_public_0x_readiness_report_input(value: &Value) -> Value {
    let mut coverage_reasons = Vec::new();
    let mut freshness_reasons = Vec::new();
    let mut consistency_reasons = Vec::new();
    let mut requests: BTreeMap<String, ReadinessReportRequest> = BTreeMap::new();

    let Some(object) = value.as_object() else {
        coverage_reasons.push(ReadinessReportReason::new(
            "input_invalid",
            "input must be a JSON object",
        ));
        return readiness_report_compact(
            None,
            None,
            &requests,
            &coverage_reasons,
            &freshness_reasons,
            &consistency_reasons,
        );
    };

    if object.get("schema_version").and_then(Value::as_str)
        != Some(READINESS_REPORT_INPUT_SCHEMA_VERSION)
    {
        coverage_reasons.push(ReadinessReportReason::new(
            "unsupported_schema_version",
            format!("schema_version must be {READINESS_REPORT_INPUT_SCHEMA_VERSION}"),
        ));
    }

    let generated_at = report_string_field(object, "generated_at");
    let freshness = object.get("freshness").and_then(Value::as_object);
    let fresh_after = freshness.and_then(|freshness| report_string_field(freshness, "fresh_after"));
    if fresh_after.is_none() {
        freshness_reasons.push(ReadinessReportReason::new(
            "fresh_after_missing",
            "freshness.fresh_after must be provided",
        ));
    } else if fresh_after
        .as_deref()
        .and_then(parse_readiness_report_timestamp)
        .is_none()
    {
        freshness_reasons.push(ReadinessReportReason::new(
            "fresh_after_invalid",
            "freshness.fresh_after must be an RFC 3339 timestamp",
        ));
    }

    let coverage = object.get("coverage").and_then(Value::as_object);
    if coverage.is_none() {
        coverage_reasons.push(ReadinessReportReason::new(
            "coverage_missing",
            "coverage requirements must be provided",
        ));
    }
    let minimum_request_count = readiness_report_unsigned_count(
        coverage,
        "minimum_request_count",
        1,
        &mut coverage_reasons,
    );
    let minimum_evidence_count = readiness_report_unsigned_count(
        coverage,
        "minimum_evidence_count",
        READINESS_REPORT_DEFAULT_MINIMUM_EVIDENCE_COUNT,
        &mut coverage_reasons,
    );
    let required_request_kinds =
        readiness_report_string_list(coverage, "required_request_kinds", &["app"]);
    let required_fields = readiness_report_string_list(
        coverage,
        "required_evidence_fields",
        READINESS_REPORT_DEFAULT_FIELDS,
    );
    let required_envelopes = readiness_report_required_envelopes(coverage, &mut coverage_reasons);

    match object.get("request_evidence").and_then(Value::as_array) {
        Some(items) => {
            for (index, item) in items.iter().enumerate() {
                parse_readiness_report_evidence(
                    item,
                    index,
                    fresh_after.as_deref(),
                    &mut requests,
                    &mut coverage_reasons,
                    &mut freshness_reasons,
                    &mut consistency_reasons,
                );
            }
        }
        None => coverage_reasons.push(ReadinessReportReason::new(
            "request_evidence_missing",
            "request_evidence must be an array of retained source evidence artifacts",
        )),
    }

    match object.get("adapter_envelopes").and_then(Value::as_array) {
        Some(items) => {
            for (index, item) in items.iter().enumerate() {
                parse_readiness_report_envelope(
                    item,
                    index,
                    fresh_after.as_deref(),
                    &mut requests,
                    &mut coverage_reasons,
                    &mut freshness_reasons,
                    &mut consistency_reasons,
                );
            }
        }
        None => coverage_reasons.push(ReadinessReportReason::new(
            "adapter_envelopes_missing",
            "adapter_envelopes must be an array of retained adapter evidence envelopes",
        )),
    }

    evaluate_readiness_report_coverage(
        &requests,
        minimum_request_count,
        &required_request_kinds,
        &required_fields,
        &required_envelopes,
        &mut coverage_reasons,
    );
    evaluate_readiness_report_consistency(
        &requests,
        minimum_evidence_count,
        &required_fields,
        &mut consistency_reasons,
    );

    readiness_report_compact(
        generated_at,
        fresh_after,
        &requests,
        &coverage_reasons,
        &freshness_reasons,
        &consistency_reasons,
    )
}

fn parse_readiness_report_evidence(
    item: &Value,
    index: usize,
    fresh_after: Option<&str>,
    requests: &mut BTreeMap<String, ReadinessReportRequest>,
    coverage_reasons: &mut Vec<ReadinessReportReason>,
    freshness_reasons: &mut Vec<ReadinessReportReason>,
    consistency_reasons: &mut Vec<ReadinessReportReason>,
) {
    let Some(object) = item.as_object() else {
        coverage_reasons.push(ReadinessReportReason::new(
            "request_evidence_invalid",
            "request_evidence items must be JSON objects",
        ));
        return;
    };
    let artifact_id = report_artifact_id(object, "request_evidence", index);
    let Some(request_id) = report_string_field(object, "request_id") else {
        coverage_reasons.push(
            ReadinessReportReason::new(
                "request_id_missing",
                "request evidence artifact must provide request_id",
            )
            .artifact(artifact_id),
        );
        return;
    };
    let adapter_id = report_string_field(object, "adapter_id");
    if adapter_id.is_none() {
        consistency_reasons.push(
            ReadinessReportReason::new(
                "request_evidence_adapter_missing",
                "request evidence artifact must provide adapter_id",
            )
            .request(request_id.clone())
            .artifact(artifact_id.clone())
            .field("adapter_id"),
        );
    }
    let captured_at = report_string_field(object, "captured_at");
    check_report_freshness(
        "request_evidence",
        &request_id,
        &artifact_id,
        captured_at.as_deref(),
        fresh_after,
        freshness_reasons,
    );

    let mut fields = BTreeMap::new();
    match object.get("digest") {
        Some(digest) if digest.is_object() => {
            if digest.get("schema_version").and_then(Value::as_str)
                != Some(READINESS_REPORT_EVIDENCE_DIGEST_SCHEMA_VERSION)
            {
                consistency_reasons.push(
                    ReadinessReportReason::new(
                        "evidence_digest_schema_mismatch",
                        format!(
                            "request evidence digest must use {READINESS_REPORT_EVIDENCE_DIGEST_SCHEMA_VERSION}"
                        ),
                    )
                    .request(request_id.clone())
                    .artifact(artifact_id.clone())
                    .field("digest.schema_version"),
                );
            }
            if digest.get("request_schema_version").and_then(Value::as_str)
                != Some(READINESS_REPORT_REQUEST_SCHEMA_VERSION)
            {
                consistency_reasons.push(
                    ReadinessReportReason::new(
                        "request_schema_mismatch",
                        format!(
                            "request evidence digest must cover {READINESS_REPORT_REQUEST_SCHEMA_VERSION}"
                        ),
                    )
                    .request(request_id.clone())
                    .artifact(artifact_id.clone())
                    .field("digest.request_schema_version"),
                );
            }
            fields = readiness_report_digest_fields(digest);
            if fields.is_empty() {
                consistency_reasons.push(
                    ReadinessReportReason::new(
                        "evidence_digest_empty",
                        "request evidence digest must expose deterministic section fields",
                    )
                    .request(request_id.clone())
                    .artifact(artifact_id.clone())
                    .field("digest.sections"),
                );
            }
        }
        _ => consistency_reasons.push(
            ReadinessReportReason::new(
                "evidence_digest_missing",
                "request evidence artifact must include a digest object",
            )
            .request(request_id.clone())
            .artifact(artifact_id.clone())
            .field("digest"),
        ),
    }

    requests
        .entry(request_id)
        .or_default()
        .evidence
        .push(ReadinessReportEvidence {
            artifact_id,
            adapter_id,
            captured_at,
            fields,
        });
}

fn parse_readiness_report_envelope(
    item: &Value,
    index: usize,
    fresh_after: Option<&str>,
    requests: &mut BTreeMap<String, ReadinessReportRequest>,
    coverage_reasons: &mut Vec<ReadinessReportReason>,
    freshness_reasons: &mut Vec<ReadinessReportReason>,
    consistency_reasons: &mut Vec<ReadinessReportReason>,
) {
    let Some(object) = item.as_object() else {
        coverage_reasons.push(ReadinessReportReason::new(
            "adapter_envelope_invalid",
            "adapter_envelopes items must be JSON objects",
        ));
        return;
    };
    let artifact_id = report_artifact_id(object, "adapter_envelopes", index);
    let Some(request_id) = report_string_field(object, "request_id") else {
        coverage_reasons.push(
            ReadinessReportReason::new(
                "request_id_missing",
                "adapter envelope artifact must provide request_id",
            )
            .artifact(artifact_id),
        );
        return;
    };
    let captured_at = report_string_field(object, "captured_at");
    check_report_freshness(
        "adapter_envelope",
        &request_id,
        &artifact_id,
        captured_at.as_deref(),
        fresh_after,
        freshness_reasons,
    );

    let envelope = object.get("envelope").unwrap_or(&Value::Null);
    let adapter_id = report_string_at(envelope, "/adapter/id");
    let phase = report_string_at(envelope, "/phase");
    let result_class = report_string_at(envelope, "/result_class");
    if !envelope.is_object() {
        consistency_reasons.push(
            ReadinessReportReason::new(
                "adapter_envelope_missing",
                "adapter envelope artifact must include an envelope object",
            )
            .request(request_id.clone())
            .artifact(artifact_id.clone())
            .field("envelope"),
        );
    } else {
        if envelope.get("schema_version").and_then(Value::as_str)
            != Some("adapter-evidence-envelope-compact.v0")
        {
            consistency_reasons.push(
                ReadinessReportReason::new(
                    "adapter_envelope_schema_mismatch",
                    "adapter envelope must use adapter-evidence-envelope-compact.v0",
                )
                .request(request_id.clone())
                .artifact(artifact_id.clone())
                .field("envelope.schema_version"),
            );
        }
        match report_string_at(envelope, "/request_id") {
            Some(envelope_request_id) if envelope_request_id == request_id => {}
            Some(_) => consistency_reasons.push(
                ReadinessReportReason::new(
                    "adapter_envelope_request_id_mismatch",
                    "adapter envelope request_id must match its artifact wrapper",
                )
                .request(request_id.clone())
                .artifact(artifact_id.clone())
                .field("envelope.request_id"),
            ),
            None => consistency_reasons.push(
                ReadinessReportReason::new(
                    "adapter_envelope_request_id_missing",
                    "adapter envelope must provide request_id",
                )
                .request(request_id.clone())
                .artifact(artifact_id.clone())
                .field("envelope.request_id"),
            ),
        }
        if adapter_id.is_none() {
            consistency_reasons.push(
                ReadinessReportReason::new(
                    "adapter_envelope_adapter_missing",
                    "adapter envelope must provide adapter.id",
                )
                .request(request_id.clone())
                .artifact(artifact_id.clone())
                .field("envelope.adapter.id"),
            );
        }
        if phase.is_none() {
            consistency_reasons.push(
                ReadinessReportReason::new(
                    "adapter_envelope_phase_missing",
                    "adapter envelope must provide phase",
                )
                .request(request_id.clone())
                .artifact(artifact_id.clone())
                .field("envelope.phase"),
            );
        }
        if envelope.get("valid").and_then(Value::as_bool) != Some(true) {
            consistency_reasons.push(
                ReadinessReportReason::new(
                    "adapter_envelope_invalid_result",
                    "adapter envelope valid must be true",
                )
                .request(request_id.clone())
                .artifact(artifact_id.clone())
                .field("envelope.valid"),
            );
        }
        if result_class.as_deref() != Some("success") {
            consistency_reasons.push(
                ReadinessReportReason::new(
                    "adapter_envelope_result_not_success",
                    "adapter envelope result_class must be success",
                )
                .request(request_id.clone())
                .artifact(artifact_id.clone())
                .field("envelope.result_class"),
            );
        }
    }

    requests
        .entry(request_id)
        .or_default()
        .envelopes
        .push(ReadinessReportEnvelope {
            artifact_id,
            adapter_id,
            phase,
            captured_at,
            result_class,
        });
}

fn evaluate_readiness_report_coverage(
    requests: &BTreeMap<String, ReadinessReportRequest>,
    minimum_request_count: u64,
    required_request_kinds: &[String],
    required_fields: &[String],
    required_envelopes: &[RequiredReportEnvelope],
    coverage_reasons: &mut Vec<ReadinessReportReason>,
) {
    if (requests.len() as u64) < minimum_request_count {
        coverage_reasons.push(ReadinessReportReason::new(
            "minimum_request_count_not_met",
            format!(
                "observed {} request(s), expected at least {minimum_request_count}",
                requests.len()
            ),
        ));
    }

    let mut observed_kinds = BTreeSet::new();
    for (request_id, request) in requests {
        if request.evidence.is_empty() {
            coverage_reasons.push(
                ReadinessReportReason::new(
                    "request_evidence_missing_for_request",
                    "request must have at least one source evidence artifact",
                )
                .request(request_id.clone()),
            );
        }
        if let Some(kind) = first_report_field(request, "request_kind").and_then(Value::as_str) {
            observed_kinds.insert(kind.to_string());
        }
        for field in required_fields {
            if first_report_field(request, field).is_none_or(report_value_missing) {
                coverage_reasons.push(
                    ReadinessReportReason::new(
                        "required_evidence_field_missing",
                        format!("request evidence is missing required field {field}"),
                    )
                    .request(request_id.clone())
                    .field(field.clone()),
                );
            }
        }
        for required in required_envelopes {
            let has_required = request.envelopes.iter().any(|envelope| {
                envelope.adapter_id.as_deref() == Some(required.adapter_id.as_str())
                    && envelope.phase.as_deref() == Some(required.phase.as_str())
            });
            if !has_required {
                coverage_reasons.push(
                    ReadinessReportReason::new(
                        "required_envelope_missing",
                        format!(
                            "required adapter envelope missing for adapter={} phase={}",
                            required.adapter_id, required.phase
                        ),
                    )
                    .request(request_id.clone())
                    .field(format!("{}:{}", required.adapter_id, required.phase)),
                );
            }
        }
    }

    for required_kind in required_request_kinds {
        if !observed_kinds.contains(required_kind) {
            coverage_reasons.push(
                ReadinessReportReason::new(
                    "required_request_kind_missing",
                    format!("no request evidence covered required request_kind {required_kind}"),
                )
                .field(format!("request_kind:{required_kind}")),
            );
        }
    }
}

fn evaluate_readiness_report_consistency(
    requests: &BTreeMap<String, ReadinessReportRequest>,
    minimum_evidence_count: u64,
    required_fields: &[String],
    consistency_reasons: &mut Vec<ReadinessReportReason>,
) {
    for (request_id, request) in requests {
        if (request.evidence.len() as u64) < minimum_evidence_count {
            consistency_reasons.push(
                ReadinessReportReason::new(
                    "evidence_comparison_insufficient",
                    format!(
                        "at least {minimum_evidence_count} request evidence artifact(s) are required for adapter comparison"
                    ),
                )
                .request(request_id.clone()),
            );
            continue;
        }
        for field in required_fields {
            let mut expected: Option<(&str, &Value)> = None;
            for artifact in &request.evidence {
                let value = artifact.fields.get(field).unwrap_or(&Value::Null);
                if report_value_missing(value) {
                    consistency_reasons.push(
                        ReadinessReportReason::new(
                            "evidence_field_missing",
                            format!(
                                "request evidence artifact is missing comparable field {field}"
                            ),
                        )
                        .request(request_id.clone())
                        .artifact(artifact.artifact_id.clone())
                        .field(field.clone()),
                    );
                    continue;
                }
                if let Some((expected_artifact, expected_value)) = expected {
                    if value != expected_value {
                        consistency_reasons.push(
                            ReadinessReportReason::new(
                                "evidence_field_mismatch",
                                format!(
                                    "request evidence field {field} differs from artifact {expected_artifact}"
                                ),
                            )
                            .request(request_id.clone())
                            .artifact(artifact.artifact_id.clone())
                            .field(field.clone()),
                        );
                    }
                } else {
                    expected = Some((artifact.artifact_id.as_str(), value));
                }
            }
        }
    }
}

fn readiness_report_compact(
    generated_at: Option<String>,
    fresh_after: Option<String>,
    requests: &BTreeMap<String, ReadinessReportRequest>,
    coverage_reasons: &[ReadinessReportReason],
    freshness_reasons: &[ReadinessReportReason],
    consistency_reasons: &[ReadinessReportReason],
) -> Value {
    let coverage_status = check_status(coverage_reasons);
    let freshness_status = check_status(freshness_reasons);
    let consistency_status = check_status(consistency_reasons);
    let evidence_artifact_count = requests
        .values()
        .map(|request| request.evidence.len())
        .sum::<usize>();
    let adapter_envelope_count = requests
        .values()
        .map(|request| request.envelopes.len())
        .sum::<usize>();
    let blocking_reason_count =
        coverage_reasons.len() + freshness_reasons.len() + consistency_reasons.len();
    let valid = blocking_reason_count == 0;

    json!({
        "schema_version": READINESS_REPORT_OUTPUT_SCHEMA_VERSION,
        "app_version": PUBLIC_0X_READINESS_REPORT_VERSION,
        "readiness_target": "public-0.x",
        "valid": valid,
        "generated_at": generated_at,
        "fresh_after": fresh_after,
        "summary": {
            "request_count": requests.len(),
            "evidence_artifact_count": evidence_artifact_count,
            "adapter_envelope_count": adapter_envelope_count,
            "coverage": coverage_status,
            "freshness": freshness_status,
            "adapter_consistency": consistency_status,
            "blocking_reason_count": blocking_reason_count,
        },
        "checks": [
            readiness_report_check(
                "coverage",
                coverage_reasons,
                format!(
                    "coverage satisfied for {} request(s): {} evidence artifact(s) and {} adapter envelope(s)",
                    requests.len(), evidence_artifact_count, adapter_envelope_count
                ),
            ),
            readiness_report_check(
                "freshness",
                freshness_reasons,
                format!(
                    "freshness satisfied for {} artifact(s) at or after {}",
                    evidence_artifact_count + adapter_envelope_count,
                    fresh_after.as_deref().unwrap_or("<missing fresh_after>")
                ),
            ),
            readiness_report_check(
                "adapter_consistency",
                consistency_reasons,
                format!("adapter consistency satisfied for {} request(s)", requests.len()),
            ),
        ],
        "requests": readiness_report_requests(requests),
        "serialization": {
            "format": "json",
            "object_key_order": "lexicographic",
            "array_order": "checks use contract order coverage/freshness/adapter_consistency; requests and artifact summaries sort by stable identifiers; reasons sort by code/request/artifact/field/message",
            "null_policy": "missing optional scalar fields serialize as null; failures remain deterministic blocking reasons",
            "reason_count_policy": "passing checks emit one synthetic *_satisfied reason, so a pass has reason_count=1 while contributing zero reasons to summary.blocking_reason_count"
        }
    })
}

fn readiness_report_check(
    key: &str,
    reasons: &[ReadinessReportReason],
    pass_message: String,
) -> Value {
    let reason_values = if reasons.is_empty() {
        vec![ReadinessReportReason::new(format!("{key}_satisfied"), pass_message).to_value()]
    } else {
        let mut ordered = reasons.to_vec();
        ordered.sort_by_key(ReadinessReportReason::sort_key);
        ordered
            .iter()
            .map(ReadinessReportReason::to_value)
            .collect::<Vec<_>>()
    };
    json!({
        "key": key,
        "status": check_status(reasons),
        "reason_count": reason_values.len(),
        "reasons": reason_values,
    })
}

fn readiness_report_requests(requests: &BTreeMap<String, ReadinessReportRequest>) -> Vec<Value> {
    requests
        .iter()
        .map(|(request_id, request)| {
            let mut evidence = request.evidence.clone();
            evidence.sort_by_key(|artifact| artifact.artifact_id.clone());
            let mut envelopes = request.envelopes.clone();
            envelopes.sort_by_key(|artifact| artifact.artifact_id.clone());
            json!({
                "request_id": request_id,
                "title": first_report_field(request, "title").cloned().unwrap_or(Value::Null),
                "request_kind": first_report_field(request, "request_kind").cloned().unwrap_or(Value::Null),
                "source_references": {
                    "source_prd": first_report_field(request, "source_prd").cloned().unwrap_or(Value::Null),
                    "source_design": first_report_field(request, "source_design").cloned().unwrap_or(Value::Null),
                    "source_roadmap": first_report_field(request, "source_roadmap").cloned().unwrap_or(Value::Null),
                },
                "evidence_artifacts": evidence.iter().map(|artifact| json!({
                    "artifact_id": artifact.artifact_id,
                    "adapter_id": artifact.adapter_id,
                    "captured_at": artifact.captured_at,
                })).collect::<Vec<_>>(),
                "adapter_envelopes": envelopes.iter().map(|artifact| json!({
                    "artifact_id": artifact.artifact_id,
                    "adapter_id": artifact.adapter_id,
                    "phase": artifact.phase,
                    "captured_at": artifact.captured_at,
                    "result_class": artifact.result_class,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn readiness_report_required_envelopes(
    coverage: Option<&Map<String, Value>>,
    coverage_reasons: &mut Vec<ReadinessReportReason>,
) -> Vec<RequiredReportEnvelope> {
    let Some(items) = coverage
        .and_then(|coverage| coverage.get("required_envelopes"))
        .and_then(Value::as_array)
    else {
        coverage_reasons.push(ReadinessReportReason::new(
            "required_envelopes_missing",
            "coverage.required_envelopes must list adapter/phase evidence requirements",
        ));
        return Vec::new();
    };

    let mut envelopes = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Some(object) = item.as_object() else {
            coverage_reasons.push(ReadinessReportReason::new(
                "required_envelope_invalid",
                format!("coverage.required_envelopes[{index}] must be an object"),
            ));
            continue;
        };
        match (
            report_string_field(object, "adapter_id"),
            report_string_field(object, "phase"),
        ) {
            (Some(adapter_id), Some(phase)) => {
                envelopes.push(RequiredReportEnvelope { adapter_id, phase })
            }
            _ => coverage_reasons.push(ReadinessReportReason::new(
                "required_envelope_invalid",
                format!("coverage.required_envelopes[{index}] must provide adapter_id and phase"),
            )),
        }
    }
    if envelopes.is_empty() {
        coverage_reasons.push(ReadinessReportReason::new(
            "required_envelopes_empty",
            "coverage.required_envelopes must contain at least one adapter/phase requirement",
        ));
    }
    envelopes
}

fn readiness_report_string_list(
    object: Option<&Map<String, Value>>,
    field: &str,
    default: &[&str],
) -> Vec<String> {
    let values = object
        .and_then(|object| object.get(field))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if values.is_empty() {
        default.iter().map(|item| (*item).to_string()).collect()
    } else {
        values
    }
}

fn readiness_report_digest_fields(digest: &Value) -> BTreeMap<String, Value> {
    let mut fields = BTreeMap::new();
    let Some(sections) = digest.get("sections").and_then(Value::as_array) else {
        return fields;
    };
    for section in sections {
        let Some(section_fields) = section.get("fields").and_then(Value::as_array) else {
            continue;
        };
        for field in section_fields {
            if let Some(key) = field.get("key").and_then(Value::as_str) {
                fields.insert(
                    key.to_string(),
                    field.get("value").cloned().unwrap_or(Value::Null),
                );
            }
        }
    }
    fields
}

fn first_report_field<'a>(request: &'a ReadinessReportRequest, field: &str) -> Option<&'a Value> {
    request
        .evidence
        .iter()
        .filter_map(|artifact| artifact.fields.get(field))
        .find(|value| !report_value_missing(value))
}

fn report_value_missing(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.trim().is_empty(),
        _ => false,
    }
}

fn readiness_report_unsigned_count(
    coverage: Option<&Map<String, Value>>,
    field: &str,
    default: u64,
    coverage_reasons: &mut Vec<ReadinessReportReason>,
) -> u64 {
    let Some(value) = coverage.and_then(|coverage| coverage.get(field)) else {
        return default;
    };
    match value.as_u64() {
        Some(count) if count > 0 => count,
        _ => {
            coverage_reasons.push(
                ReadinessReportReason::new(
                    format!("{field}_invalid"),
                    format!("coverage.{field} must be a positive unsigned integer"),
                )
                .field(format!("coverage.{field}")),
            );
            default
        }
    }
}

fn parse_readiness_report_timestamp(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value.trim()).ok()
}

fn check_report_freshness(
    artifact_kind: &str,
    request_id: &str,
    artifact_id: &str,
    captured_at: Option<&str>,
    fresh_after: Option<&str>,
    freshness_reasons: &mut Vec<ReadinessReportReason>,
) {
    let label = if artifact_kind == "request_evidence" {
        "request evidence artifact"
    } else {
        "adapter envelope"
    };
    let Some(captured_at) = captured_at else {
        freshness_reasons.push(
            ReadinessReportReason::new(
                format!("{artifact_kind}_captured_at_missing"),
                format!("{label} must provide captured_at"),
            )
            .request(request_id.to_string())
            .artifact(artifact_id.to_string())
            .field("captured_at"),
        );
        return;
    };
    let Some(captured_at_timestamp) = parse_readiness_report_timestamp(captured_at) else {
        freshness_reasons.push(
            ReadinessReportReason::new(
                format!("{artifact_kind}_captured_at_invalid"),
                format!("{label} captured_at must be an RFC 3339 timestamp"),
            )
            .request(request_id.to_string())
            .artifact(artifact_id.to_string())
            .field("captured_at"),
        );
        return;
    };
    if let Some(fresh_after) = fresh_after {
        let Some(fresh_after_timestamp) = parse_readiness_report_timestamp(fresh_after) else {
            return;
        };
        if captured_at_timestamp < fresh_after_timestamp {
            freshness_reasons.push(
                ReadinessReportReason::new(
                    format!("{artifact_kind}_stale"),
                    format!("{label} captured_at is before fresh_after {fresh_after}"),
                )
                .request(request_id.to_string())
                .artifact(artifact_id.to_string())
                .field("captured_at"),
            );
        }
    }
}

fn report_artifact_id(object: &Map<String, Value>, prefix: &str, index: usize) -> String {
    report_string_field(object, "artifact_id").unwrap_or_else(|| format!("{prefix}[{index}]"))
}

fn report_string_field(object: &Map<String, Value>, field: &str) -> Option<String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn report_string_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn check_status(reasons: &[ReadinessReportReason]) -> &'static str {
    if reasons.is_empty() {
        "pass"
    } else {
        "fail"
    }
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
    fn adapter_parity_report_fixtures_are_deterministic() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "adapter_parity_full_input.json",
                "expected_adapter_parity_full_payload.json",
            ),
            (
                "adapter_parity_canonical_mismatch_input.json",
                "expected_adapter_parity_canonical_mismatch_payload.json",
            ),
            (
                "adapter_parity_extension_mismatch_input.json",
                "expected_adapter_parity_extension_mismatch_payload.json",
            ),
            (
                "adapter_parity_invalid_input.json",
                "expected_adapter_parity_invalid_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            assert_eq!(
                build_adapter_parity_report_input(&input),
                expected,
                "{input_name} should match {expected_name}"
            );
        }
    }

    #[test]
    fn adapter_parity_error_classes_are_reported_separately() {
        let output = build_adapter_parity_report_input(&json!({
            "schema_version": PARITY_REPORT_INPUT_SCHEMA_VERSION,
            "request_id": "REQ-001",
            "canonical_fields": ["title"],
            "left": {
                "adapter_id": "markdown",
                "request_id": "REQ-001",
                "result": {
                    "valid": true,
                    "canonical": {"title": "Add parity report"}
                }
            },
            "right": {
                "adapter_id": "local",
                "request_id": "REQ-001",
                "result": {
                    "valid": false,
                    "canonical": {"title": "Add parity report"},
                    "errors": [{"code": "required_field_missing", "message": "status is required"}]
                }
            }
        }));

        assert_eq!(output["valid"], false);
        assert_eq!(output["canonical_mismatch_count"], 0);
        assert_eq!(output["extension_mismatch_count"], 0);
        assert_eq!(
            output["error_mismatches"][0]["code"],
            "error_class_mismatch"
        );
        assert_eq!(
            output["normalized_errors"][1]["records"][0]["taxonomy_code"],
            "request.field_required"
        );
    }

    #[test]
    fn adapter_parity_success_serializes_byte_identically_for_object_order() {
        let left_order = json!({
            "schema_version": PARITY_REPORT_INPUT_SCHEMA_VERSION,
            "request_id": "REQ-001",
            "canonical_fields": ["title", "status"],
            "left": {"adapter_id": "a", "result": {"valid": true, "canonical": {"title": "App", "status": "ready"}, "extension": {"b": 2, "a": 1}}},
            "right": {"adapter_id": "b", "result": {"valid": true, "canonical": {"title": "App", "status": "ready"}, "extension": {"a": 1, "b": 2}}}
        });
        let right_order = json!({
            "right": {"result": {"extension": {"b": 2, "a": 1}, "canonical": {"status": "ready", "title": "App"}, "valid": true}, "adapter_id": "b"},
            "left": {"result": {"extension": {"a": 1, "b": 2}, "canonical": {"status": "ready", "title": "App"}, "valid": true}, "adapter_id": "a"},
            "canonical_fields": ["title", "status"],
            "request_id": "REQ-001",
            "schema_version": PARITY_REPORT_INPUT_SCHEMA_VERSION
        });

        let left_bytes = serde_json::to_string(&build_adapter_parity_report_input(&left_order))
            .expect("left output serializes");
        let right_bytes = serde_json::to_string(&build_adapter_parity_report_input(&right_order))
            .expect("right output serializes");
        assert_eq!(left_bytes, right_bytes);
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
    fn public_readiness_report_fixtures_are_deterministic() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "public_0x_readiness_report_success_input.json",
                "expected_public_0x_readiness_report_success_payload.json",
            ),
            (
                "public_0x_readiness_report_failure_input.json",
                "expected_public_0x_readiness_report_failure_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            assert_eq!(
                evaluate_public_0x_readiness_report_input(&input),
                expected,
                "{input_name} should match {expected_name}"
            );
        }
    }

    fn readiness_report_reason_codes(output: &Value) -> BTreeSet<String> {
        output["checks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|check| check["reasons"].as_array().unwrap())
            .filter_map(|reason| reason["code"].as_str())
            .map(str::to_string)
            .collect()
    }

    fn minimal_readiness_report_input() -> Value {
        json!({
            "schema_version": READINESS_REPORT_INPUT_SCHEMA_VERSION,
            "generated_at": "2026-08-01T12:00:00Z",
            "freshness": {"fresh_after": "2026-08-01T00:00:00Z"},
            "coverage": {
                "minimum_request_count": 1,
                "required_request_kinds": ["app"],
                "required_evidence_fields": ["title", "request_kind"],
                "required_envelopes": [{"adapter_id": "markdown", "phase": "validation"}]
            },
            "request_evidence": [{
                "artifact_id": "markdown-digest.json",
                "request_id": "DOT-1298",
                "adapter_id": "markdown",
                "captured_at": "2026-08-01T08:00:00Z",
                "digest": {
                    "schema_version": READINESS_REPORT_EVIDENCE_DIGEST_SCHEMA_VERSION,
                    "request_schema_version": READINESS_REPORT_REQUEST_SCHEMA_VERSION,
                    "sections": [{"key": "identity", "fields": [
                        {"key": "title", "value": "App"},
                        {"key": "request_kind", "value": "app"}
                    ]}]
                }
            }, {
                "artifact_id": "local-digest.json",
                "request_id": "DOT-1298",
                "adapter_id": "local",
                "captured_at": "2026-08-01T08:01:00Z",
                "digest": {
                    "schema_version": READINESS_REPORT_EVIDENCE_DIGEST_SCHEMA_VERSION,
                    "request_schema_version": READINESS_REPORT_REQUEST_SCHEMA_VERSION,
                    "sections": [{"key": "identity", "fields": [
                        {"key": "title", "value": "App"},
                        {"key": "request_kind", "value": "app"}
                    ]}]
                }
            }],
            "adapter_envelopes": [{
                "artifact_id": "markdown-validation-envelope.json",
                "request_id": "DOT-1298",
                "captured_at": "2026-08-01T08:02:00Z",
                "envelope": {
                    "schema_version": EVIDENCE_ENVELOPE_OUTPUT_SCHEMA_VERSION,
                    "valid": true,
                    "request_id": "DOT-1298",
                    "phase": "validation",
                    "adapter": {"id": "markdown"},
                    "result_class": "success"
                }
            }]
        })
    }

    #[test]
    fn public_readiness_report_covers_deterministic_reason_codes() {
        let mut cases: Vec<(&str, Value)> = vec![
            ("input_invalid", json!(null)),
            ("unsupported_schema_version", {
                let mut input = minimal_readiness_report_input();
                input["schema_version"] = json!("public-0x-readiness-report-input.v1");
                input
            }),
            ("minimum_request_count_invalid", {
                let mut input = minimal_readiness_report_input();
                input["coverage"]["minimum_request_count"] = json!("many");
                input
            }),
            ("required_envelopes_missing", {
                let mut input = minimal_readiness_report_input();
                input["coverage"]
                    .as_object_mut()
                    .unwrap()
                    .remove("required_envelopes");
                input
            }),
            ("required_envelope_invalid", {
                let mut input = minimal_readiness_report_input();
                input["coverage"]["required_envelopes"] = json!(["not-an-object"]);
                input
            }),
            ("evidence_digest_missing", {
                let mut input = minimal_readiness_report_input();
                input["request_evidence"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("digest");
                input
            }),
            ("evidence_digest_schema_mismatch", {
                let mut input = minimal_readiness_report_input();
                input["request_evidence"][0]["digest"]["schema_version"] = json!("digest.v1");
                input
            }),
            ("evidence_digest_empty", {
                let mut input = minimal_readiness_report_input();
                input["request_evidence"][0]["digest"]["sections"] = json!([]);
                input
            }),
            ("adapter_envelope_request_id_mismatch", {
                let mut input = minimal_readiness_report_input();
                input["adapter_envelopes"][0]["envelope"]["request_id"] = json!("DOT-OTHER");
                input
            }),
            ("evidence_field_mismatch", {
                let mut input = minimal_readiness_report_input();
                input["request_evidence"][1]["digest"]["sections"][0]["fields"][0]["value"] =
                    json!("Different");
                input
            }),
            ("fresh_after_invalid", {
                let mut input = minimal_readiness_report_input();
                input["freshness"]["fresh_after"] = json!("2026-8-1");
                input
            }),
            ("request_evidence_captured_at_invalid", {
                let mut input = minimal_readiness_report_input();
                input["request_evidence"][0]["captured_at"] = json!("2026-8-1");
                input
            }),
        ];

        for (expected_code, input) in cases.drain(..) {
            let output = evaluate_public_0x_readiness_report_input(&input);
            let codes = readiness_report_reason_codes(&output);
            assert!(
                codes.contains(expected_code),
                "expected {expected_code} in {codes:?}"
            );
        }
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
    fn adapter_evidence_traceability_fixtures_are_deterministic() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "traceability_success_input.json",
                "expected_traceability_success_payload.json",
            ),
            (
                "traceability_malformed_input.json",
                "expected_traceability_malformed_payload.json",
            ),
            (
                "traceability_missing_evidence_input.json",
                "expected_traceability_missing_evidence_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            assert_eq!(
                build_adapter_evidence_traceability_input(&input),
                expected,
                "{input_name} should match {expected_name}"
            );
        }
    }

    #[test]
    fn adapter_evidence_traceability_sorts_replay_references() {
        let output = build_adapter_evidence_traceability_input(&json!({
            "schema_version": TRACEABILITY_INPUT_SCHEMA_VERSION,
            "request": {"id": "DOT-1", "source_file": "Requests/App/example.md"},
            "parser": {"artifact": "parser.json", "output": {"valid": true}},
            "adapter": {"id": "adapter-b", "artifact": "adapter.json", "output": {"valid": true}},
            "evidence": {"artifact": "evidence.json", "envelope": {"valid": true}},
            "replay": {"artifacts": [
                {"kind": "transcript", "path": "z.json"},
                {"kind": "sidecar", "path": "c.json", "digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
                {"kind": "sidecar", "path": "b.json"},
                {"kind": "sidecar", "path": "c.json", "digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
                {"kind": "sidecar", "path": "a.json"}
            ]}
        }));

        assert_eq!(output["valid"], true);
        assert_eq!(output["replay_references"][0]["path"], "a.json");
        assert_eq!(output["replay_references"][1]["path"], "b.json");
        assert_eq!(output["replay_references"][2]["path"], "c.json");
        assert_eq!(
            output["replay_references"][2]["digest"]["value"],
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(output["replay_references"][3]["path"], "c.json");
        assert_eq!(
            output["replay_references"][3]["digest"]["value"],
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert_eq!(output["replay_references"][4]["path"], "z.json");
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
