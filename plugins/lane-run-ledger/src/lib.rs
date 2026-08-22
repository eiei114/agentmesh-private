//! Tool-neutral lane-run ledger.
//!
//! Two deterministic operations over caller-provided data (pure; no filesystem
//! access in the plugin):
//!
//! - `record`: validate one execution-run record and return the exact canonical
//!   JSON line plus month bucket key the caller should append to its ledger.
//! - `classify`: join a bounded ledger array — deduplicate observation records,
//!   subtract self-reported lane runs, rank unclassified candidates by cost.

use serde::Deserialize;
use serde_json::{json, Map, Value};

/// Plugin/schema version exposed in compact output.
pub const LANE_RUN_LEDGER_VERSION: &str = "lane-run-ledger.v0";
const INPUT_SCHEMA_VERSION: &str = "lane-run-ledger-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "lane-run-ledger-output.v0";
const MAX_NOTE_CHARS: usize = 512;
const MAX_LEDGER_ITEMS: usize = 5000;
const RESULTS: &[&str] = &["done", "split_required", "blocked"];

#[derive(Debug, Deserialize)]
struct LedgerInput {
    schema_version: String,
    operation: String,
    #[serde(default)]
    record: Option<Value>,
    #[serde(default)]
    ledger: Option<Vec<Value>>,
}

fn issue(code: &str, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn ts_bucket_month(ts: &str) -> Option<String> {
    let bytes = ts.as_bytes();
    if bytes.len() < 11 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let month: u32 = ts.get(5..7)?.parse().ok()?;
    let day: u32 = ts.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(ts[..7].to_string())
}

fn record_section(record: Option<&Value>) -> Result<(String, String, String, Value), Value> {
    let record = match record {
        Some(Value::Object(map)) => map,
        Some(_) | None => {
            return Err(issue("record_missing", "record must be a JSON object"));
        }
    };
    let ts = str_field(record, "ts").ok_or_else(|| issue("ts_missing", "record.ts is required"))?;
    let bucket = ts_bucket_month(&ts)
        .ok_or_else(|| issue("ts_bad_format", "record.ts must start with YYYY-MM-DDT"))?;
    let result = str_field(record, "result")
        .ok_or_else(|| issue("result_missing", "record.result is required"))?;
    if !RESULTS.contains(&result.as_str()) {
        return Err(issue(
            "unknown_result",
            format!("record.result must be one of {RESULTS:?}"),
        ));
    }
    let note_len = optional_str_field(record, "note").map(|note| note.chars().count());
    if let Some(len) = note_len {
        if len > MAX_NOTE_CHARS {
            return Err(issue(
                "note_too_long",
                format!("record.note exceeds {MAX_NOTE_CHARS} chars"),
            ));
        }
    }
    let mut line = Map::new();
    line.insert("ts".into(), json!(ts));
    line.insert("feedback_version".into(), json!(1));
    line.insert("event_type".into(), json!("lane_run_record"));
    line.insert("result_type".into(), json!(result));
    line.insert(
        "lane".into(),
        json!(optional_str_field(record, "lane").unwrap_or_else(|| "direct".to_string())),
    );
    for key in ["scope_key", "pr_url", "session_ref"] {
        if let Some(value) = optional_str_field(record, key) {
            line.insert(key.into(), json!(value));
        }
    }
    if let Some(note) = optional_str_field(record, "note") {
        line.insert("note".into(), json!(note));
    }
    Ok((
        result.clone(),
        bucket,
        serde_json::to_string(&Value::Object(line)).unwrap_or_default(),
        json!(null),
    ))
}

fn run_record_op(record: Option<&Value>) -> Value {
    match record_section(record) {
        Ok((result, bucket_month, line, _)) => compact(
            "record",
            true,
            vec![issue_placeholder()],
            RecordFields {
                bucket_month: Some(bucket_month),
                line: Some(line),
                feedback_code: Some(format!("lane_run_record_{result}")),
            },
            ClassifyFields::default(),
        ),
        Err(err) => compact(
            "record",
            false,
            vec![err],
            RecordFields::default(),
            ClassifyFields::default(),
        ),
    }
}

fn session_keys(entry: &Map<String, Value>) -> Vec<String> {
    ["session_ref", "session_file", "session_id"]
        .iter()
        .filter_map(|key| str_field(entry, key))
        .collect()
}

fn u64_field(entry: &Map<String, Value>, key: &str) -> u64 {
    entry.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn run_classify_op(ledger: Option<Vec<Value>>) -> Value {
    let ledger = match ledger {
        Some(entries) => entries,
        None => {
            return compact(
                "classify",
                false,
                vec![issue("ledger_missing", "ledger array is required")],
                RecordFields::default(),
                ClassifyFields::default(),
            )
        }
    };
    if ledger.len() > MAX_LEDGER_ITEMS {
        return compact(
            "classify",
            false,
            vec![issue(
                "ledger_too_large",
                format!("ledger exceeds {MAX_LEDGER_ITEMS} items"),
            )],
            RecordFields::default(),
            ClassifyFields::default(),
        );
    }

    let mut observations: std::collections::HashMap<String, &Map<String, Value>> =
        std::collections::HashMap::new();
    let mut classified_keys: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entry in &ledger {
        let Some(map) = entry.as_object() else {
            continue;
        };
        if str_field(map, "event_type").as_deref() == Some("lane_run_record") {
            for key in session_keys(map) {
                classified_keys.insert(key);
            }
        } else if str_field(map, "result_type").as_deref() == Some("orphan_session") {
            if let Some(id) = session_keys(map).first() {
                match observations.get_mut(id) {
                    Some(existing) => {
                        if str_field(map, "ts") >= str_field(existing, "ts") {
                            *existing = map;
                        }
                    }
                    None => {
                        observations.insert(id.clone(), map);
                    }
                }
            }
        }
    }

    let mut candidates: Vec<Map<String, Value>> = Vec::new();
    for (id, entry) in &observations {
        let keys = session_keys(entry);
        if keys.iter().any(|key| classified_keys.contains(key)) {
            continue;
        }
        let mut candidate = Map::new();
        candidate.insert("session_id".into(), json!(id));
        candidate.insert(
            "session_ref".into(),
            entry.get("session_ref").cloned().unwrap_or(json!(null)),
        );
        candidate.insert(
            "total_tokens".into(),
            json!(u64_field(entry, "total_tokens")),
        );
        candidate.insert("suggestion".into(), json!("review_then_record_lane_event"));
        candidates.push(candidate);
    }
    candidates.sort_by(|a, b| {
        let tokens_b = b.get("total_tokens").and_then(Value::as_u64).unwrap_or(0);
        let tokens_a = a.get("total_tokens").and_then(Value::as_u64).unwrap_or(0);
        tokens_b
            .cmp(&tokens_a)
            .then_with(|| str_field(a, "session_id").cmp(&str_field(b, "session_id")))
    });

    compact(
        "classify",
        true,
        vec![issue_placeholder()],
        RecordFields::default(),
        ClassifyFields {
            observation_count: Some(observations.len()),
            self_reported_count: Some(classified_keys.len()),
            candidate_count: Some(candidates.len()),
            candidates: Some(Value::Array(
                candidates.into_iter().map(Value::Object).collect(),
            )),
        },
    )
}

/// Validate opaque plugin input and return deterministic compact JSON.
pub fn run_lane_ledger(value: &Value) -> Value {
    let input: Result<LedgerInput, _> = serde_json::from_value(value.clone());
    let Ok(input) = input else {
        return invalid_input();
    };
    if input.schema_version != INPUT_SCHEMA_VERSION {
        return invalid_input();
    }
    match input.operation.as_str() {
        "record" => run_record_op(input.record.as_ref()),
        "classify" => run_classify_op(input.ledger),
        _ => invalid_input(),
    }
}

fn invalid_input() -> Value {
    compact(
        "record",
        false,
        vec![issue(
            "input_invalid",
            "input must match lane-run-ledger-input.v0 with operation record|classify",
        )],
        RecordFields::default(),
        ClassifyFields::default(),
    )
}

#[derive(Default)]
struct RecordFields {
    bucket_month: Option<String>,
    line: Option<String>,
    feedback_code: Option<String>,
}

#[derive(Default)]
struct ClassifyFields {
    observation_count: Option<usize>,
    self_reported_count: Option<usize>,
    candidate_count: Option<usize>,
    candidates: Option<Value>,
}

fn issue_placeholder() -> Value {
    json!({ "code": "", "message": "" })
}

fn compact(
    operation: &str,
    valid: bool,
    issues: Vec<Value>,
    record_fields: RecordFields,
    classify_fields: ClassifyFields,
) -> Value {
    let issues: Vec<Value> = issues
        .into_iter()
        .filter(|entry| !entry["code"].as_str().unwrap_or("").is_empty())
        .collect();
    let count = issues.len();
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "app_version": LANE_RUN_LEDGER_VERSION,
        "operation": operation,
        "valid": valid,
        "bucket_month": record_fields.bucket_month,
        "line": record_fields.line,
        "feedback_code": record_fields.feedback_code,
        "observation_count": classify_fields.observation_count,
        "self_reported_count": classify_fields.self_reported_count,
        "candidate_count": classify_fields.candidate_count,
        "candidates": classify_fields.candidates.unwrap_or_else(|| json!([])),
        "issue_count": count,
        "issues": issues,
    })
}

fn str_field(map: &Map<String, Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    }
}

fn optional_str_field(map: &Map<String, Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_month_parses_valid_ts() {
        assert_eq!(
            ts_bucket_month("2026-08-22T10:00:00+09:00"),
            Some("2026-08".to_string())
        );
        assert_eq!(ts_bucket_month("2026-13-01T00:00:00Z"), None);
        assert_eq!(ts_bucket_month("not-a-ts"), None);
    }

    #[test]
    fn record_rejects_unknown_result() {
        let input = json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": "record",
            "record": {"ts": "2026-08-22T10:00:00+09:00", "result": "shipped"}
        });
        let output = run_lane_ledger(&input);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["issues"][0]["code"], json!("unknown_result"));
    }

    #[test]
    fn record_line_uses_stable_key_order() {
        let input = json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": "record",
            "record": {"ts": "2026-08-22T10:00:00+09:00", "result": "done"}
        });
        let output = run_lane_ledger(&input);
        assert_eq!(
            output["line"],
            json!("{\"event_type\":\"lane_run_record\",\"feedback_version\":1,\"lane\":\"direct\",\"result_type\":\"done\",\"ts\":\"2026-08-22T10:00:00+09:00\"}")
        );
    }

    #[test]
    fn classify_excludes_self_reported_and_ranks_by_tokens() {
        let input = json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": "classify",
            "ledger": [
                {"ts": "2026-08-20T01:00:00+09:00", "result_type": "orphan_session", "session_id": "s-reported", "session_file": "C:/sessions/s-reported.jsonl", "total_tokens": 400000},
                {"ts": "2026-08-19T01:00:00+09:00", "result_type": "orphan_session", "session_id": "s-big", "total_tokens": 900000},
                {"ts": "2026-08-21T01:00:00+09:00", "event_type": "lane_run_record", "result_type": "done", "lane": "direct", "session_ref": "C:/sessions/s-reported.jsonl"}
            ]
        });
        let output = run_lane_ledger(&input);
        assert_eq!(output["valid"], json!(true));
        assert_eq!(output["observation_count"], json!(2));
        assert_eq!(output["self_reported_count"], json!(1));
        assert_eq!(output["candidate_count"], json!(1));
        assert_eq!(output["candidates"][0]["session_id"], json!("s-big"));
    }

    #[test]
    fn classify_empty_ledger_is_valid_empty() {
        let input = json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": "classify",
            "ledger": []
        });
        let output = run_lane_ledger(&input);
        assert_eq!(output["valid"], json!(true));
        assert_eq!(output["candidate_count"], json!(0));
    }
}
