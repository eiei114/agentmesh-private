//! Shared JSON/Markdown helpers for adapter-neutral compact App modules.
//!
//! Centralizes deterministic error records, canonical JSON key ordering, and
//! Markdown rendering utilities reused across request App implementations.

use serde_json::{json, Map, Value};

/// Build a normalized adapter error record for compact JSON output.
pub(crate) fn adapter_error_record(
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

/// Collect stable error codes from normalized error records.
pub(crate) fn error_codes(errors: &[Value]) -> Vec<String> {
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

/// Recursively sort JSON object keys for deterministic serialization.
pub(crate) fn sort_json_keys(value: Value) -> Value {
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

/// Serialize JSON with lexicographically sorted object keys.
pub(crate) fn canonical_json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(&sort_json_keys(value.clone())).expect("serialize canonical json")
}

/// Compact JSON string for Markdown evidence blocks.
pub(crate) fn json_compact(value: &Value) -> String {
    serde_json::to_string(value).expect("serialize value")
}

/// Inline Markdown code span containing compact JSON.
pub(crate) fn inline_json(value: &Value) -> String {
    format!("`{}`", json_compact(value).replace('`', "\\`"))
}

/// Escape Markdown table cell content.
pub(crate) fn markdown_cell(text: impl AsRef<str>) -> String {
    text.as_ref()
        .replace('`', "\\`")
        .replace('|', "\\|")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_error_record_shape_is_stable() {
        let record = adapter_error_record(
            "AGENTMESH_INPUT_SCHEMA_INVALID",
            "request.input_schema_invalid",
            Some("$.schema_version"),
            "schema_version is required",
        );
        assert_eq!(record["severity"], "error");
        assert_eq!(record["path"], "$.schema_version");
    }

    #[test]
    fn sort_json_keys_orders_nested_objects() {
        let sorted = sort_json_keys(json!({"b": 1, "a": {"d": 2, "c": 3}}));
        assert_eq!(sorted, json!({"a": {"c": 3, "d": 2}, "b": 1}));
    }

    #[test]
    fn canonical_json_bytes_matches_sorted_key_order() {
        let bytes = canonical_json_bytes(&json!({"z": 1, "a": 2}));
        assert_eq!(String::from_utf8(bytes).unwrap(), r#"{"a":2,"z":1}"#);
    }

    #[test]
    fn error_codes_collects_codes_in_order() {
        let errors = vec![
            adapter_error_record("CODE_A", "cat", None::<String>, "one"),
            adapter_error_record("CODE_B", "cat", None::<String>, "two"),
        ];
        assert_eq!(error_codes(&errors), vec!["CODE_A", "CODE_B"]);
    }
}
