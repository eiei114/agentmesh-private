//! Deterministic canonical request Markdown renderer App.
//!
//! The renderer closes the request seam in the opposite direction of the
//! normalizer: it accepts one validated canonical request summary (frontmatter
//! fields plus canonical section bodies) and renders a canonical
//! `agentmesh-request.v0` Markdown source document, then proves round-trip
//! fidelity by re-entering the existing canonical projection and comparing the
//! canonical digests.
//!
//! The rendered document is produced by the same canonical writer the
//! normalizer App exposes, so a regenerated request source cannot drift from its
//! canonical projection by construction. Non-canonical and adapter-owned
//! summary fields never reach the rendered source; they are reported as bounded
//! diagnostics instead.

use crate::adapter_compact_helpers::canonical_json_bytes;
use crate::request_markdown_normalizer::{
    canonical_document_from_summary, reenter_canonical_markdown, CANONICAL_FRONTMATTER_ORDER,
    CANONICAL_SECTION_ORDER, REQUIRED_SECTIONS,
};
use agentmesh_evidence::sha256_prefixed;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Plugin/schema version exposed in compact output.
pub const RENDERER_VERSION: &str = "request-markdown-renderer.v0";
const INPUT_SCHEMA_VERSION: &str = "request-markdown-renderer-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "request-markdown-renderer-compact.v0";
const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";
const MAX_SUMMARY_BYTES: usize = 64 * 1024;
const MAX_DIAGNOSTICS: usize = 32;
const ACCEPTED_REQUEST_KINDS: &[&str] = &["app", "repair"];
const STRING_FIELDS: &[&str] = &[
    "title",
    "request_kind",
    "issue_type",
    "status",
    "project_key",
    "source_prd",
    "source_design",
    "source_roadmap",
];
const STRING_ARRAY_FIELDS: &[&str] = &["blocked_by", "unblocks"];
const POSITIVE_INTEGER_FIELDS: &[&str] = &["sequence_index", "sequence_total"];
/// Fields that appear in source frontmatter but are intentionally not part of
/// the rendered canonical document.
const NON_RENDERED_COMPATIBILITY_FIELDS: &[&str] = &["ready_for_multica"];

#[derive(Debug)]
struct ErrorRecord {
    code: &'static str,
    category: &'static str,
    path: String,
    message: String,
}

#[derive(Debug)]
struct DiagnosticRecord {
    code: &'static str,
    category: &'static str,
    path: String,
    message: String,
}

#[derive(Debug)]
struct MismatchRecord {
    code: &'static str,
    path: String,
    message: String,
}

/// Render a canonical request summary into a canonical request Markdown document.
pub fn render_request_markdown(value: &Value) -> Value {
    let mut errors = Vec::new();
    let mut diagnostics = Vec::new();

    let Some(input) = value.as_object() else {
        errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_RENDERER_INPUT_INVALID",
            "input_schema_invalid",
            "$",
            "input must be a JSON object",
        ));
        return compact(None, diagnostics, errors);
    };

    match input.get("schema_version").and_then(Value::as_str) {
        Some(INPUT_SCHEMA_VERSION) => {}
        Some(version) => errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_RENDERER_INPUT_INVALID",
            "input_schema_invalid",
            "$.schema_version",
            format!("schema_version must be {INPUT_SCHEMA_VERSION}, got {version:?}"),
        )),
        None => errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_REQUIRED",
            "missing_field",
            "$.schema_version",
            "schema_version is required",
        )),
    }

    if let Some(version) = input.get("request_schema_version") {
        match version.as_str() {
            Some(REQUEST_SCHEMA_VERSION) => {}
            Some(version) => errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_INPUT_INVALID",
                "input_schema_invalid",
                "$.request_schema_version",
                format!("request_schema_version must be {REQUEST_SCHEMA_VERSION}, got {version:?}"),
            )),
            None => errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_INPUT_INVALID",
                "input_schema_invalid",
                "$.request_schema_version",
                "request_schema_version must be a string when provided",
            )),
        }
    }

    if let Some(extras) = input.get("adapter_extras") {
        match extras.as_object() {
            Some(extras) => collect_adapter_extras("$.adapter_extras", extras, &mut diagnostics),
            None => errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_INPUT_INVALID",
                "input_schema_invalid",
                "$.adapter_extras",
                "adapter_extras must be a JSON object when provided",
            )),
        }
    }

    let rendered = parse_summary(input, &mut errors, &mut diagnostics).and_then(|summary| {
        if summary_bytes(input) > MAX_SUMMARY_BYTES {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_INPUT_INVALID",
                "input_schema_invalid",
                "$.summary",
                format!(
                    "summary is {} bytes; limit is {MAX_SUMMARY_BYTES}",
                    summary_bytes(input)
                ),
            ));
            return None;
        }
        Some(summary)
    });

    let rendered = rendered.filter(|_| errors.is_empty());
    compact(rendered, diagnostics, errors)
}

#[derive(Debug)]
struct SummaryInput {
    fields: Map<String, Value>,
    section_bodies: BTreeMap<&'static str, Vec<String>>,
}

fn summary_bytes(input: &Map<String, Value>) -> usize {
    serde_json::to_vec(&Value::Object(input.clone())).map_or(usize::MAX, |bytes| bytes.len())
}

fn parse_summary(
    input: &Map<String, Value>,
    errors: &mut Vec<ErrorRecord>,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> Option<SummaryInput> {
    let Some(Value::Object(summary)) = input.get("summary") else {
        errors.push(match input.get("summary") {
            None | Some(Value::Null) => error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_REQUIRED",
                "missing_field",
                "$.summary",
                "summary is required",
            ),
            Some(_) => error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_MALFORMED",
                "malformed_summary",
                "$.summary",
                "summary must be a JSON object",
            ),
        });
        return None;
    };

    match summary.get("valid") {
        Some(Value::Bool(true)) => {}
        Some(Value::Bool(false)) => errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_INVALID",
            "invalid_summary",
            "$.summary.valid",
            "summary.valid must be true before a canonical document is rendered",
        )),
        Some(_) => errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_MALFORMED",
            "malformed_summary",
            "$.summary.valid",
            "summary.valid must be a boolean",
        )),
        None => errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_REQUIRED",
            "missing_field",
            "$.summary.valid",
            "summary.valid is required",
        )),
    }

    let canonical = match summary.get("canonical").or_else(|| summary.get("fields")) {
        Some(Value::Object(fields)) => fields,
        None => {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_REQUIRED",
                "missing_field",
                "$.summary.canonical",
                "summary.canonical is required",
            ));
            return None;
        }
        Some(_) => {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_MALFORMED",
                "malformed_summary",
                "$.summary.canonical",
                "summary.canonical must be a JSON object",
            ));
            return None;
        }
    };

    for key in summary.keys() {
        if !matches!(key.as_str(), "valid" | "canonical" | "fields" | "sections") {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_MALFORMED",
                "malformed_summary",
                format!("$.summary.{key}"),
                format!("summary key {key:?} is not part of the canonical summary contract"),
            ));
        }
    }

    let fields = canonical_fields(canonical, diagnostics, errors);
    let section_bodies = section_bodies(summary.get("sections"), diagnostics, errors);

    if !errors.is_empty() {
        return None;
    }
    Some(SummaryInput {
        fields,
        section_bodies,
    })
}

fn canonical_fields(
    canonical: &Map<String, Value>,
    diagnostics: &mut Vec<DiagnosticRecord>,
    errors: &mut Vec<ErrorRecord>,
) -> Map<String, Value> {
    let mut fields = Map::new();

    for key in canonical.keys() {
        if !CANONICAL_FRONTMATTER_ORDER.contains(&key.as_str())
            && !NON_RENDERED_COMPATIBILITY_FIELDS.contains(&key.as_str())
        {
            diagnostics.push(diagnostic(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_ADAPTER_OWNED_FIELD",
                "adapter_owned_field",
                format!("$.summary.canonical.{key}"),
                format!("field {key:?} is adapter-owned and is never rendered into the canonical source"),
            ));
        }
    }
    for key in NON_RENDERED_COMPATIBILITY_FIELDS {
        if canonical.contains_key(*key) {
            diagnostics.push(diagnostic(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_NON_CANONICAL_FIELD",
                "non_canonical_field",
                format!("$.summary.canonical.{key}"),
                format!("field {key:?} is a source compatibility hint and is not part of the rendered canonical document"),
            ));
        }
    }

    for key in CANONICAL_FRONTMATTER_ORDER {
        let Some(value) = canonical.get(*key) else {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_REQUIRED",
                "missing_field",
                format!("$.summary.canonical.{key}"),
                format!("request field {key} is required"),
            ));
            continue;
        };
        fields.insert((*key).to_string(), value.clone());
    }

    if !errors.is_empty() {
        return fields;
    }

    validate_string_fields(&fields, errors);
    validate_string_array_fields(&fields, errors);
    validate_positive_integer_fields(&fields, errors);
    validate_request_kind(&fields, errors);
    validate_sequence(&fields, errors);
    fields
}

fn validate_string_fields(fields: &Map<String, Value>, errors: &mut Vec<ErrorRecord>) {
    for key in STRING_FIELDS {
        let valid = fields
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty());
        if !valid {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_INVALID",
                "invalid_field_value",
                format!("$.summary.canonical.{key}"),
                format!("request field {key} must be a non-empty string"),
            ));
        }
    }
}

fn validate_string_array_fields(fields: &Map<String, Value>, errors: &mut Vec<ErrorRecord>) {
    for key in STRING_ARRAY_FIELDS {
        let valid = fields
            .get(*key)
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().all(Value::is_string));
        if !valid {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_INVALID",
                "invalid_field_value",
                format!("$.summary.canonical.{key}"),
                format!("request field {key} must be an array of strings"),
            ));
        }
    }
}

fn validate_positive_integer_fields(fields: &Map<String, Value>, errors: &mut Vec<ErrorRecord>) {
    for key in POSITIVE_INTEGER_FIELDS {
        let valid = fields
            .get(*key)
            .and_then(Value::as_u64)
            .is_some_and(|number| number > 0);
        if !valid {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_INVALID",
                "invalid_field_value",
                format!("$.summary.canonical.{key}"),
                format!("request field {key} must be a positive integer"),
            ));
        }
    }
}

fn validate_request_kind(fields: &Map<String, Value>, errors: &mut Vec<ErrorRecord>) {
    let Some(request_kind) = fields.get("request_kind").and_then(Value::as_str) else {
        return;
    };
    if ACCEPTED_REQUEST_KINDS.contains(&request_kind) {
        return;
    }
    errors.push(error(
        "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_INVALID",
        "invalid_field_value",
        "$.summary.canonical.request_kind",
        "request_kind must be one of app, repair",
    ));
}

fn validate_sequence(fields: &Map<String, Value>, errors: &mut Vec<ErrorRecord>) {
    let sequence_index = fields.get("sequence_index").and_then(Value::as_u64);
    let sequence_total = fields.get("sequence_total").and_then(Value::as_u64);
    let (Some(sequence_index), Some(sequence_total)) = (sequence_index, sequence_total) else {
        return;
    };
    if sequence_index > sequence_total {
        errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_INVALID",
            "invalid_field_value",
            "$.summary.canonical.sequence_index",
            "sequence_index must be less than or equal to sequence_total",
        ));
    }
}

fn section_bodies(
    sections: Option<&Value>,
    diagnostics: &mut Vec<DiagnosticRecord>,
    errors: &mut Vec<ErrorRecord>,
) -> BTreeMap<&'static str, Vec<String>> {
    let mut bodies = BTreeMap::new();
    let mut seen = BTreeSet::new();

    let entries: Vec<(String, String)> = match sections {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| match item.as_object() {
                Some(object) => Some((
                    object
                        .get("heading")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    object
                        .get("body")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ))
                .or_else(|| {
                    errors.push(error(
                        "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_MALFORMED",
                        "malformed_summary",
                        format!("$.summary.sections[{index}]"),
                        "section entries must be objects with heading and body strings",
                    ));
                    None
                }),
                None => {
                    errors.push(error(
                        "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_MALFORMED",
                        "malformed_summary",
                        format!("$.summary.sections[{index}]"),
                        "section entries must be objects with heading and body strings",
                    ));
                    None
                }
            })
            .collect(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(heading, body)| {
                (
                    heading.clone(),
                    body.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        Some(_) => {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_MALFORMED",
                "malformed_summary",
                "$.summary.sections",
                "sections must be an array of objects or an object map of heading to body",
            ));
            return bodies;
        }
    };

    for (heading, body) in entries {
        let Some(canonical) = canonical_section_name(&heading) else {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SECTION_UNSUPPORTED",
                "unsupported_section_structure",
                format!("$.summary.sections.{heading}"),
                format!("unsupported section heading {heading:?}"),
            ));
            continue;
        };
        if !seen.insert(canonical) {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SECTION_UNSUPPORTED",
                "unsupported_section_structure",
                format!("$.summary.sections.{canonical}"),
                format!("section {canonical:?} appears more than once"),
            ));
            continue;
        }
        bodies.insert(canonical, body_lines(&body));
    }

    for heading in REQUIRED_SECTIONS {
        if !bodies.contains_key(heading) {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SECTION_REQUIRED",
                "missing_section",
                format!("$.summary.sections.{heading}"),
                format!("section {heading:?} is required"),
            ));
        }
    }

    for (heading, lines) in &bodies {
        for line in lines {
            if line.trim_start().starts_with('#') {
                diagnostics.push(diagnostic(
                    "AGENTMESH_REQUEST_MARKDOWN_RENDERER_NESTED_HEADING",
                    "non_canonical_body",
                    format!("$.summary.sections.{heading}"),
                    "section body line starts with '#' and is reported as a round-trip risk",
                ));
                break;
            }
        }
    }

    bodies
}

fn body_lines(body: &str) -> Vec<String> {
    let normalized = body.replace("\r\n", "\n");
    let lines: Vec<String> = normalized.lines().map(ToString::to_string).collect();
    if lines.iter().all(|line| line.trim().is_empty()) {
        // The canonical writer emits `- none` for an empty section body; using the
        // same placeholder keeps the round-trip digest comparison honest.
        return vec!["- none".to_string()];
    }
    lines
}

fn canonical_section_name(heading: &str) -> Option<&'static str> {
    CANONICAL_SECTION_ORDER
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(heading.trim()))
        .copied()
}

fn collect_adapter_extras(
    path: &str,
    extras: &Map<String, Value>,
    diagnostics: &mut Vec<DiagnosticRecord>,
) {
    for key in extras.keys() {
        diagnostics.push(diagnostic(
            "AGENTMESH_REQUEST_MARKDOWN_RENDERER_ADAPTER_OWNED_FIELD",
            "adapter_owned_field",
            format!("{path}.{key}"),
            format!("adapter-owned field {key:?} is reported and never rendered"),
        ));
    }
}

fn compact(
    summary: Option<SummaryInput>,
    mut diagnostics: Vec<DiagnosticRecord>,
    mut errors: Vec<ErrorRecord>,
) -> Value {
    errors.sort_by(|left, right| {
        left.code
            .cmp(right.code)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.message.cmp(&right.message))
    });
    diagnostics.sort_by(|left, right| {
        left.category
            .cmp(right.category)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.message.cmp(&right.message))
    });
    let valid = errors.is_empty();
    let errors_json: Vec<Value> = errors.iter().map(ErrorRecord::value).collect();
    let (render, round_trip) = summary
        .filter(|_| valid)
        .map(render_summary)
        .unwrap_or_else(|| (None, Value::Null));
    let diagnostics_json = bounded_diagnostics(diagnostics);

    sort_json_keys(json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "renderer_version": RENDERER_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "valid": valid,
        "serialization": serialization_contract(),
        "request_slug": render.as_ref().map(|render| render.slug.clone()),
        "markdown": render.as_ref().map(|render| render.markdown.clone()),
        "content_hashes": render.as_ref().map(|render| json!({
            "algorithm": "sha256",
            "markdown_sha256": sha256_prefixed(render.markdown.as_bytes()),
            "input_canonical_sha256": render.input_digest,
        })),
        "round_trip": round_trip,
        "diagnostic_count": diagnostics_json.len(),
        "diagnostics": diagnostics_json,
        "error_count": errors_json.len(),
        "errors": errors_json,
    }))
}

struct Render {
    slug: String,
    markdown: String,
    input_digest: String,
}

fn render_summary(summary: SummaryInput) -> (Option<Render>, Value) {
    let SummaryInput {
        fields,
        section_bodies,
    } = summary;
    let document = match canonical_document_from_summary(fields, &section_bodies) {
        Ok(document) => document,
        Err(canonical_errors) => {
            let message = format!(
                "canonical projection rejected the summary: {}",
                canonical_errors
                    .iter()
                    .filter_map(|record| record.get("code").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            return (
                None,
                json!({
                    "status": "mismatch",
                    "input_canonical_sha256": Value::Null,
                    "rendered_canonical_sha256": Value::Null,
                    "mismatch_count": 1,
                    "mismatches": [{
                        "code": "AGENTMESH_REQUEST_MARKDOWN_RENDERER_RENDER_UNPARSEABLE",
                        "path": "$.summary",
                        "message": message,
                    }],
                }),
            );
        }
    };

    let input_projection = document.projection;
    let input_digest = sha256_prefixed(&canonical_json_bytes(&input_projection));
    let slug = input_projection
        .get("request_slug")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let markdown = document.markdown;

    let reentered = reenter_canonical_markdown(&markdown);
    let round_trip = round_trip(&input_projection, &input_digest, &reentered);

    (
        Some(Render {
            slug,
            markdown,
            input_digest,
        }),
        round_trip,
    )
}

fn round_trip(input_projection: &Value, input_digest: &str, reentered: &Value) -> Value {
    if reentered.get("valid").and_then(Value::as_bool) != Some(true) {
        let mut mismatches = vec![json!({
            "code": "AGENTMESH_REQUEST_MARKDOWN_RENDERER_RENDER_UNPARSEABLE",
            "path": "$.markdown",
            "message": "rendered Markdown did not re-enter the canonical request parser",
        })];
        let reentered_error_codes: Vec<Value> = reentered
            .get("errors")
            .and_then(Value::as_array)
            .map(|errors| {
                errors
                    .iter()
                    .filter_map(|record| record.get("code").cloned())
                    .collect()
            })
            .unwrap_or_default();
        mismatches.push(json!({
            "code": "AGENTMESH_REQUEST_MARKDOWN_RENDERER_RENDER_PARSER_ERRORS",
            "path": "$.markdown.errors",
            "message": format!(
                "canonical parser error codes: {}",
                reentered_error_codes
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }));
        return json!({
            "status": "mismatch",
            "input_canonical_sha256": input_digest,
            "rendered_canonical_sha256": Value::Null,
            "mismatch_count": mismatches.len(),
            "mismatches": mismatches,
        });
    }

    let rendered_projection = reentered.get("projection").cloned().unwrap_or(Value::Null);
    let rendered_digest = reentered
        .get("content_hashes")
        .and_then(|hashes| hashes.get("projection_sha256"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut mismatches = Vec::new();
    compare_fields(input_projection, &rendered_projection, &mut mismatches);
    compare_sections(input_projection, &rendered_projection, &mut mismatches);
    compare_requirements(input_projection, &rendered_projection, &mut mismatches);
    compare_slug(input_projection, &rendered_projection, &mut mismatches);
    if mismatches.is_empty() && input_digest != rendered_digest {
        mismatches.push(MismatchRecord {
            code: "AGENTMESH_REQUEST_MARKDOWN_RENDERER_DIGEST_MISMATCH",
            path: "$.round_trip".to_string(),
            message: format!(
                "input digest {input_digest} differs from rendered digest {rendered_digest}"
            ),
        });
    }

    let status = if mismatches.is_empty() {
        "match"
    } else {
        "mismatch"
    };
    json!({
        "status": status,
        "input_canonical_sha256": input_digest,
        "rendered_canonical_sha256": rendered_digest,
        "mismatch_count": mismatches.len(),
        "mismatches": mismatches
            .iter()
            .map(|mismatch| json!({
                "code": mismatch.code,
                "path": mismatch.path,
                "message": mismatch.message,
            }))
            .collect::<Vec<_>>(),
    })
}

fn compare_fields(input: &Value, rendered: &Value, mismatches: &mut Vec<MismatchRecord>) {
    for key in CANONICAL_FRONTMATTER_ORDER {
        let input_value = input.get("fields").and_then(|fields| fields.get(*key));
        let rendered_value = rendered.get("fields").and_then(|fields| fields.get(*key));
        if input_value != rendered_value {
            mismatches.push(MismatchRecord {
                code: "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_MISMATCH",
                path: format!("$.round_trip.fields.{key}"),
                message: format!(
                    "canonical field {key} changed across the render round-trip: {} -> {}",
                    inline(input_value.unwrap_or(&Value::Null)),
                    inline(rendered_value.unwrap_or(&Value::Null))
                ),
            });
        }
    }
}

fn compare_sections(input: &Value, rendered: &Value, mismatches: &mut Vec<MismatchRecord>) {
    let input_sections = input
        .get("sections")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rendered_sections = rendered
        .get("sections")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if input_sections.len() != rendered_sections.len() {
        mismatches.push(MismatchRecord {
            code: "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SECTION_MISMATCH",
            path: "$.round_trip.sections".to_string(),
            message: format!(
                "canonical section count changed across the render round-trip: {} -> {}",
                input_sections.len(),
                rendered_sections.len()
            ),
        });
        return;
    }
    for (index, (input_section, rendered_section)) in input_sections
        .iter()
        .zip(rendered_sections.iter())
        .enumerate()
    {
        if input_section != rendered_section {
            let heading = input_section
                .get("heading")
                .and_then(Value::as_str)
                .unwrap_or_default();
            mismatches.push(MismatchRecord {
                code: "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SECTION_MISMATCH",
                path: format!("$.round_trip.sections[{index}]"),
                message: format!(
                    "canonical section {heading:?} body changed across the render round-trip"
                ),
            });
        }
    }
}

fn compare_requirements(input: &Value, rendered: &Value, mismatches: &mut Vec<MismatchRecord>) {
    let input_requirements = input
        .get("requirements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rendered_requirements = rendered
        .get("requirements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if input_requirements != rendered_requirements {
        mismatches.push(MismatchRecord {
            code: "AGENTMESH_REQUEST_MARKDOWN_RENDERER_REQUIREMENT_MISMATCH",
            path: "$.round_trip.requirements".to_string(),
            message: format!(
                "canonical requirements changed across the render round-trip: {} -> {}",
                input_requirements.len(),
                rendered_requirements.len()
            ),
        });
    }
}

fn compare_slug(input: &Value, rendered: &Value, mismatches: &mut Vec<MismatchRecord>) {
    let input_slug = input.get("request_slug");
    let rendered_slug = rendered.get("request_slug");
    if input_slug != rendered_slug {
        mismatches.push(MismatchRecord {
            code: "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SLUG_MISMATCH",
            path: "$.round_trip.request_slug".to_string(),
            message: format!(
                "request slug changed across the render round-trip: {} -> {}",
                inline(input_slug.unwrap_or(&Value::Null)),
                inline(rendered_slug.unwrap_or(&Value::Null))
            ),
        });
    }
}

fn inline(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

fn bounded_diagnostics(diagnostics: Vec<DiagnosticRecord>) -> Vec<Value> {
    let total = diagnostics.len();
    let mut records: Vec<Value> = diagnostics
        .into_iter()
        .take(MAX_DIAGNOSTICS)
        .map(|diagnostic| {
            json!({
                "code": diagnostic.code,
                "category": diagnostic.category,
                "path": diagnostic.path,
                "message": diagnostic.message,
            })
        })
        .collect();
    if total > MAX_DIAGNOSTICS {
        records.push(json!({
            "code": "AGENTMESH_REQUEST_MARKDOWN_RENDERER_DIAGNOSTICS_TRUNCATED",
            "category": "bounded_diagnostics",
            "path": "$.summary",
            "message": format!(
                "diagnostics truncated: {} recorded, {} reported",
                total,
                records.len()
            ),
        }));
    }
    records
}

fn serialization_contract() -> Value {
    json!({
        "format": "canonical_request_markdown",
        "object_key_order": "lexicographic",
        "frontmatter_field_order": CANONICAL_FRONTMATTER_ORDER,
        "section_order": CANONICAL_SECTION_ORDER,
        "required_sections": REQUIRED_SECTIONS,
        "bullet_style": "dash-space; acceptance requirements render as - [ ] or - [x]",
        "line_endings": "LF",
        "whitespace": "trim line edges; collapse inline whitespace; one canonical heading block per section",
        "empty_section_placeholder": "- none",
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

fn error(
    code: &'static str,
    category: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> ErrorRecord {
    ErrorRecord {
        code,
        category,
        path: path.into(),
        message: message.into(),
    }
}

impl ErrorRecord {
    fn value(&self) -> Value {
        json!({
            "code": self.code,
            "category": self.category,
            "severity": "error",
            "path": self.path,
            "message": self.message,
            "remediation_hint": remediation_hint(self.code),
        })
    }
}

fn diagnostic(
    code: &'static str,
    category: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> DiagnosticRecord {
    DiagnosticRecord {
        code,
        category,
        path: path.into(),
        message: message.into(),
    }
}

fn remediation_hint(code: &str) -> &'static str {
    match code {
        "AGENTMESH_REQUEST_MARKDOWN_RENDERER_INPUT_INVALID" => {
            "Match request-markdown-renderer-input.v0 and provide one canonical summary below the byte limit."
        }
        "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_REQUIRED" => {
            "Add the missing contract field before rendering a canonical request document."
        }
        "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_INVALID" => {
            "Use the documented scalar type and canonical request_kind/sequence values, then rerun the renderer."
        }
        "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_INVALID" => {
            "Render only a validated canonical summary; invalid summaries never produce Markdown."
        }
        "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_MALFORMED" => {
            "Provide summary.valid, summary.canonical fields, and canonical section objects."
        }
        "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SECTION_REQUIRED" => {
            "Add every required canonical section body so the rendered source can re-enter the parser."
        }
        "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SECTION_UNSUPPORTED" => {
            "Use only the documented canonical section headings once per document."
        }
        _ => "Use the normalized category, path, and message to repair the summary, then rerun.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_summary() -> Value {
        json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request_schema_version": REQUEST_SCHEMA_VERSION,
            "summary": {
                "valid": true,
                "canonical": {
                    "title": "Add a canonical request Markdown renderer App",
                    "request_kind": "app",
                    "issue_type": "AFK",
                    "ready_for_multica": true,
                    "status": "ready",
                    "project_key": "agentmesh-private",
                    "source_prd": "synthetic://requests/request-markdown-renderer",
                    "source_design": "synthetic://docs/agentmesh-request-operations-v1",
                    "source_roadmap": "synthetic://roadmaps/agentmesh-private",
                    "blocked_by": [],
                    "unblocks": [],
                    "sequence_index": 1,
                    "sequence_total": 1
                },
                "sections": {
                    "What to build": "Render canonical summaries deterministically.",
                    "Acceptance criteria": "- [ ] Round-trip digest matches.\n- [ ] Rendered Markdown stays byte-stable.",
                    "Blocked by": "- None - can start immediately.",
                    "User stories covered": "- As a non-Multica adapter maintainer, I can regenerate request sources.",
                    "Notes": "Request kind: `app`"
                }
            }
        })
    }

    #[test]
    fn valid_summary_renders_canonical_markdown_and_matches_round_trip() {
        let output = render_request_markdown(&valid_summary());
        assert_eq!(output["valid"], true, "{}", output["errors"]);
        assert_eq!(output["schema_version"], OUTPUT_SCHEMA_VERSION);
        assert_eq!(output["renderer_version"], RENDERER_VERSION);
        assert_eq!(
            output["request_slug"],
            "add-a-canonical-request-markdown-renderer-app"
        );
        assert_eq!(output["round_trip"]["status"], "match");
        assert_eq!(output["round_trip"]["mismatch_count"], 0);
        assert_eq!(
            output["round_trip"]["input_canonical_sha256"],
            output["round_trip"]["rendered_canonical_sha256"]
        );
        let markdown = output["markdown"].as_str().expect("rendered markdown");
        assert!(
            markdown.starts_with("---\ntitle: \"Add a canonical request Markdown renderer App\"\n")
        );
        assert!(!markdown.contains('\r'));
        assert!(!markdown.contains("ready_for_multica"));
    }

    #[test]
    fn repeated_runs_are_byte_identical() {
        let first = render_request_markdown(&valid_summary());
        let second = render_request_markdown(&valid_summary());
        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
    }

    #[test]
    fn adapter_owned_fields_are_diagnostics_and_never_rendered() {
        let mut input = valid_summary();
        input["summary"]["canonical"]["multica_issue_id"] = json!("DOT-9999");
        input["adapter_extras"] = json!({"tracker_assignee": "agent", "sidecar_path": "ignored"});
        let output = render_request_markdown(&input);
        assert_eq!(output["valid"], true, "{}", output["errors"]);
        assert_eq!(output["round_trip"]["status"], "match");
        assert_eq!(output["diagnostic_count"], 4);
        let categories: Vec<_> = output["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|record| record["category"].as_str().unwrap())
            .collect();
        assert!(categories.contains(&"adapter_owned_field"));
        assert!(categories.contains(&"non_canonical_field"));
        let markdown = output["markdown"].as_str().unwrap();
        assert!(!markdown.contains("DOT-9999"));
        assert!(!markdown.contains("tracker_assignee"));
    }

    #[test]
    fn missing_required_field_fails_without_markdown() {
        let mut input = valid_summary();
        let canonical = input["summary"]["canonical"].as_object_mut().unwrap();
        canonical.remove("source_design");
        let output = render_request_markdown(&input);
        assert_eq!(output["valid"], false);
        assert_eq!(output["markdown"], Value::Null);
        assert_eq!(output["round_trip"], Value::Null);
        assert_eq!(output["error_count"], 1);
        assert_eq!(
            output["errors"][0]["code"],
            "AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_REQUIRED"
        );
    }

    #[test]
    fn unsupported_schema_version_and_oversized_summary_fail_deterministically() {
        let mut input = valid_summary();
        input["schema_version"] = json!("request-markdown-renderer-input.v9");
        let output = render_request_markdown(&input);
        assert_eq!(output["valid"], false);
        assert_eq!(output["markdown"], Value::Null);
        assert_eq!(
            output["errors"][0]["code"],
            "AGENTMESH_REQUEST_MARKDOWN_RENDERER_INPUT_INVALID"
        );

        let mut input = valid_summary();
        input["summary"]["sections"]["Notes"] = json!("x".repeat(MAX_SUMMARY_BYTES));
        let output = render_request_markdown(&input);
        assert_eq!(output["valid"], false);
        assert_eq!(output["markdown"], Value::Null);
        assert!(output["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["message"].as_str().unwrap().contains("limit is")));
    }

    #[test]
    fn malformed_summary_shapes_fail_without_partial_markdown() {
        for (mutate, expected_code) in [
            (
                Box::new(|input: &mut Value| {
                    input["summary"] = json!("not-an-object");
                }) as Box<dyn Fn(&mut Value)>,
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_MALFORMED",
            ),
            (
                Box::new(|input: &mut Value| {
                    input["summary"]["valid"] = json!(false);
                }),
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_INVALID",
            ),
            (
                Box::new(|input: &mut Value| {
                    input["summary"]["sections"] = json!([]);
                }),
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SECTION_REQUIRED",
            ),
            (
                Box::new(|input: &mut Value| {
                    input["summary"]["sections"]["Unknown section"] = json!("x");
                }),
                "AGENTMESH_REQUEST_MARKDOWN_RENDERER_SECTION_UNSUPPORTED",
            ),
        ] {
            let mut input = valid_summary();
            mutate(&mut input);
            let output = render_request_markdown(&input);
            assert_eq!(output["valid"], false);
            assert_eq!(output["markdown"], Value::Null);
            assert!(
                output["errors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|record| record["code"] == expected_code),
                "{expected_code} missing from {}",
                output["errors"]
            );
        }
    }

    #[test]
    fn nested_heading_in_body_reports_round_trip_mismatch() {
        let mut input = valid_summary();
        input["summary"]["sections"]["Notes"] = json!("### Nested\nNot canonical.");
        let output = render_request_markdown(&input);
        assert_eq!(output["valid"], true, "{}", output["errors"]);
        assert_eq!(output["round_trip"]["status"], "mismatch");
        let codes: Vec<_> = output["round_trip"]["mismatches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|record| record["code"].as_str().unwrap())
            .collect();
        assert!(codes.contains(&"AGENTMESH_REQUEST_MARKDOWN_RENDERER_RENDER_UNPARSEABLE"));
        assert!(output["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["code"] == "AGENTMESH_REQUEST_MARKDOWN_RENDERER_NESTED_HEADING"));
    }

    #[test]
    fn recorded_fixtures_match_expected_payloads() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "request_markdown_renderer_complete_input.json",
                "expected_request_markdown_renderer_complete_payload.json",
            ),
            (
                "request_markdown_renderer_repair_input.json",
                "expected_request_markdown_renderer_repair_payload.json",
            ),
            (
                "request_markdown_renderer_missing_field_input.json",
                "expected_request_markdown_renderer_missing_field_payload.json",
            ),
            (
                "request_markdown_renderer_adapter_extras_input.json",
                "expected_request_markdown_renderer_adapter_extras_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            assert_eq!(
                render_request_markdown(&input),
                expected,
                "{input_name} should match {expected_name}"
            );
        }
    }
}
