//! Tool-neutral lane-run ledger.
//!
//! Two deterministic operations over caller-provided data (pure; no filesystem
//! access in the plugin):
//!
//! - `record`: validate one execution-run record and return the exact canonical
//!   JSON line plus month bucket key the caller should append to its ledger.
//! - `classify`: join a bounded ledger array — deduplicate observation records,
//!   subtract self-reported lane runs, rank unclassified candidates by cost.
//!
//! Input is validated strictly against the declared v0 envelope before
//! dispatch: unknown fields, wrong types, and out-of-bounds strings are
//! rejected with deterministic issue codes instead of being silently dropped.

use serde::Deserialize;
use serde_json::{json, Map, Value};

/// Plugin/schema version exposed in compact output.
pub const LANE_RUN_LEDGER_VERSION: &str = "lane-run-ledger.v0";
const INPUT_SCHEMA_VERSION: &str = "lane-run-ledger-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "lane-run-ledger-output.v0";
const RESULTS: &[&str] = &["done", "split_required", "blocked"];
const MAX_NOTE_CHARS: usize = 512;
const MAX_LEDGER_ITEMS: usize = 5000;
const MAX_TS_CHARS: usize = 64;
const MIN_TS_CHARS: usize = 11;
const MAX_LANE_CHARS: usize = 64;
const MAX_SCOPE_KEY_CHARS: usize = 256;
const MAX_PR_URL_CHARS: usize = 512;
const MAX_SESSION_REF_CHARS: usize = 512;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerInput {
    schema_version: String,
    operation: String,
    #[serde(default)]
    record: Option<RunRecord>,
    #[serde(default)]
    ledger: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRecord {
    ts: String,
    result: String,
    #[serde(default)]
    lane: Option<String>,
    #[serde(default)]
    scope_key: Option<String>,
    #[serde(default)]
    pr_url: Option<String>,
    #[serde(default)]
    session_ref: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

fn issue(code: &str, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn str_field(map: &Map<String, Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    }
}

/// Parse an RFC 3339 timestamp into epoch seconds, normalizing UTC offsets.
fn parse_rfc3339_epoch(ts: &str) -> Option<i64> {
    let bytes = ts.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || (bytes[10] != b'T' && bytes[10] != b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> { ts.get(range)?.parse().ok() };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let offset_seconds: i64 = match ts.get(19..)?.chars().next()? {
        'Z' | 'z' => 0,
        sign @ ('+' | '-') => {
            let offset_hours: i64 = ts.get(20..22)?.parse().ok()?;
            let offset_minutes: i64 = ts.get(23..25)?.parse().ok()?;
            if ts.as_bytes()[22] != b':' {
                return None;
            }
            let magnitude = offset_hours * 3600 + offset_minutes * 60;
            if sign == '+' {
                magnitude
            } else {
                -magnitude
            }
        }
        _ => return None,
    };
    Some(
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
            - offset_seconds,
    )
}

/// Days-from-civil (Howard Hinnant) for proleptic Gregorian dates.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = if shifted_year >= 0 {
        shifted_year
    } else {
        shifted_year - 399
    } / 400;
    let year_of_era = shifted_year - era * 400;
    let month_of_year = (month + 9) % 12;
    let day_of_year = (153 * month_of_year + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn is_newer_observation(incoming_ts: &str, existing_ts: &str) -> bool {
    match (
        parse_rfc3339_epoch(incoming_ts),
        parse_rfc3339_epoch(existing_ts),
    ) {
        (Some(incoming), Some(existing)) => incoming >= existing,
        _ => incoming_ts >= existing_ts,
    }
}

fn validate_record(record: &RunRecord) -> Vec<Value> {
    let mut issues = Vec::new();
    if record.ts.is_empty()
        || record.ts.chars().count() < MIN_TS_CHARS
        || record.ts.chars().count() > MAX_TS_CHARS
    {
        issues.push(issue(
            "record_field_invalid",
            format!("record.ts must be {MIN_TS_CHARS}..={MAX_TS_CHARS} chars"),
        ));
    }
    if !RESULTS.contains(&record.result.as_str()) {
        issues.push(issue(
            "unknown_result",
            format!("record.result must be one of {RESULTS:?}"),
        ));
    }
    let bounded_fields: [(&str, &Option<String>, usize); 5] = [
        ("lane", &record.lane, MAX_LANE_CHARS),
        ("scope_key", &record.scope_key, MAX_SCOPE_KEY_CHARS),
        ("pr_url", &record.pr_url, MAX_PR_URL_CHARS),
        ("session_ref", &record.session_ref, MAX_SESSION_REF_CHARS),
        ("note", &record.note, MAX_NOTE_CHARS),
    ];
    for (name, value, max_chars) in bounded_fields {
        if let Some(text) = value {
            let length = text.chars().count();
            if name != "note" && (text.is_empty() || length > max_chars) {
                issues.push(issue(
                    "record_field_invalid",
                    format!("record.{name} must be 1..={max_chars} chars"),
                ));
            } else if name == "note" && length > max_chars {
                issues.push(issue(
                    "record_field_invalid",
                    format!("record.{name} must be <={max_chars} chars"),
                ));
            }
        }
    }
    issues
}

fn canonical_record_line(record: &RunRecord) -> Map<String, Value> {
    let mut line = Map::new();
    line.insert("ts".into(), json!(record.ts));
    line.insert("feedback_version".into(), json!(1));
    line.insert("event_type".into(), json!("lane_run_record"));
    line.insert("result_type".into(), json!(record.result));
    line.insert(
        "lane".into(),
        json!(record
            .lane
            .clone()
            .filter(|lane| !lane.is_empty())
            .unwrap_or_else(|| "direct".to_string())),
    );
    for (field, value) in [
        ("scope_key", &record.scope_key),
        ("pr_url", &record.pr_url),
        ("session_ref", &record.session_ref),
    ] {
        if let Some(text) = value.clone().filter(|text| !text.is_empty()) {
            line.insert(field.into(), json!(text));
        }
    }
    if let Some(note) = record.note.clone().filter(|note| !note.is_empty()) {
        line.insert("note".into(), json!(note));
    }
    line
}

type RecordLineParts = (String, String, String);
type ClassifySummary = (usize, usize, usize, Vec<Map<String, Value>>);

fn compact(
    operation: &str,
    valid: bool,
    mut issues: Vec<Value>,
    record_line: Option<RecordLineParts>,
    classify: Option<ClassifySummary>,
) -> Value {
    issues.retain(|entry| !entry["code"].as_str().unwrap_or("").is_empty());
    let issue_count = issues.len();
    let (bucket_month, line, feedback_code) = match record_line {
        Some((bucket, line_text, code)) => (json!(bucket), json!(line_text), json!(code)),
        None => (json!(null), json!(null), json!(null)),
    };
    let (observation_count, self_reported_count, candidate_count, candidates) = match classify {
        Some((observed, reported, candidate_total, candidate_items)) => (
            json!(observed),
            json!(reported),
            json!(candidate_total),
            Value::Array(candidate_items.into_iter().map(Value::Object).collect()),
        ),
        None => (
            json!(null),
            json!(null),
            json!(null),
            Value::Array(Vec::new()),
        ),
    };
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "app_version": LANE_RUN_LEDGER_VERSION,
        "operation": operation,
        "valid": valid,
        "bucket_month": bucket_month,
        "line": line,
        "feedback_code": feedback_code,
        "observation_count": observation_count,
        "self_reported_count": self_reported_count,
        "candidate_count": candidate_count,
        "candidates": candidates,
        "issue_count": issue_count,
        "issues": issues,
    })
}

fn run_record_op(record: &RunRecord) -> Value {
    let bucket_month = match ts_bucket_month(&record.ts) {
        Some(bucket) => bucket,
        None => {
            return compact(
                "record",
                false,
                vec![issue(
                    "ts_bad_format",
                    "record.ts must start with YYYY-MM-DDT",
                )],
                None,
                None,
            )
        }
    };
    let line_map = canonical_record_line(record);
    let line_text = serde_json::to_string(&Value::Object(line_map)).unwrap_or_default();
    compact(
        "record",
        true,
        Vec::new(),
        Some((
            bucket_month,
            line_text,
            format!("lane_run_record_{}", record.result),
        )),
        None,
    )
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

fn session_keys(entry: &Map<String, Value>) -> Vec<String> {
    ["session_ref", "session_file", "session_id"]
        .iter()
        .filter_map(|key| str_field(entry, key))
        .collect()
}

fn u64_field(entry: &Map<String, Value>, key: &str) -> u64 {
    entry.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn observation_identity(entry: &Map<String, Value>) -> Option<String> {
    ["session_id", "session_file", "session_ref"]
        .iter()
        .find_map(|key| str_field(entry, key))
}

fn run_classify_op(ledger: &[Value]) -> Value {
    if ledger.len() > MAX_LEDGER_ITEMS {
        return compact(
            "classify",
            false,
            vec![issue(
                "ledger_too_large",
                format!("ledger exceeds {MAX_LEDGER_ITEMS} items"),
            )],
            None,
            None,
        );
    }

    let mut observations: std::collections::HashMap<String, &Map<String, Value>> =
        std::collections::HashMap::new();
    let mut classified_keys: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entry in ledger {
        let Some(map) = entry.as_object() else {
            continue;
        };
        if str_field(map, "event_type").as_deref() == Some("lane_run_record") {
            for key in session_keys(map) {
                classified_keys.insert(key);
            }
        } else if str_field(map, "result_type").as_deref() == Some("orphan_session") {
            if let Some(identity) = observation_identity(map) {
                match observations.get_mut(&identity) {
                    Some(existing) => {
                        if is_newer_observation(
                            str_field(map, "ts").as_deref().unwrap_or_default(),
                            str_field(existing, "ts").as_deref().unwrap_or_default(),
                        ) {
                            *existing = map;
                        }
                    }
                    None => {
                        observations.insert(identity, map);
                    }
                }
            }
        }
    }

    let mut candidates: Vec<Map<String, Value>> = Vec::new();
    for entry in observations.values() {
        if session_keys(entry)
            .iter()
            .any(|key| classified_keys.contains(key))
        {
            continue;
        }
        // Candidate identity comes strictly from session_id; session_ref is a
        // separate optional reference field.
        let Some(session_id) = str_field(entry, "session_id") else {
            continue;
        };
        let mut candidate = Map::new();
        candidate.insert("session_id".into(), json!(session_id));
        candidate.insert(
            "session_ref".into(),
            str_field(entry, "session_ref").map_or(json!(null), Value::String),
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
    let candidate_count = candidates.len();

    compact(
        "classify",
        true,
        Vec::new(),
        None,
        Some((
            observations.len(),
            classified_keys.len(),
            candidate_count,
            candidates,
        )),
    )
}

/// Validate opaque plugin input and return deterministic compact JSON.
pub fn run_lane_ledger(value: &Value) -> Value {
    let Ok(input) = serde_json::from_value::<LedgerInput>(value.clone()) else {
        return compact(
            "record",
            false,
            vec![issue(
                "input_invalid",
                "input must match lane-run-ledger-input.v0 with operation record|classify",
            )],
            None,
            None,
        );
    };

    let mut issues = Vec::new();
    if input.schema_version != INPUT_SCHEMA_VERSION {
        issues.push(issue(
            "input_invalid",
            format!("schema_version must be {INPUT_SCHEMA_VERSION}"),
        ));
    }
    let operation = input.operation.as_str();
    if operation != "record" && operation != "classify" {
        issues.push(issue(
            "unknown_operation",
            "operation must be record or classify",
        ));
    }
    if let Some(record) = &input.record {
        issues.extend(validate_record(record));
    }
    if let Some(ledger) = &input.ledger {
        if ledger.iter().any(|entry| !entry.is_object()) {
            issues.push(issue(
                "ledger_entry_type",
                "every ledger item must be a JSON object",
            ));
        }
    }
    if !issues.is_empty() {
        return compact(operation, false, issues, None, None);
    }

    match operation {
        "record" => run_record_op(input.record.as_ref().expect("record presence checked")),
        "classify" => run_classify_op(input.ledger.as_deref().expect("ledger presence checked")),
        _ => unreachable!("operation enum checked above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input(operation: &str) -> Value {
        json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": operation,
        })
    }

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
    fn rfc3339_instants_normalize_offsets() {
        // 2026-08-21T00:00:00+09:00 == 2026-08-20T15:00:00Z
        assert_eq!(
            parse_rfc3339_epoch("2026-08-21T00:00:00+09:00"),
            parse_rfc3339_epoch("2026-08-20T15:00:00Z")
        );
        // ...and that instant is earlier than 2026-08-20T20:00:00Z even though
        // the raw string sorts lexically after it.
        assert!(
            parse_rfc3339_epoch("2026-08-21T00:00:00+09:00")
                < parse_rfc3339_epoch("2026-08-20T20:00:00Z")
        );
    }

    #[test]
    fn record_rejects_unknown_result() {
        let mut input = base_input("record");
        input["record"] = json!({"ts": "2026-08-22T10:00:00+09:00", "result": "shipped"});
        let output = run_lane_ledger(&input);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["issues"][0]["code"], json!("unknown_result"));
    }

    #[test]
    fn record_rejects_overlong_lane() {
        let mut input = base_input("record");
        input["record"] = json!({
            "ts": "2026-08-22T10:00:00+09:00",
            "result": "done",
            "lane": "x".repeat(MAX_LANE_CHARS + 1)
        });
        let output = run_lane_ledger(&input);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["issues"][0]["code"], json!("record_field_invalid"));
    }

    #[test]
    fn input_rejects_unknown_top_level_field() {
        let mut input = base_input("record");
        input["surprise"] = json!(true);
        input["record"] = json!({"ts": "2026-08-22T10:00:00+09:00", "result": "done"});
        let output = run_lane_ledger(&input);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["issues"][0]["code"], json!("input_invalid"));
    }

    #[test]
    fn input_rejects_non_object_ledger_entry() {
        let mut input = base_input("classify");
        input["ledger"] = json!(["not-an-object"]);
        let output = run_lane_ledger(&input);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["issues"][0]["code"], json!("ledger_entry_type"));
    }

    #[test]
    fn record_line_uses_stable_key_order() {
        let mut input = base_input("record");
        input["record"] = json!({"ts": "2026-08-22T10:00:00+09:00", "result": "done"});
        let output = run_lane_ledger(&input);
        assert_eq!(
            output["line"],
            json!("{\"event_type\":\"lane_run_record\",\"feedback_version\":1,\"lane\":\"direct\",\"result_type\":\"done\",\"ts\":\"2026-08-22T10:00:00+09:00\"}")
        );
    }

    #[test]
    fn classify_excludes_self_reported_and_ranks_by_tokens() {
        let mut input = base_input("classify");
        input["ledger"] = json!([
            {"ts": "2026-08-20T01:00:00+09:00", "result_type": "orphan_session", "session_id": "s-reported", "session_file": "C:/sessions/s-reported.jsonl", "total_tokens": 400000},
            {"ts": "2026-08-19T01:00:00+09:00", "result_type": "orphan_session", "session_id": "s-big", "total_tokens": 900000},
            {"ts": "2026-08-21T01:00:00+09:00", "event_type": "lane_run_record", "result_type": "done", "lane": "direct", "session_ref": "C:/sessions/s-reported.jsonl"}
        ]);
        let output = run_lane_ledger(&input);
        assert_eq!(output["valid"], json!(true));
        assert_eq!(output["observation_count"], json!(2));
        assert_eq!(output["self_reported_count"], json!(1));
        assert_eq!(output["candidate_count"], json!(1));
        assert_eq!(output["candidates"][0]["session_id"], json!("s-big"));
    }

    #[test]
    fn classify_dedupe_compares_instants_not_strings() {
        let mut input = base_input("classify");
        input["ledger"] = json!([
            {"ts": "2026-08-21T00:00:00+09:00", "result_type": "orphan_session", "session_id": "s-offset", "total_tokens": 111},
            {"ts": "2026-08-20T20:00:00Z", "result_type": "orphan_session", "session_id": "s-offset", "total_tokens": 222}
        ]);
        let output = run_lane_ledger(&input);
        // The +09:00 entry sorts lexically later but its instant
        // (2026-08-20T15:00:00Z) is earlier than 2026-08-20T20:00:00Z,
        // so it must NOT replace the existing observation.
        assert_eq!(output["candidates"][0]["total_tokens"], json!(222));
    }

    #[test]
    fn classify_empty_ledger_is_valid_empty() {
        let mut input = base_input("classify");
        input["ledger"] = json!([]);
        let output = run_lane_ledger(&input);
        assert_eq!(output["valid"], json!(true));
        assert_eq!(output["candidate_count"], json!(0));
    }
}
