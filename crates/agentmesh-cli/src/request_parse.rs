//! Stable AgentMesh request parsing for CLI and adapter handoff contracts.

use serde_json::{json, Map, Value};

const INPUT_SCHEMA_VERSION: &str = "agentmesh-request-parse-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "agentmesh-request-parse-output.v0";
const REQUEST_SCHEMA_VERSION: &str = "agentmesh-request.v0";
const MAX_SOURCE_BYTES: usize = 64 * 1024;

const ACCEPTED_INPUT_SCHEMA_VERSIONS: &[&str] = &[
    INPUT_SCHEMA_VERSION,
    "non-multica-request-adapter-input.v0",
    "local-tracker-adapter-input.v0",
    "markdown-request-validator-input.v0",
];

const REQUIRED_MARKDOWN_SECTIONS: &[&str] = &["What to build", "Acceptance criteria"];

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

/// Parse a request input file and return `(payload, valid)`.
pub fn parse_request_input_bytes(bytes: &[u8]) -> (Value, bool) {
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(err) => {
            return finish(
                None,
                vec![error(
                    "invalid_schema",
                    format!("input must be valid JSON: {err}"),
                    Some("$"),
                    None,
                )],
            )
        }
    };
    parse_request_input_value(&value)
}

fn parse_request_input_value(value: &Value) -> (Value, bool) {
    let Some(object) = value.as_object() else {
        return finish(
            None,
            vec![error(
                "invalid_schema",
                "input must be a JSON object",
                Some("$"),
                None,
            )],
        );
    };

    let mut errors = Vec::new();
    if let Some(schema_version) = object.get("schema_version") {
        match schema_version.as_str() {
            Some(version) if ACCEPTED_INPUT_SCHEMA_VERSIONS.contains(&version) => {}
            Some(version) => errors.push(error(
                "invalid_schema",
                format!("schema_version {version} is not supported by {INPUT_SCHEMA_VERSION}"),
                Some("$.schema_version"),
                None,
            )),
            None => errors.push(error(
                "invalid_schema",
                "schema_version must be a string when provided",
                Some("$.schema_version"),
                None,
            )),
        }
    }

    let has_markdown = object.contains_key("markdown");
    let has_request = object.contains_key("request");
    let direct_request = !has_markdown
        && !has_request
        && (object.contains_key("title")
            || object.contains_key("request_kind")
            || object.contains_key("issue_type"));

    let fields = if has_markdown && has_request {
        errors.push(error(
            "unsupported_request_shape",
            "provide exactly one of markdown or request",
            Some("$"),
            None,
        ));
        None
    } else if has_markdown {
        match object.get("markdown").and_then(Value::as_str) {
            Some(markdown) => parse_markdown_request(markdown, &mut errors),
            None => {
                errors.push(error(
                    "invalid_schema",
                    "markdown must be a string",
                    Some("$.markdown"),
                    None,
                ));
                None
            }
        }
    } else if has_request {
        match object.get("request").and_then(Value::as_object) {
            Some(request) => Some(fields_from_object(request)),
            None => {
                errors.push(error(
                    "unsupported_request_shape",
                    "request must be a JSON object",
                    Some("$.request"),
                    None,
                ));
                None
            }
        }
    } else if direct_request {
        Some(fields_from_object(object))
    } else {
        errors.push(error(
            "unsupported_request_shape",
            "input must contain markdown, request, or direct request fields",
            Some("$"),
            None,
        ));
        None
    };

    if let Some(fields) = &fields {
        validate_fields(fields, &mut errors);
    }

    finish(fields, errors)
}

fn parse_markdown_request(markdown: &str, errors: &mut Vec<Value>) -> Option<RequestFields> {
    if markdown.len() > MAX_SOURCE_BYTES {
        errors.push(error(
            "invalid_schema",
            format!(
                "markdown is {} bytes; limit is {MAX_SOURCE_BYTES}",
                markdown.len()
            ),
            Some("$.markdown"),
            None,
        ));
    }

    let normalized = markdown.replace("\r\n", "\n");
    let Some(frontmatter) = parse_frontmatter(&normalized) else {
        errors.push(error(
            "missing_required_section",
            "YAML frontmatter block is required for markdown sources",
            Some("$.markdown"),
            Some("frontmatter"),
        ));
        return None;
    };

    for section in REQUIRED_MARKDOWN_SECTIONS {
        if !has_markdown_section(&normalized, section) {
            errors.push(error(
                "missing_required_section",
                format!("markdown section {section:?} is required"),
                Some("$.markdown"),
                Some(section),
            ));
        }
    }

    Some(fields_from_frontmatter(frontmatter))
}

fn validate_fields(fields: &RequestFields, errors: &mut Vec<Value>) {
    if fields.title.as_deref().unwrap_or("").trim().is_empty() {
        errors.push(error(
            "invalid_schema",
            "title is required",
            Some("$.canonical.title"),
            None,
        ));
    }
    if fields.request_kind.as_deref() != Some("app") {
        errors.push(error(
            "unsupported_request_shape",
            "request_kind must be app",
            Some("$.canonical.request_kind"),
            None,
        ));
    }
    if fields.issue_type.as_deref().unwrap_or("").trim().is_empty() {
        errors.push(error(
            "invalid_schema",
            "issue_type is required",
            Some("$.canonical.issue_type"),
            None,
        ));
    }
    if fields.sequence_index.is_some() != fields.sequence_total.is_some() {
        errors.push(error(
            "invalid_schema",
            "sequence_index and sequence_total must be provided together",
            Some("$.canonical.sequence"),
            None,
        ));
    }
}

fn finish(fields: Option<RequestFields>, errors: Vec<Value>) -> (Value, bool) {
    let valid = errors.is_empty();
    let payload = json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "request_schema_version": REQUEST_SCHEMA_VERSION,
        "valid": valid,
        "canonical": fields.map(canonical_payload),
        "error_count": errors.len(),
        "errors": errors,
    });
    (payload, valid)
}

fn canonical_payload(fields: RequestFields) -> Value {
    json!({
        "title": fields.title,
        "request_kind": fields.request_kind,
        "issue_type": fields.issue_type,
        "ready_for_multica": fields.ready_for_multica,
        "status": fields.status,
        "project_key": fields.project_key,
        "source_prd": fields.source_prd,
        "source_design": fields.source_design,
        "source_roadmap": fields.source_roadmap,
        "blocked_by": fields.blocked_by,
        "unblocks": fields.unblocks,
        "sequence_index": fields.sequence_index,
        "sequence_total": fields.sequence_total,
    })
}

fn error(
    code: &str,
    message: impl Into<String>,
    path: Option<&str>,
    section: Option<&str>,
) -> Value {
    let mut object = Map::new();
    object.insert("code".to_string(), Value::String(code.to_string()));
    object.insert("message".to_string(), Value::String(message.into()));
    if let Some(path) = path {
        object.insert("path".to_string(), Value::String(path.to_string()));
    }
    if let Some(section) = section {
        object.insert("section".to_string(), Value::String(section.to_string()));
    }
    Value::Object(object)
}

fn parse_frontmatter(markdown: &str) -> Option<&str> {
    let rest = markdown.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some(&rest[..end])
}

fn has_markdown_section(markdown: &str, section: &str) -> bool {
    markdown.lines().any(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("## ")
            .is_some_and(|heading| heading.trim() == section)
    })
}

fn fields_from_frontmatter(frontmatter: &str) -> RequestFields {
    let mut fields = RequestFields::default();
    for line in frontmatter.lines() {
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        set_field(&mut fields, key.trim(), scalar(raw.trim()));
    }
    fields
}

fn fields_from_object(object: &Map<String, Value>) -> RequestFields {
    let mut fields = RequestFields::default();
    for (key, value) in object {
        set_field(&mut fields, key, value.clone());
    }
    fields
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
        _ => trimmed
            .parse::<u64>()
            .map_or_else(|_| Value::String(trimmed.to_string()), |n| json!(n)),
    }
}

fn set_field(fields: &mut RequestFields, key: &str, value: Value) {
    match key {
        "title" => fields.title = value.as_str().map(ToString::to_string),
        "request_kind" => fields.request_kind = value.as_str().map(ToString::to_string),
        "issue_type" => fields.issue_type = value.as_str().map(ToString::to_string),
        "ready_for_multica" => fields.ready_for_multica = value.as_bool(),
        "status" => fields.status = value.as_str().map(ToString::to_string),
        "project_key" => fields.project_key = value.as_str().map(ToString::to_string),
        "source_prd" => fields.source_prd = value.as_str().map(ToString::to_string),
        "source_design" => fields.source_design = value.as_str().map(ToString::to_string),
        "source_roadmap" => fields.source_roadmap = value.as_str().map(ToString::to_string),
        "blocked_by" => fields.blocked_by = strings(value),
        "unblocks" => fields.unblocks = strings(value),
        "sequence_index" => fields.sequence_index = value.as_u64(),
        "sequence_total" => fields.sequence_total = value.as_u64(),
        _ => {}
    }
}

fn strings(value: Value) -> Vec<String> {
    value.as_array().map_or_else(Vec::new, |items| {
        items
            .iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect()
    })
}
