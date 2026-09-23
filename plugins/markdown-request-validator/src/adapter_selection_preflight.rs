//! Deterministic, tool-neutral adapter capability selection preflight.
use serde_json::{json, Map, Value};

pub const VERSION: &str = "adapter-selection-preflight.v0";
const INPUT: &str = "adapter-selection-preflight-input.v0";
const OUTPUT: &str = "adapter-selection-preflight-compact.v0";

/// Select an explicitly supplied adapter without discovery or execution.
pub fn select_adapter(value: &Value) -> Value {
    let mut diagnostics = Vec::new();
    let Some(root) = value.as_object() else {
        return output(
            Vec::new(),
            None,
            vec![diagnostic(
                "input_schema_invalid",
                "$",
                "input must be an object",
            )],
        );
    };
    if root.get("schema_version").and_then(Value::as_str) != Some(INPUT) {
        diagnostics.push(diagnostic(
            "unknown_schema_version",
            "$.schema_version",
            "schema_version is not supported",
        ));
        return output(Vec::new(), None, diagnostics);
    }
    let requirements = match root.get("requirements") {
        Some(Value::Array(items)) if !items.is_empty() => items,
        Some(Value::Array(_)) => {
            diagnostics.push(diagnostic(
                "unsupported_requirements",
                "$.requirements",
                "requirements must not be empty",
            ));
            return output(Vec::new(), None, diagnostics);
        }
        Some(_) => {
            diagnostics.push(diagnostic(
                "input_schema_invalid",
                "$.requirements",
                "requirements must be an array",
            ));
            return output(Vec::new(), None, diagnostics);
        }
        None => {
            diagnostics.push(diagnostic(
                "missing_field",
                "$.requirements",
                "requirements is required",
            ));
            return output(Vec::new(), None, diagnostics);
        }
    };
    let candidates = match root.get("candidates") {
        Some(Value::Array(items)) => items,
        Some(_) => {
            diagnostics.push(diagnostic(
                "input_schema_invalid",
                "$.candidates",
                "candidates must be an array",
            ));
            return output(Vec::new(), None, diagnostics);
        }
        None => {
            diagnostics.push(diagnostic(
                "missing_field",
                "$.candidates",
                "candidates is required",
            ));
            return output(Vec::new(), None, diagnostics);
        }
    };
    let mut required = Vec::new();
    let mut malformed_requirement = false;
    for (i, item) in requirements.iter().enumerate() {
        let Some(obj) = item.as_object() else {
            diagnostics.push(diagnostic(
                "malformed_requirement",
                &format!("$.requirements[{i}]"),
                "requirement must be an object",
            ));
            malformed_requirement = true;
            continue;
        };
        let Some(name) = nonempty(obj, "name") else {
            diagnostics.push(diagnostic(
                "malformed_requirement",
                &format!("$.requirements[{i}].name"),
                "requirement name is required",
            ));
            malformed_requirement = true;
            continue;
        };
        let Some(version) = nonempty(obj, "version") else {
            diagnostics.push(diagnostic(
                "malformed_requirement",
                &format!("$.requirements[{i}].version"),
                "requirement version is required",
            ));
            malformed_requirement = true;
            continue;
        };
        required.push((name.to_string(), version.to_string()));
    }
    if malformed_requirement {
        diagnostics.sort_by_key(|item| item.to_string());
        return output(Vec::new(), None, diagnostics);
    }
    let mut eligible = Vec::new();
    for (i, item) in candidates.iter().enumerate() {
        let path = format!("$.candidates[{i}]");
        let Some(obj) = item.as_object() else {
            diagnostics.push(diagnostic(
                "malformed_manifest",
                &path,
                "manifest must be an object",
            ));
            continue;
        };
        let Some(id) = nonempty(obj, "adapter_id") else {
            diagnostics.push(diagnostic(
                "malformed_manifest",
                &format!("{path}.adapter_id"),
                "adapter_id is required",
            ));
            continue;
        };
        let Some(version) = nonempty(obj, "adapter_version") else {
            diagnostics.push(diagnostic(
                "malformed_manifest",
                &format!("{path}.adapter_version"),
                "adapter_version is required",
            ));
            continue;
        };
        let priority = match obj.get("priority").and_then(Value::as_i64) {
            Some(p) => p,
            None => {
                diagnostics.push(diagnostic(
                    "malformed_manifest",
                    &format!("{path}.priority"),
                    "priority must be an integer",
                ));
                continue;
            }
        };
        let Some(capabilities) = obj.get("capabilities").and_then(Value::as_array) else {
            diagnostics.push(diagnostic(
                "malformed_manifest",
                &format!("{path}.capabilities"),
                "capabilities must be an array",
            ));
            continue;
        };
        let mut offered = Vec::new();
        let mut malformed = false;
        for (j, cap) in capabilities.iter().enumerate() {
            let Some(c) = cap.as_object() else {
                diagnostics.push(diagnostic(
                    "malformed_manifest",
                    &format!("{path}.capabilities[{j}]"),
                    "capability must be an object",
                ));
                malformed = true;
                continue;
            };
            match (nonempty(c, "name"), nonempty(c, "version")) {
                (Some(n), Some(v)) => offered.push((n.to_string(), v.to_string())),
                _ => {
                    diagnostics.push(diagnostic(
                        "malformed_manifest",
                        &format!("{path}.capabilities[{j}]"),
                        "capability name and version are required",
                    ));
                    malformed = true;
                }
            }
        }
        if malformed {
            continue;
        }
        for field in ["common", "adapter_specific"] {
            if let Some(value) = obj.get(field) {
                if !value.is_object() {
                    diagnostics.push(diagnostic(
                        "malformed_manifest",
                        &format!("{path}.{field}"),
                        &format!("{field} must be an object"),
                    ));
                    malformed = true;
                }
            }
        }
        if malformed {
            continue;
        }
        let compatible = required.iter().all(|(name, wanted)| {
            offered
                .iter()
                .any(|(got, version)| got == name && compatible_version(wanted, version))
        });
        if compatible {
            eligible.push(json!({
                "adapter_id": id,
                "adapter_version": version,
                "priority": priority,
                "common": canonical_object(obj.get("common").cloned().unwrap_or_else(|| json!({}))),
                "adapter_specific": canonical_object(obj.get("adapter_specific").cloned().unwrap_or_else(|| json!({})))
            }));
        } else {
            diagnostics.push(diagnostic(
                "incompatible_capabilities",
                &path,
                "manifest does not satisfy all requirements",
            ));
        }
    }
    eligible.sort_by(|a, b| {
        b["priority"]
            .as_i64()
            .cmp(&a["priority"].as_i64())
            .then_with(|| a["adapter_id"].as_str().cmp(&b["adapter_id"].as_str()))
    });
    diagnostics.sort_by_key(|item| item.to_string());
    let selected = if eligible.len() > 1 && eligible[0]["priority"] == eligible[1]["priority"] {
        diagnostics.push(diagnostic(
            "selection_tie",
            "$.candidates",
            "eligible candidates share the highest priority",
        ));
        None
    } else {
        eligible.first().cloned()
    };
    if selected.is_none() && eligible.is_empty() && diagnostics.is_empty() {
        diagnostics.push(diagnostic(
            "no_eligible_adapter",
            "$.candidates",
            "no candidate satisfies the requirements",
        ));
    }
    output(eligible, selected, diagnostics)
}

fn compatible_version(wanted: &str, offered: &str) -> bool {
    if wanted == offered {
        return true;
    }
    let (op, raw) = if let Some(v) = wanted.strip_prefix(">=") {
        (">=", v)
    } else if let Some(v) = wanted.strip_prefix('^') {
        ("^", v)
    } else {
        ("", wanted)
    };
    let parse = |s: &str| {
        s.split('.')
            .map(|x| x.parse::<u64>().ok())
            .collect::<Option<Vec<_>>>()
    };
    let (Some(w), Some(o)) = (parse(raw), parse(offered)) else {
        return false;
    };
    match op {
        ">=" => o >= w,
        "^" => o.first() == w.first() && o >= w,
        _ => false,
    }
}
fn canonical_object(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonical_object(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical_object).collect()),
        other => other,
    }
}

fn nonempty<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}
fn diagnostic(code: &str, path: &str, message: &str) -> Value {
    json!({"code": code, "path": path, "message": message})
}
fn output(eligible: Vec<Value>, selected: Option<Value>, diagnostics: Vec<Value>) -> Value {
    json!({"schema_version": OUTPUT, "app_version": VERSION, "eligible_candidates": eligible, "selected_adapter": selected, "selection": if selected.is_some() { "selected" } else { "no_selection" }, "diagnostics": diagnostics})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> Value {
        json!({"schema_version":INPUT,"requirements":[{"name":"read","version":"1.0.0"}],"candidates":[{"adapter_id":"b","adapter_version":"1.0.0","priority":1,"capabilities":[{"name":"read","version":"1.0.0"}]},{"adapter_id":"a","adapter_version":"1.0.0","priority":2,"capabilities":[{"name":"read","version":"1.0.0"}]}]})
    }
    #[test]
    fn chooses_highest_priority() {
        assert_eq!(
            select_adapter(&input())["selected_adapter"]["adapter_id"],
            "a"
        );
    }
    #[test]
    fn tie_is_no_selection() {
        let mut v = input();
        v["candidates"][1]["priority"] = json!(1);
        assert_eq!(select_adapter(&v)["selection"], "no_selection");
    }
    #[test]
    fn stable_bytes() {
        assert_eq!(select_adapter(&input()), select_adapter(&input()));
    }

    #[test]
    fn unsupported_schema_version_cannot_select_a_candidate() {
        let mut value = input();
        value["schema_version"] = json!("unsupported.v0");

        let result = select_adapter(&value);

        assert_eq!(result["selection"], "no_selection");
        assert!(result["eligible_candidates"].as_array().unwrap().is_empty());
        assert_eq!(result["diagnostics"][0]["code"], "unknown_schema_version");
    }

    #[test]
    fn malformed_requirement_cannot_select_a_candidate() {
        let mut value = input();
        value["requirements"][0] = json!({"name": "read"});

        let result = select_adapter(&value);

        assert_eq!(result["selection"], "no_selection");
        assert!(result["eligible_candidates"].as_array().unwrap().is_empty());
        assert!(result["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["code"] == "malformed_requirement"));
    }

    #[test]
    fn malformed_metadata_shapes_reject_candidates() {
        for field in ["common", "adapter_specific"] {
            let mut value = input();
            value["candidates"] = json!([value["candidates"][0].clone()]);
            value["candidates"][0][field] = json!("not-an-object");

            let result = select_adapter(&value);

            assert_eq!(result["selection"], "no_selection", "{field}");
            assert!(result["eligible_candidates"].as_array().unwrap().is_empty());
            assert!(
                result["diagnostics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["path"] == format!("$.candidates[0].{field}")),
                "{field}"
            );
        }
    }
}
