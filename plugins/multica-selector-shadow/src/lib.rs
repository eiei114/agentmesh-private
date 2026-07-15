//! Offline Multica backlog selector shadow adapter.
//!
//! Plugin-owned types stay here. They must never move into `agentmesh-proto`
//! or `agentmesh-host`.

use serde::Deserialize;
use serde_json::{json, Map, Value};
use thiserror::Error;

/// Named reasons used by shadow shape/value comparison failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ShadowCompareReason {
    /// Expected object root was not a JSON object.
    #[error("shadow_expected_not_object")]
    ExpectedNotObject,
    /// Actual payload root was not a JSON object.
    #[error("shadow_actual_not_object")]
    ActualNotObject,
    /// Required key present in the Python-equivalent compact shape is missing.
    #[error("shadow_missing_required_key:{0}")]
    MissingRequiredKey(String),
    /// Payload contains a key outside the compact selector contract.
    #[error("shadow_unexpected_key:{0}")]
    UnexpectedKey(String),
    /// JSON type at a path differs from the recorded expectation.
    #[error("shadow_type_mismatch:{path}:expected={expected},actual={actual}")]
    TypeMismatch {
        /// JSON path (dot / index notation).
        path: String,
        /// Expected JSON type name.
        expected: &'static str,
        /// Actual JSON type name.
        actual: &'static str,
    },
    /// Value at a path differs from the recorded expectation.
    #[error("shadow_value_mismatch:{0}")]
    ValueMismatch(String),
}

/// Errors while interpreting opaque plugin input.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SelectError {
    /// Input JSON is not an object or fails plugin-owned validation.
    #[error("plugin_input_invalid:{0}")]
    InvalidInput(String),
}

/// Opaque Multica-shaped issue listing used for offline shadow runs.
#[derive(Debug, Clone, Deserialize)]
pub struct ShadowSelectorInput {
    /// Controller identity; Phase 1.0 skeleton accepts backlog_promoter only.
    pub controller: String,
    /// Must be `shadow` for this adapter.
    pub mode: String,
    /// Deterministic clock override (ISO-8601 string).
    pub now: String,
    /// Recorded Multica issue objects (subset fields are enough for the stub).
    #[serde(default)]
    pub issues: Vec<Value>,
}

/// Required top-level keys of the Python `compact_summary` contract.
pub const COMPACT_REQUIRED_KEYS: &[&str] = &[
    "schema_version",
    "decision",
    "reason",
    "fast_exit_required",
    "recommended_next_action",
    "run_only_result",
    "run_context",
    "cap_state",
    "promotion_candidates",
    "age_boost_action_count",
    "age_boost_skipped_count",
    "replenishment_candidate",
    "skipped_summary",
    "metrics",
    "warnings",
];

/// Parse and validate plugin-owned shadow input.
pub fn parse_input(value: &Value) -> Result<ShadowSelectorInput, SelectError> {
    let input: ShadowSelectorInput = serde_json::from_value(value.clone())
        .map_err(|e| SelectError::InvalidInput(format!("deserialize: {e}")))?;
    if input.controller != "backlog_promoter" {
        return Err(SelectError::InvalidInput(format!(
            "unsupported controller: {}",
            input.controller
        )));
    }
    if input.mode != "shadow" {
        return Err(SelectError::InvalidInput(format!(
            "unsupported mode: {}",
            input.mode
        )));
    }
    if input.now.trim().is_empty() {
        return Err(SelectError::InvalidInput("now is empty".into()));
    }
    Ok(input)
}

/// Build compact payload equivalent to Python selector `compact_summary` shape.
///
/// This is a **skeleton** admission path for recorded listings only:
/// - backlog status issues become candidate rows
/// - human-owned / member-assignee issues are counted in `skipped_summary`
/// - at most 5 eligible backlog items are promoted with `selection_reason=shadow_stub_eligible`
///
/// Full parity (caps, evidence preflight, schedule admission, age boost) is deferred.
pub fn select_compact_payload(input: &ShadowSelectorInput) -> Value {
    let mut skipped_summary: Map<String, Value> = Map::new();
    let mut backlog_issues = Vec::new();
    let mut candidates = Vec::new();

    for issue in &input.issues {
        let status = issue
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if status != "backlog" {
            continue;
        }
        backlog_issues.push(issue);
        if is_human_owned(issue) {
            bump(&mut skipped_summary, "human_owned");
            continue;
        }
        candidates.push(issue);
    }

    let mut selected = Vec::new();
    for issue in candidates.iter().take(5) {
        selected.push(json!({
            "issue_id": string_field(issue, "id"),
            "issue_key": issue_key(issue),
            "project_key": string_field(issue, "project_key"),
            "title": string_field(issue, "title"),
            "selection_reason": "shadow_stub_eligible",
        }));
    }

    let chosen_action = if selected.is_empty() {
        "no_candidate"
    } else {
        "promote_backlog"
    };
    let reason = if selected.is_empty() {
        Value::String("no_candidate".into())
    } else {
        Value::Null
    };
    let recommended_next_action = if selected.is_empty() {
        "create_replenishment_or_final_report"
    } else {
        "apply_promotions"
    };
    let replenishment_candidate = if selected.is_empty() {
        replenish_candidate(&skipped_summary, candidates.len())
    } else {
        Value::Null
    };
    let candidate_issue_keys: Vec<Value> = selected
        .iter()
        .map(|item| item["issue_key"].clone())
        .collect();
    let promotion_candidate_count = selected.len();

    json!({
        "schema_version": 1,
        "decision": "continue",
        "reason": reason.clone(),
        "fast_exit_required": false,
        "recommended_next_action": recommended_next_action,
        "run_only_result": {
            "decision": "continue",
            "reason": reason,
            "selected": !selected.is_empty(),
            "promotion_candidate_count": promotion_candidate_count,
            "chosen_action": chosen_action,
            "candidate_issue_keys": candidate_issue_keys,
        },
        "run_context": {
            "now": input.now,
            "deterministic_mode": true,
        },
        "cap_state": {
            "ai_todo_cap": 30,
            "pr_producing_todo_cap": 20,
            "review_fix_exempt_todo_cap": 10,
            "normal_promotion_limit": 5,
            "special_promotion_limit": 8,
        },
        "promotion_candidates": selected,
        "age_boost_action_count": 0,
        "age_boost_skipped_count": 0,
        "replenishment_candidate": replenishment_candidate,
        "skipped_summary": skipped_summary,
        "metrics": {
            "backlog_count": backlog_issues.len(),
            "candidate_count": candidates.len(),
            "promotion_candidate_count": promotion_candidate_count,
            "chosen_action": chosen_action,
        },
        "warnings": [],
    })
}

fn replenish_candidate(skipped_summary: &Map<String, Value>, candidate_count: usize) -> Value {
    if candidate_count > 0 {
        return json!({
            "action": "none",
            "reason": "promotion_candidates_blocked_by_caps",
        });
    }
    if skipped_summary.contains_key("blocked_dependency")
        || skipped_summary.contains_key("blocked_reason")
    {
        return json!({
            "action": "create_issue",
            "priority": 1,
            "reason": "blocker_repair_or_dependency_metadata_reconciliation",
            "title_stub": "Repair blocked backlog dependency metadata",
            "status": "backlog",
        });
    }
    if skipped_summary.contains_key("issue_evidence_preflight_failed") {
        return json!({
            "action": "create_issue",
            "priority": 2,
            "reason": "preflight_evidence_repair",
            "title_stub": "Repair stale backlog evidence paths",
            "status": "backlog",
        });
    }
    json!({
        "action": "create_issue",
        "priority": 5,
        "reason": "roadmap_context_microtask",
        "title_stub": "Seed next bounded maintenance microtask",
        "status": "backlog",
    })
}

fn is_human_owned(issue: &Value) -> bool {
    let work_owner = issue
        .pointer("/metadata/work_owner")
        .and_then(Value::as_str)
        .or_else(|| {
            issue
                .get("metadata")
                .and_then(|m| m.get("work_owner"))
                .and_then(Value::as_str)
        });
    if work_owner == Some("human") {
        return true;
    }
    issue.get("assignee_type").and_then(Value::as_str) == Some("member")
}

fn issue_key(issue: &Value) -> String {
    issue
        .get("identifier")
        .and_then(Value::as_str)
        .or_else(|| issue.get("id").and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

fn string_field(issue: &Value, key: &str) -> String {
    issue
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn bump(map: &mut Map<String, Value>, key: &str) {
    let next = map.get(key).and_then(Value::as_u64).unwrap_or(0) + 1;
    map.insert(key.to_string(), json!(next));
}

/// Compare actual payload against recorded Python-equivalent compact output.
///
/// On failure, returns the first named [`ShadowCompareReason`] for reproducible diagnosis.
pub fn compare_compact_shadow(actual: &Value, expected: &Value) -> Result<(), ShadowCompareReason> {
    let actual_obj = actual
        .as_object()
        .ok_or(ShadowCompareReason::ActualNotObject)?;
    let expected_obj = expected
        .as_object()
        .ok_or(ShadowCompareReason::ExpectedNotObject)?;

    for key in COMPACT_REQUIRED_KEYS {
        if !expected_obj.contains_key(*key) {
            continue;
        }
        if !actual_obj.contains_key(*key) {
            return Err(ShadowCompareReason::MissingRequiredKey((*key).to_string()));
        }
    }
    for key in actual_obj.keys() {
        if !COMPACT_REQUIRED_KEYS.contains(&key.as_str()) {
            return Err(ShadowCompareReason::UnexpectedKey(key.clone()));
        }
    }
    compare_value(actual, expected, "$")
}

fn compare_value(actual: &Value, expected: &Value, path: &str) -> Result<(), ShadowCompareReason> {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => {
            for key in e.keys() {
                if !a.contains_key(key) {
                    return Err(ShadowCompareReason::MissingRequiredKey(format!(
                        "{path}.{key}"
                    )));
                }
            }
            for key in a.keys() {
                if !e.contains_key(key) {
                    return Err(ShadowCompareReason::UnexpectedKey(format!("{path}.{key}")));
                }
            }
            for (key, expected_child) in e {
                let actual_child = &a[key];
                let child_path = if path == "$" {
                    format!("$.{key}")
                } else {
                    format!("{path}.{key}")
                };
                compare_value(actual_child, expected_child, &child_path)?;
            }
            Ok(())
        }
        (Value::Array(a), Value::Array(e)) => {
            if a.len() != e.len() {
                return Err(ShadowCompareReason::ValueMismatch(format!(
                    "{path}.length:expected={},actual={}",
                    e.len(),
                    a.len()
                )));
            }
            for (idx, (actual_child, expected_child)) in a.iter().zip(e.iter()).enumerate() {
                compare_value(actual_child, expected_child, &format!("{path}[{idx}]"))?;
            }
            Ok(())
        }
        (Value::Null, Value::Null) => Ok(()),
        (Value::Bool(a), Value::Bool(e)) if a == e => Ok(()),
        (Value::Number(a), Value::Number(e)) if a == e => Ok(()),
        (Value::String(a), Value::String(e)) if a == e => Ok(()),
        (a, e) if type_name(a) != type_name(e) => Err(ShadowCompareReason::TypeMismatch {
            path: path.to_string(),
            expected: type_name(e),
            actual: type_name(a),
        }),
        _ => Err(ShadowCompareReason::ValueMismatch(path.to_string())),
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn empty_expected() -> Value {
        json!({
            "schema_version": 1,
            "decision": "continue",
            "reason": "no_candidate",
            "fast_exit_required": false,
            "recommended_next_action": "create_replenishment_or_final_report",
            "run_only_result": {
                "decision": "continue",
                "reason": "no_candidate",
                "selected": false,
                "promotion_candidate_count": 0,
                "chosen_action": "no_candidate",
                "candidate_issue_keys": []
            },
            "run_context": {
                "now": "2026-07-15T12:00:00+09:00",
                "deterministic_mode": true
            },
            "cap_state": {
                "ai_todo_cap": 30,
                "pr_producing_todo_cap": 20,
                "review_fix_exempt_todo_cap": 10,
                "normal_promotion_limit": 5,
                "special_promotion_limit": 8
            },
            "promotion_candidates": [],
            "age_boost_action_count": 0,
            "age_boost_skipped_count": 0,
            "replenishment_candidate": {
                "action": "create_issue",
                "priority": 5,
                "reason": "roadmap_context_microtask",
                "title_stub": "Seed next bounded maintenance microtask",
                "status": "backlog"
            },
            "skipped_summary": {},
            "metrics": {
                "backlog_count": 0,
                "candidate_count": 0,
                "promotion_candidate_count": 0,
                "chosen_action": "no_candidate"
            },
            "warnings": []
        })
    }

    #[test]
    fn empty_listing_matches_python_compact_shape() {
        let input = parse_input(&json!({
            "controller": "backlog_promoter",
            "mode": "shadow",
            "now": "2026-07-15T12:00:00+09:00",
            "issues": []
        }))
        .unwrap();
        let actual = select_compact_payload(&input);
        compare_compact_shadow(&actual, &empty_expected()).unwrap();
    }

    #[test]
    fn selects_eligible_backlog_issue() {
        let input = parse_input(&json!({
            "controller": "backlog_promoter",
            "mode": "shadow",
            "now": "2026-07-15T12:00:00+09:00",
            "issues": [{
                "id": "iss_1",
                "identifier": "AM-1",
                "project_key": "agentmesh",
                "title": "Stub promotion",
                "status": "backlog",
                "assignee_type": "agent",
                "metadata": {"work_owner": "ai"}
            }]
        }))
        .unwrap();
        let actual = select_compact_payload(&input);
        assert_eq!(actual["decision"], "continue");
        assert_eq!(actual["reason"], Value::Null);
        assert_eq!(actual["promotion_candidates"][0]["issue_key"], "AM-1");
        assert_eq!(
            actual["promotion_candidates"][0]["selection_reason"],
            "shadow_stub_eligible"
        );
        assert_eq!(actual["metrics"]["chosen_action"], "promote_backlog");
    }

    #[test]
    fn shadow_compare_names_missing_required_key() {
        let mut actual = empty_expected();
        actual.as_object_mut().unwrap().remove("decision");
        let err = compare_compact_shadow(&actual, &empty_expected()).unwrap_err();
        assert_eq!(
            err,
            ShadowCompareReason::MissingRequiredKey("decision".into())
        );
        assert_eq!(err.to_string(), "shadow_missing_required_key:decision");
    }

    #[test]
    fn shadow_compare_names_value_mismatch() {
        let mut actual = empty_expected();
        actual["decision"] = json!("stop");
        let err = compare_compact_shadow(&actual, &empty_expected()).unwrap_err();
        assert_eq!(err, ShadowCompareReason::ValueMismatch("$.decision".into()));
        assert_eq!(err.to_string(), "shadow_value_mismatch:$.decision");
    }

    #[test]
    fn rejects_non_shadow_mode() {
        let err = parse_input(&json!({
            "controller": "backlog_promoter",
            "mode": "live",
            "now": "2026-07-15T12:00:00+09:00",
            "issues": []
        }))
        .unwrap_err();
        assert!(matches!(err, SelectError::InvalidInput(_)));
    }

    #[test]
    fn recorded_fixtures_match_expected_payloads() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (input_name, expected_name) in [
            (
                "recorded_empty_backlog_input.json",
                "expected_empty_backlog_compact_payload.json",
            ),
            (
                "recorded_one_candidate_input.json",
                "expected_one_candidate_compact_payload.json",
            ),
        ] {
            let input_value: Value =
                serde_json::from_slice(&std::fs::read(root.join(input_name)).unwrap()).unwrap();
            let expected: Value =
                serde_json::from_slice(&std::fs::read(root.join(expected_name)).unwrap()).unwrap();
            let input = parse_input(&input_value).unwrap();
            let actual = select_compact_payload(&input);
            compare_compact_shadow(&actual, &expected).unwrap_or_else(|err| {
                panic!("{input_name} vs {expected_name}: {err}");
            });
        }
    }
}
