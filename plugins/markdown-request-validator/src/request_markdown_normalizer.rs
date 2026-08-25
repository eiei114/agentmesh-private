//! Deterministic request Markdown normalizer App.
//!
//! The normalizer accepts one already-authored AgentMesh request Markdown
//! document, projects only the tool-neutral request fields local runners need,
//! and emits byte-stable JSON/Markdown previews for fixture comparison.

use agentmesh_evidence::sha256_prefixed;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Plugin/schema version exposed in compact output.
pub const NORMALIZER_VERSION: &str = "request-markdown-normalizer.v0";
const INPUT_SCHEMA_VERSION: &str = "request-markdown-normalizer-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "request-markdown-normalizer-compact.v0";
const PROJECTION_SCHEMA_VERSION: &str = "request-markdown-normalizer-projection.v0";
const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";
const MAX_MARKDOWN_BYTES: usize = 64 * 1024;
const SLUG_MAX_CHARS: usize = 80;
const ACCEPTED_REQUEST_KINDS: &[&str] = &["app", "repair"];
const CANONICAL_FRONTMATTER_ORDER: &[&str] = &[
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
const CANONICAL_SECTION_ORDER: &[&str] = &[
    "Parent",
    "What to build",
    "Acceptance criteria",
    "Blocked by",
    "User stories covered",
    "Notes",
];
const REQUIRED_SECTIONS: &[&str] = &[
    "What to build",
    "Acceptance criteria",
    "Blocked by",
    "User stories covered",
    "Notes",
];

#[derive(Debug, Clone, Copy)]
enum FieldKind {
    String,
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

#[derive(Debug, Clone, Eq, PartialEq)]
struct Requirement {
    checked: bool,
    text: String,
}

#[derive(Debug, Clone)]
struct CanonicalSection {
    heading: &'static str,
    body: String,
}

#[derive(Debug, Clone)]
struct ParsedDocument {
    fields: Map<String, Value>,
    sections: Vec<CanonicalSection>,
    requirements: Vec<Requirement>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ErrorRecord {
    code: &'static str,
    category: &'static str,
    path: String,
    message: String,
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

/// Normalize request Markdown into a deterministic, adapter-neutral projection.
pub fn normalize_request_markdown(value: &Value) -> Value {
    let mut errors = Vec::new();
    let Some(input) = value.as_object() else {
        errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_INPUT_INVALID",
            "input_schema_invalid",
            "$",
            "input must be a JSON object",
        ));
        return compact(None, errors);
    };

    validate_input_schema(input, &mut errors);
    let markdown = markdown_source(input, &mut errors);
    let parsed = markdown.and_then(|source| parse_document(source, &mut errors));

    compact(parsed, errors)
}

fn validate_input_schema(input: &Map<String, Value>, errors: &mut Vec<ErrorRecord>) {
    match input.get("schema_version").and_then(Value::as_str) {
        Some(INPUT_SCHEMA_VERSION) => {}
        Some(_) => errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_INPUT_INVALID",
            "input_schema_invalid",
            "$.schema_version",
            format!("schema_version must be {INPUT_SCHEMA_VERSION}"),
        )),
        None => errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FIELD_REQUIRED",
            "missing_field",
            "$.schema_version",
            "schema_version is required",
        )),
    }

    if let Some(version) = input.get("request_schema_version") {
        match version.as_str() {
            Some(REQUEST_SCHEMA_VERSION) => {}
            Some(_) => errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_INPUT_INVALID",
                "input_schema_invalid",
                "$.request_schema_version",
                format!("request_schema_version must be {REQUEST_SCHEMA_VERSION}"),
            )),
            None => errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_INPUT_INVALID",
                "input_schema_invalid",
                "$.request_schema_version",
                "request_schema_version must be a string when provided",
            )),
        }
    }
}

fn markdown_source<'a>(
    input: &'a Map<String, Value>,
    errors: &mut Vec<ErrorRecord>,
) -> Option<&'a str> {
    if input.contains_key("request") {
        errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_UNSUPPORTED_SHAPE",
            "unsupported_request_shape",
            "$",
            "request-markdown-normalizer accepts exactly one markdown source and no request object",
        ));
        return None;
    }

    match input.get("markdown") {
        Some(Value::String(markdown)) if !markdown.trim().is_empty() => Some(markdown),
        Some(Value::String(_)) | None | Some(Value::Null) => {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FIELD_REQUIRED",
                "missing_field",
                "$.markdown",
                "markdown is required",
            ));
            None
        }
        Some(_) => {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_INPUT_INVALID",
                "input_schema_invalid",
                "$.markdown",
                "markdown must be a string",
            ));
            None
        }
    }
}

fn parse_document(markdown: &str, errors: &mut Vec<ErrorRecord>) -> Option<ParsedDocument> {
    if markdown.len() > MAX_MARKDOWN_BYTES {
        errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_INPUT_INVALID",
            "input_schema_invalid",
            "$.markdown",
            format!(
                "markdown is {} bytes; limit is {MAX_MARKDOWN_BYTES}",
                markdown.len()
            ),
        ));
    }

    let normalized = markdown
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    let Some((frontmatter, body)) = parse_frontmatter(&normalized) else {
        errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FRONTMATTER_MALFORMED",
            "malformed_frontmatter",
            "$.markdown",
            "markdown requests require a complete YAML frontmatter block",
        ));
        return None;
    };

    let fields = parse_frontmatter_fields(frontmatter, errors);
    validate_fields(&fields, errors);
    let raw_sections = parse_sections(body, errors);
    let (sections, requirements) = canonical_sections(&raw_sections, errors);

    Some(ParsedDocument {
        fields,
        sections,
        requirements,
    })
}

fn parse_frontmatter(markdown: &str) -> Option<(&str, &str)> {
    let rest = markdown.strip_prefix("---\n")?;
    if let Some(end) = rest.find("\n---\n") {
        let body_start = end + "\n---\n".len();
        return Some((&rest[..end], &rest[body_start..]));
    }

    let frontmatter = rest.strip_suffix("\n---")?;
    Some((frontmatter, ""))
}

fn parse_frontmatter_fields(
    frontmatter: &str,
    errors: &mut Vec<ErrorRecord>,
) -> Map<String, Value> {
    let mut fields = Map::new();
    let mut seen = BTreeSet::new();

    for (line_index, line) in frontmatter.lines().enumerate() {
        let line_number = line_index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with("- ") {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FRONTMATTER_MALFORMED",
                "malformed_frontmatter",
                format!("$.markdown.frontmatter.line{line_number}"),
                "frontmatter list syntax is not supported; use single-line key: value entries",
            ));
            continue;
        }
        let Some((key, raw)) = trimmed.split_once(':') else {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FRONTMATTER_MALFORMED",
                "malformed_frontmatter",
                format!("$.markdown.frontmatter.line{line_number}"),
                "frontmatter entries must use key: value syntax",
            ));
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FRONTMATTER_MALFORMED",
                "malformed_frontmatter",
                format!("$.markdown.frontmatter.line{line_number}"),
                "frontmatter key must not be empty",
            ));
            continue;
        }
        if !seen.insert(key.to_string()) {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FIELD_INVALID",
                "invalid_field_value",
                format!("$.markdown.frontmatter.{key}"),
                format!("frontmatter field {key} appears more than once"),
            ));
            continue;
        }
        let (value, parse_error) = scalar(raw.trim());
        if let Some(message) = parse_error {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FRONTMATTER_MALFORMED",
                "malformed_frontmatter",
                format!("$.markdown.frontmatter.{key}"),
                message,
            ));
        }
        fields.insert(key.to_string(), normalize_value(value));
    }

    fields
}

fn scalar(raw: &str) -> (Value, Option<String>) {
    let trimmed = raw.trim();
    if matches!(trimmed, "|" | ">") {
        return (
            Value::String(trimmed.to_string()),
            Some("frontmatter block scalar syntax is not supported".to_string()),
        );
    }
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
        "[]" => (Value::Array(Vec::new()), None),
        _ => trimmed.parse::<u64>().map_or_else(
            |_| (Value::String(trimmed.to_string()), None),
            |n| (json!(n), None),
        ),
    }
}

fn normalize_value(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(collapse_inline_whitespace(&text)),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_value).collect()),
        value => value,
    }
}

fn validate_fields(fields: &Map<String, Value>, errors: &mut Vec<ErrorRecord>) {
    for spec in FIELD_SPECS {
        validate_field(spec, fields, errors);
    }
    validate_request_kind(fields, errors);
    validate_sequence(fields, errors);
}

fn validate_field(spec: &FieldSpec, fields: &Map<String, Value>, errors: &mut Vec<ErrorRecord>) {
    match fields.get(spec.key) {
        None | Some(Value::Null) => {
            if spec.required {
                errors.push(error(
                    "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FIELD_REQUIRED",
                    "missing_field",
                    format!("$.markdown.frontmatter.{}", spec.key),
                    format!("request field {} is required", spec.key),
                ));
            }
        }
        Some(value) => validate_field_value(spec, value, errors),
    }
}

fn validate_field_value(spec: &FieldSpec, value: &Value, errors: &mut Vec<ErrorRecord>) {
    let valid = match spec.kind {
        FieldKind::String => value.as_str().is_some_and(|text| !text.trim().is_empty()),
        FieldKind::StringArray => value
            .as_array()
            .is_some_and(|items| items.iter().all(|item| item.as_str().is_some())),
        FieldKind::PositiveInteger => value.as_u64().is_some_and(|n| n > 0),
    };
    if !valid {
        errors.push(error(
            "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FIELD_INVALID",
            "invalid_field_value",
            format!("$.markdown.frontmatter.{}", spec.key),
            format!(
                "request field {} must be {}",
                spec.key,
                field_kind_name(spec.kind)
            ),
        ));
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
        "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_UNSUPPORTED_SHAPE",
        "unsupported_request_shape",
        "$.markdown.frontmatter.request_kind",
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
            "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FIELD_INVALID",
            "invalid_field_value",
            "$.markdown.frontmatter.sequence_index",
            "sequence_index must be less than or equal to sequence_total",
        ));
    }
}

fn field_kind_name(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::String => "a non-empty string",
        FieldKind::StringArray => "an array of strings",
        FieldKind::PositiveInteger => "a positive integer",
    }
}

fn parse_sections(
    body: &str,
    errors: &mut Vec<ErrorRecord>,
) -> BTreeMap<&'static str, Vec<String>> {
    let mut sections = BTreeMap::new();
    let mut current: Option<&'static str> = None;
    let mut saw_section = false;

    for (line_index, line) in body.lines().enumerate() {
        let line_number = line_index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() && current.is_none() {
            continue;
        }
        if let Some(heading) = h2_heading(trimmed) {
            saw_section = true;
            if let Some(canonical) = canonical_section_name(heading) {
                if sections.contains_key(canonical) {
                    errors.push(error(
                        "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_SECTION_UNSUPPORTED",
                        "unsupported_section_structure",
                        format!("$.markdown.sections.{canonical}"),
                        format!("section {canonical:?} appears more than once"),
                    ));
                    current = None;
                } else {
                    sections.insert(canonical, Vec::new());
                    current = Some(canonical);
                }
            } else {
                errors.push(error(
                    "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_SECTION_UNSUPPORTED",
                    "unsupported_section_structure",
                    format!("$.markdown.sections.line{line_number}"),
                    format!("unsupported section heading {heading:?}"),
                ));
                current = None;
            }
            continue;
        }
        if trimmed.starts_with("# ") && !saw_section {
            continue;
        }
        if trimmed.starts_with('#') {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_SECTION_UNSUPPORTED",
                "unsupported_section_structure",
                format!("$.markdown.sections.line{line_number}"),
                "nested or non-H2 headings are not supported in request sections",
            ));
            current = None;
            continue;
        }
        if let Some(heading) = current {
            sections
                .get_mut(heading)
                .expect("current section exists")
                .push(line.to_string());
        } else if !trimmed.is_empty() {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_SECTION_UNSUPPORTED",
                "unsupported_section_structure",
                format!("$.markdown.sections.line{line_number}"),
                "request body text must appear inside supported H2 sections",
            ));
        }
    }

    sections
}

fn h2_heading(trimmed: &str) -> Option<&str> {
    if trimmed.starts_with("###") {
        return None;
    }
    trimmed.strip_prefix("## ").map(str::trim)
}

fn canonical_section_name(heading: &str) -> Option<&'static str> {
    CANONICAL_SECTION_ORDER
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(heading))
        .copied()
}

fn canonical_sections(
    raw_sections: &BTreeMap<&'static str, Vec<String>>,
    errors: &mut Vec<ErrorRecord>,
) -> (Vec<CanonicalSection>, Vec<Requirement>) {
    for heading in REQUIRED_SECTIONS {
        if !raw_sections.contains_key(heading) {
            errors.push(error(
                "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_SECTION_REQUIRED",
                "missing_section",
                format!("$.markdown.sections.{heading}"),
                format!("section {heading:?} is required"),
            ));
        }
    }

    let mut canonical = Vec::new();
    let mut requirements = Vec::new();
    for heading in CANONICAL_SECTION_ORDER {
        let Some(lines) = raw_sections.get(heading) else {
            continue;
        };
        let body = if *heading == "Acceptance criteria" {
            requirements = normalize_requirements(lines, errors);
            requirement_lines(&requirements).join("\n")
        } else {
            normalize_section_body(lines)
        };
        canonical.push(CanonicalSection { heading, body });
    }

    (canonical, requirements)
}

fn normalize_requirements(lines: &[String], errors: &mut Vec<ErrorRecord>) -> Vec<Requirement> {
    let mut requirements = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        let line_number = line_index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(rest) = bullet_body(trimmed) else {
            errors.push(checklist_error(
                line_number,
                "acceptance criteria entries must be top-level Markdown bullets",
            ));
            continue;
        };
        let Some((checked, text)) = checklist_or_plain_requirement(rest) else {
            errors.push(checklist_error(
                line_number,
                "checklist marker must be [ ], [x], or [X] followed by requirement text",
            ));
            continue;
        };
        let text = collapse_inline_whitespace(text);
        if text.is_empty() {
            errors.push(checklist_error(
                line_number,
                "requirement text must not be empty",
            ));
            continue;
        }
        requirements.push(Requirement { checked, text });
    }

    requirements.sort_by(|left, right| {
        left.text
            .cmp(&right.text)
            .then_with(|| left.checked.cmp(&right.checked))
    });
    requirements
}

fn checklist_or_plain_requirement(rest: &str) -> Option<(bool, &str)> {
    let trimmed = rest.trim();
    if !trimmed.starts_with('[') {
        return Some((false, trimmed));
    }
    if let Some(text) = trimmed.strip_prefix("[ ] ") {
        return Some((false, text));
    }
    if let Some(text) = trimmed
        .strip_prefix("[x] ")
        .or_else(|| trimmed.strip_prefix("[X] "))
    {
        return Some((true, text));
    }
    None
}

fn checklist_error(line_number: usize, message: impl Into<String>) -> ErrorRecord {
    error(
        "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_CHECKLIST_MALFORMED",
        "malformed_checklist",
        format!("$.markdown.sections.Acceptance criteria.line{line_number}"),
        message,
    )
}

fn normalize_section_body(lines: &[String]) -> String {
    let mut out = Vec::new();
    let mut paragraph = Vec::new();

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush_paragraph(&mut out, &mut paragraph);
            continue;
        }
        if let Some(rest) = bullet_body(trimmed) {
            flush_paragraph(&mut out, &mut paragraph);
            out.push(format!("- {}", collapse_inline_whitespace(rest)));
        } else {
            paragraph.push(collapse_inline_whitespace(trimmed));
        }
    }
    flush_paragraph(&mut out, &mut paragraph);

    out.join("\n")
}

fn flush_paragraph(out: &mut Vec<String>, paragraph: &mut Vec<String>) {
    if paragraph.is_empty() {
        return;
    }
    out.push(paragraph.join(" "));
    paragraph.clear();
}

fn bullet_body(trimmed: &str) -> Option<&str> {
    trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
        .map(str::trim)
}

fn requirement_lines(requirements: &[Requirement]) -> Vec<String> {
    requirements
        .iter()
        .map(|requirement| {
            let marker = if requirement.checked { "[x]" } else { "[ ]" };
            format!("- {marker} {}", requirement.text)
        })
        .collect()
}

fn compact(parsed: Option<ParsedDocument>, mut errors: Vec<ErrorRecord>) -> Value {
    errors.sort_by(|left, right| {
        left.code
            .cmp(right.code)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.message.cmp(&right.message))
    });
    let valid = errors.is_empty();
    let errors_json: Vec<Value> = errors.iter().map(ErrorRecord::value).collect();
    let projection = parsed.as_ref().filter(|_| valid).map(projection);
    let request_slug = projection
        .as_ref()
        .and_then(|value| value.get("request_slug"))
        .cloned();
    let slug_metadata = projection
        .as_ref()
        .and_then(|value| value.get("slug_metadata"))
        .cloned();
    let content_hashes = projection.as_ref().map(content_hashes);

    sort_json_keys(json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "normalizer_version": NORMALIZER_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "valid": valid,
        "serialization": serialization_contract(),
        "request_slug": request_slug,
        "slug_metadata": slug_metadata,
        "projection": projection,
        "content_hashes": content_hashes,
        "error_count": errors_json.len(),
        "errors": errors_json,
    }))
}

fn projection(parsed: &ParsedDocument) -> Value {
    let projection_fields = projection_fields(&parsed.fields);
    let slug = request_slug(&projection_fields);
    let slug_metadata = slug_metadata(&projection_fields, &slug);
    let sections = sections_json(&parsed.sections);
    let requirements = requirements_json(&parsed.requirements);
    let normalized_markdown = normalized_markdown(&projection_fields, &parsed.sections);
    let project_key = projection_fields
        .get("project_key")
        .and_then(Value::as_str)
        .unwrap_or("");

    json!({
        "schema_version": PROJECTION_SCHEMA_VERSION,
        "field_order": CANONICAL_FRONTMATTER_ORDER,
        "section_order": CANONICAL_SECTION_ORDER,
        "request_slug": slug,
        "slug_metadata": slug_metadata,
        "fields": projection_fields,
        "sections": sections,
        "requirements": requirements,
        "normalized_markdown": normalized_markdown,
        "local_runner": {
            "schema_version": "request-markdown-normalizer-local-runner.v0",
            "request_id": format!("agentmesh-request://{project_key}/{slug}"),
            "request_slug": slug,
            "project_key": project_key,
            "uses_multica_fields": false,
        },
    })
}

fn projection_fields(fields: &Map<String, Value>) -> Value {
    let mut projected = Map::new();
    for key in CANONICAL_FRONTMATTER_ORDER {
        projected.insert(
            (*key).to_string(),
            fields.get(*key).cloned().unwrap_or(Value::Null),
        );
    }
    Value::Object(projected)
}

fn sections_json(sections: &[CanonicalSection]) -> Value {
    Value::Array(
        sections
            .iter()
            .map(|section| {
                json!({
                    "heading": section.heading,
                    "body": section.body,
                })
            })
            .collect(),
    )
}

fn requirements_json(requirements: &[Requirement]) -> Value {
    Value::Array(
        requirements
            .iter()
            .enumerate()
            .map(|(index, requirement)| {
                json!({
                    "index": index + 1,
                    "checked": requirement.checked,
                    "text": requirement.text,
                    "normalized_line": requirement_lines(std::slice::from_ref(requirement))[0],
                })
            })
            .collect(),
    )
}

fn normalized_markdown(fields: &Value, sections: &[CanonicalSection]) -> String {
    let mut out = String::new();
    writeln!(out, "---").expect("write markdown");
    let object = fields.as_object().expect("projection fields object");
    for key in CANONICAL_FRONTMATTER_ORDER {
        let value = object.get(*key).unwrap_or(&Value::Null);
        writeln!(out, "{key}: {}", frontmatter_value(value)).expect("write markdown");
    }
    writeln!(out, "---").expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(
        out,
        "# {}",
        object.get("title").and_then(Value::as_str).unwrap_or("")
    )
    .expect("write markdown");
    for section in sections {
        writeln!(out).expect("write markdown");
        writeln!(out, "## {}", section.heading).expect("write markdown");
        writeln!(out).expect("write markdown");
        if section.body.is_empty() {
            writeln!(out, "- none").expect("write markdown");
        } else {
            writeln!(out, "{}", section.body).expect("write markdown");
        }
    }
    out
}

fn frontmatter_value(value: &Value) -> String {
    match value {
        Value::String(text) => serde_json::to_string(text).expect("serialize frontmatter string"),
        Value::Array(_) => serde_json::to_string(value).expect("serialize frontmatter array"),
        Value::Bool(_) | Value::Number(_) | Value::Null => {
            serde_json::to_string(value).expect("serialize frontmatter value")
        }
        Value::Object(_) => serde_json::to_string(value).expect("serialize frontmatter object"),
    }
}

fn request_slug(fields: &Value) -> String {
    let title = fields
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("untitled request");
    slugify(title)
}

fn slug_metadata(fields: &Value, slug: &str) -> Value {
    let title = fields.get("title").and_then(Value::as_str).unwrap_or("");
    json!({
        "source_field": "title",
        "source_value": title,
        "algorithm": "lowercase-ascii-alnum-dash-collapse-trim-80",
        "max_length": SLUG_MAX_CHARS,
        "fallback": "untitled-request",
        "slug": slug,
    })
}

fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
        if slug.len() >= SLUG_MAX_CHARS {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "untitled-request".to_string()
    } else {
        slug
    }
}

fn content_hashes(projection: &Value) -> Value {
    let normalized_markdown = projection
        .get("normalized_markdown")
        .and_then(Value::as_str)
        .unwrap_or("");
    json!({
        "algorithm": "sha256",
        "projection_sha256": sha256_prefixed(&canonical_json_bytes(projection)),
        "normalized_markdown_sha256": sha256_prefixed(normalized_markdown.as_bytes()),
    })
}

fn serialization_contract() -> Value {
    json!({
        "format": "json_markdown_projection",
        "object_key_order": "lexicographic",
        "frontmatter_field_order": CANONICAL_FRONTMATTER_ORDER,
        "section_order": CANONICAL_SECTION_ORDER,
        "required_sections": REQUIRED_SECTIONS,
        "bullet_style": "dash-space; acceptance requirements render as - [ ] or - [x]",
        "whitespace": "LF line endings; trim line edges; collapse inline whitespace; one canonical heading block",
    })
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

fn remediation_hint(code: &str) -> &'static str {
    match code {
        "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_INPUT_INVALID" => {
            "Match request-markdown-normalizer-input.v0 and provide UTF-8 Markdown below the byte limit."
        }
        "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_UNSUPPORTED_SHAPE" => {
            "Provide one Markdown request document; JSON request objects and unknown section shapes are intentionally not normalized."
        }
        "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FRONTMATTER_MALFORMED" => {
            "Use a complete YAML frontmatter fence with single-line key: value entries and JSON arrays."
        }
        "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FIELD_REQUIRED" => {
            "Add the missing request field before normalizing the Markdown projection."
        }
        "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FIELD_INVALID" => {
            "Use the documented scalar type for the frontmatter field and rerun normalization."
        }
        "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_SECTION_REQUIRED" => {
            "Add the required H2 request section so the projection can be ordered deterministically."
        }
        "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_SECTION_UNSUPPORTED" => {
            "Use only the supported H2 request sections; remove nested headings or adapter-owned body structure."
        }
        "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_CHECKLIST_MALFORMED" => {
            "Write acceptance criteria as top-level bullets with optional [ ]/[x] checklist markers and non-empty text."
        }
        _ => "Use the normalized category, path, and message to repair the request source, then rerun.",
    }
}

fn collapse_inline_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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

    fn valid_markdown() -> String {
        "---\ntitle: \"Add a Markdown request normalizer App\"\nrequest_kind: app\nissue_type: AFK\nready_for_multica: true\nstatus: ready\nproject_key: agentmesh-private\nsource_prd: \"synthetic://requests/request-markdown-normalizer\"\nsource_design: synthetic://docs/agentmesh-request-operations-v1\nsource_roadmap: synthetic://roadmaps/agentmesh-private\nblocked_by: []\nunblocks: []\nsequence_index: 1\nsequence_total: 1\n---\n# Add a Markdown request normalizer App\n\n## Notes\nNormalize outside Multica.\n\n## What to build\nBuild a deterministic normalizer preview.\n\n## Acceptance criteria\n- [ ] Emit stable projection payloads.\n- [ ] Sort canonical requirements.\n\n## Blocked by\n- None.\n\n## User stories covered\n- As a local runner maintainer, I can diff stable output.\n"
            .into()
    }

    fn equivalent_markdown() -> String {
        "---\nsequence_total: 1\nsequence_index: 1\nunblocks: []\nblocked_by: []\nsource_roadmap: synthetic://roadmaps/agentmesh-private\nsource_design: synthetic://docs/agentmesh-request-operations-v1\nsource_prd: synthetic://requests/request-markdown-normalizer\nproject_key: agentmesh-private\nstatus: ready\nready_for_multica: true\nissue_type: AFK\nrequest_kind: app\ntitle: Add   a Markdown request normalizer App\n---\r\n# Add a Markdown request normalizer App\r\n\r\n## Acceptance criteria\r\n* Sort canonical requirements.\r\n* [ ] Emit stable projection payloads.\r\n\r\n## User stories covered\r\n* As a local runner maintainer, I can diff stable output.\r\n\r\n## Blocked by\r\n* None.\r\n\r\n## What to build\r\nBuild   a deterministic normalizer preview.\r\n\r\n## Notes\r\nNormalize outside Multica.\r\n"
            .into()
    }

    #[test]
    fn valid_markdown_produces_projection_and_slug() {
        let output = normalize_request_markdown(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request_schema_version": REQUEST_SCHEMA_VERSION,
            "markdown": valid_markdown()
        }));
        assert_eq!(output["valid"], true);
        assert_eq!(
            output["request_slug"],
            "add-a-markdown-request-normalizer-app"
        );
        assert_eq!(
            output["projection"]["requirements"][0]["text"],
            "Emit stable projection payloads."
        );
        assert_eq!(
            output["projection"]["requirements"][1]["text"],
            "Sort canonical requirements."
        );
        assert_eq!(
            output["projection"]["local_runner"]["uses_multica_fields"],
            false
        );
    }

    #[test]
    fn semantically_equivalent_markdown_is_byte_identical() {
        let left = normalize_request_markdown(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request_schema_version": REQUEST_SCHEMA_VERSION,
            "markdown": valid_markdown()
        }));
        let right = normalize_request_markdown(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request_schema_version": REQUEST_SCHEMA_VERSION,
            "markdown": equivalent_markdown()
        }));
        assert_eq!(left, right);
        assert_eq!(
            serde_json::to_vec(&left).unwrap(),
            serde_json::to_vec(&right).unwrap()
        );
    }

    #[test]
    fn unsupported_shape_and_bad_checklist_have_normalized_errors() {
        let output = normalize_request_markdown(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "request": {"title": "Shape"}
        }));
        assert_eq!(output["valid"], false);
        assert_eq!(
            output["errors"][0]["code"],
            "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_UNSUPPORTED_SHAPE"
        );
        assert!(output["errors"][0]["remediation_hint"]
            .as_str()
            .unwrap()
            .contains("Markdown request document"));

        let output = normalize_request_markdown(&json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "markdown": "---\ntitle: Bad\nrequest_kind: app\nissue_type: AFK\nstatus: ready\nproject_key: agentmesh-private\nsource_prd: Requests/App/bad.md\nsource_design: Docs/design.md\nsource_roadmap: ROADMAP.md\nblocked_by: []\nunblocks: []\nsequence_index: 1\nsequence_total: 1\n---\n# Bad\n\n## What to build\nBuild it.\n\n## Acceptance criteria\n- [todo] Not valid.\n\n## Blocked by\n- None.\n\n## User stories covered\n- As a maintainer.\n\n## Notes\n- none\n"
        }));
        assert_eq!(output["valid"], false);
        assert_eq!(
            output["errors"][0]["code"],
            "AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_CHECKLIST_MALFORMED"
        );
    }

    #[test]
    fn recorded_fixtures_match_expected_payloads() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "request_markdown_normalizer_success_input.json",
                "expected_request_markdown_normalizer_success_payload.json",
            ),
            (
                "request_markdown_normalizer_equivalent_input.json",
                "expected_request_markdown_normalizer_success_payload.json",
            ),
            (
                "request_markdown_normalizer_malformed_input.json",
                "expected_request_markdown_normalizer_malformed_payload.json",
            ),
        ] {
            let input: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            assert_eq!(
                normalize_request_markdown(&input),
                expected,
                "{input_name} should match {expected_name}"
            );
        }
    }
}
