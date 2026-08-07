//! JSON Schema generation helpers for protocol-v0.md drift checks.

use crate::compact::CompactEnvelope;
use crate::rpc::{InitializeParams, InitializeResult, RunParams, RunResult};
use schemars::{
    generate::SchemaSettings, transform::ReplaceConstValue, JsonSchema, SchemaGenerator,
};
use serde_json::{Map, Value};

/// Generate the Phase 0 host-owned schemas as a single JSON object.
pub fn generate_schemas() -> Value {
    sort_json_keys(serde_json::json!({
        "protocol_version": crate::versions::PROTOCOL_VERSION,
        "initialize_params": protocol_schema::<InitializeParams>(),
        "initialize_result": protocol_schema::<InitializeResult>(),
        "run_params": protocol_schema::<RunParams>(),
        "run_result": protocol_schema::<RunResult>(),
        "compact_envelope": protocol_schema::<CompactEnvelope>(),
    }))
}

fn protocol_schema<T: JsonSchema>() -> schemars::Schema {
    SchemaGenerator::from(SchemaSettings::draft07().with_transform(ReplaceConstValue::default()))
        .into_root_schema_for::<T>()
}

fn sort_json_keys(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.into_iter().collect();
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));

            let mut sorted = Map::new();
            for (key, value) in entries {
                let value = sort_json_keys(value);
                if key == "required" {
                    sorted.insert(key, sort_required_values(value));
                } else {
                    sorted.insert(key, value);
                }
            }
            Value::Object(sorted)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(sort_json_keys).collect()),
        value => value,
    }
}

fn sort_required_values(value: Value) -> Value {
    let Value::Array(mut values) = value else {
        return value;
    };
    if values.iter().all(Value::is_string) {
        values.sort_unstable_by(|left, right| left.as_str().unwrap().cmp(right.as_str().unwrap()));
    }
    Value::Array(values)
}

/// Pretty-printed schema document bytes.
pub fn generate_schemas_pretty() -> String {
    serde_json::to_string_pretty(&generate_schemas()).expect("schema serialization")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn schema_snapshot_is_current() {
        let generated = generate_schemas_pretty();
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas/protocol-v0.schema.json");
        if path.exists() {
            let on_disk = std::fs::read_to_string(&path).expect("read schema");
            assert_eq!(
                on_disk.replace("\r\n", "\n").trim(),
                generated.trim(),
                "schemas/protocol-v0.schema.json drifted; regenerate via tests or scripts"
            );
        }
    }
}
