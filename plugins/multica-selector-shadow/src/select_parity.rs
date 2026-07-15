//! Full Python-parity backlog selection over snapshot v0 (Multica-free).

use chrono::{DateTime, FixedOffset, TimeZone, Timelike, Utc};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

const SNAPSHOT_SCHEMA_VERSION: &str = "backlog-promoter-snapshot.v0";
const AI_TODO_CAP: i64 = 30;
const PR_PRODUCING_TODO_CAP: i64 = 20;
const REVIEW_FIX_EXEMPT_TODO_CAP: i64 = 10;
const NORMAL_PROMOTION_LIMIT: i64 = 5;
const SPECIAL_PROMOTION_LIMIT: i64 = 8;
const AGE_BOOST_DAYS: i64 = 7;
const METADATA_KEY_LIMIT: usize = 50;
const AGE_BOOST_KEYS: &[&str] = &["age_boosted", "age_boosted_at"];
const DONE_LIKE: &[&str] = &["done", "completed", "cancelled", "canceled", "closed"];
const ACTIVE_STATUSES: &[&str] = &["todo", "in_progress", "in_review"];
const ACTIVE_AUTOPILOT_RUN: &[&str] = &["running", "queued", "in_progress", "pending"];
const TITLE_BLOCK_HINTS: &[&str] = &[
    "blocked",
    "waiting",
    "draft",
    "pending human input",
    "needs decision",
    "unclear",
    "保留",
    "要判断",
];
const BODY_BLOCK_HINTS: &[&str] = &[
    "pending human input",
    "needs decision",
    "waiting on human",
    "human input required",
    "blocked pending",
    "requires human approval",
    "要判断",
];

/// Errors while selecting from a snapshot v0 document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParityError {
    /// Snapshot root / required fields invalid.
    Invalid(String),
}

impl std::fmt::Display for ParityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(msg) => write!(f, "parity_input_invalid:{msg}"),
        }
    }
}

impl std::error::Error for ParityError {}

/// Canonical JSON bytes matching Python `canonical_json_bytes`.
pub fn canonical_json_bytes(value: &Value) -> Vec<u8> {
    // serde_json::to_vec does not sort keys; use a sorted Map walk.
    let sorted = sort_value(value);
    serde_json::to_vec(&sorted).expect("canonical json")
}

fn sort_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for key in keys {
                out.insert(key.clone(), sort_value(&map[key]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_value).collect()),
        other => other.clone(),
    }
}

/// SHA-256 hex of canonical snapshot content.
pub fn content_hash(value: &Value) -> String {
    let digest = Sha256::digest(canonical_json_bytes(value));
    hex::encode(digest)
}

/// True when value looks like backlog-promoter-snapshot.v0.
pub fn is_snapshot_v0(value: &Value) -> bool {
    value.get("snapshot_schema_version").and_then(Value::as_str) == Some(SNAPSHOT_SCHEMA_VERSION)
}

/// Select Python-shaped compact payload from a snapshot v0 object.
pub fn select_compact_from_snapshot(snapshot: &Value) -> Result<Value, ParityError> {
    if !is_snapshot_v0(snapshot) {
        return Err(ParityError::Invalid("not snapshot v0".into()));
    }
    if snapshot.get("controller").and_then(Value::as_str) != Some("backlog_promoter") {
        return Err(ParityError::Invalid(
            "controller must be backlog_promoter".into(),
        ));
    }
    if !snapshot.get("error").is_none_or(Value::is_null) {
        return Err(ParityError::Invalid("snapshot.error is set".into()));
    }
    let limits = snapshot.get("limits").and_then(Value::as_object);
    if limits.is_some_and(|l| {
        l.get("issues_truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || l.get("bytes_truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false)
    }) {
        return Err(ParityError::Invalid("snapshot is truncated".into()));
    }

    let now = parse_now(
        snapshot
            .get("now_jst")
            .and_then(Value::as_str)
            .ok_or_else(|| ParityError::Invalid("now_jst missing".into()))?,
    )?;
    let hash = content_hash(snapshot);
    let issues = snapshot
        .get("issues")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let schedule_inventory = snapshot
        .get("schedule_inventory")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut status_by_id: Map<String, Value> = Map::new();
    for issue in &issues {
        if let Some(id) = issue.get("id").and_then(Value::as_str) {
            status_by_id.insert(
                id.to_string(),
                json!(issue.get("status").and_then(Value::as_str).unwrap_or("")),
            );
        }
    }
    if let Some(extra) = snapshot
        .get("dependency_status_by_id")
        .and_then(Value::as_object)
    {
        for (k, v) in extra {
            status_by_id.insert(k.clone(), v.clone());
        }
    }
    let evidence = snapshot
        .get("evidence_preflight_by_issue_id")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let run_presence = snapshot
        .get("issue_run_presence")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut warnings: Vec<Value> = snapshot
        .get("warnings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Schedule admission (Python evaluate_controller_admission)
    let admission = evaluate_controller_admission(&schedule_inventory, &now);
    if !admission
        .get("admit")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        let reason = admission
            .get("reason")
            .cloned()
            .unwrap_or_else(|| json!("schedule_collision_blocked"));
        // Match Python early-return: schedule stop happens before cap_state is computed.
        return Ok(stop_compact(&now, &hash, reason, json!({}), &warnings));
    }

    let backlog: Vec<&Value> = issues
        .iter()
        .filter(|i| status_of(i) == "backlog")
        .collect();
    let todos: Vec<&Value> = issues.iter().filter(|i| status_of(i) == "todo").collect();
    let active: Vec<&Value> = issues
        .iter()
        .filter(|i| ACTIVE_STATUSES.contains(&status_of(i).as_str()))
        .collect();

    let mut active_signatures = Vec::new();
    for issue in &active {
        active_signatures.extend(issue_signatures(issue));
    }

    let ai_todo_total = todos.iter().filter(|i| is_ai_todo(i)).count() as i64;
    let pr_producing_todo_total = todos
        .iter()
        .filter(|i| is_ai_todo(i) && is_pr_producing(i))
        .count() as i64;
    let review_fix_exempt_todo_total = todos
        .iter()
        .filter(|i| is_ai_todo(i) && is_review_fix_exempt(i))
        .count() as i64;
    let human_todo_total = todos.iter().filter(|i| !is_ai_todo(i)).count() as i64;
    let cap_state = json!({
        "ai_todo_total": ai_todo_total,
        "ai_todo_headroom": (AI_TODO_CAP - ai_todo_total).max(0),
        "pr_producing_todo_total": pr_producing_todo_total,
        "pr_producing_headroom": (PR_PRODUCING_TODO_CAP - pr_producing_todo_total).max(0),
        "review_fix_exempt_todo_total": review_fix_exempt_todo_total,
        "review_fix_exempt_headroom": (REVIEW_FIX_EXEMPT_TODO_CAP - review_fix_exempt_todo_total).max(0),
        "human_todo_total": human_todo_total,
        "max_promotions_this_run": NORMAL_PROMOTION_LIMIT,
    });

    if ai_todo_total >= AI_TODO_CAP || pr_producing_todo_total >= PR_PRODUCING_TODO_CAP {
        return Ok(stop_compact(
            &now,
            &hash,
            json!("todo_cap_reached"),
            cap_state,
            &warnings,
        ));
    }

    let mut skipped_summary: Map<String, Value> = Map::new();
    let mut age_boost_pending: Vec<(&Value, String, String)> = Vec::new();
    let mut fallback_age_warning = false;
    let mut candidate_rows: Vec<CandidateRow> = Vec::new();

    for issue in &backlog {
        let issue_id = string_field(issue, "id");
        let issue_key = issue_key(issue);
        let age_days = backlog_age_days(issue, &now);
        if age_days >= AGE_BOOST_DAYS && metadata_bool(issue, "age_boosted") != Some(true) {
            age_boost_pending.push((issue, issue_id.clone(), issue_key.clone()));
            if !fallback_age_warning {
                warnings.push(json!({
                    "code": "missing_status_history",
                    "message": "created_at used as backlog age fallback",
                }));
                fallback_age_warning = true;
            }
        }

        let mut skip_reason: Option<&str> = None;
        if metadata_str(issue, "work_owner") == Some("human") {
            skip_reason = Some("human_owned");
        } else if metadata_str(issue, "waiting_on").is_some()
            || metadata_str(issue, "blocked_reason").is_some()
            || metadata_value(issue, "waiting_on").is_some()
            || metadata_value(issue, "blocked_reason").is_some()
        {
            skip_reason = Some("blocked_reason");
        } else if blocked_text_reason(issue) {
            skip_reason = Some("blocked_text");
        } else if unresolved_dependencies(issue, &status_by_id) {
            skip_reason = Some("blocked_dependency");
        } else {
            let preflight = evidence
                .get(&issue_id)
                .cloned()
                .unwrap_or_else(|| json!({"ok": true, "reason_code": "no_repo_references"}));
            let ok = preflight
                .get("ok")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let reason_code = preflight
                .get("reason_code")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !ok && reason_code != "no_repo_references" && reason_code != "workspace_unavailable"
            {
                skip_reason = Some("issue_evidence_preflight_failed");
            }
        }

        if skip_reason.is_none() {
            let sigs = issue_signatures(issue);
            if !sigs.is_empty() && sigs.iter().any(|s| active_signatures.contains(s)) {
                skip_reason = Some("same_scope_active");
            } else if run_has_active_or_queued(&run_presence, &issue_key) {
                skip_reason = Some("running_run_exists");
            } else if let Some(cooldown) =
                metadata_str(issue, "todo_runner_no_action_until").and_then(|s| parse_now(s).ok())
            {
                if cooldown > now {
                    skip_reason = Some("cooldown_active");
                }
            }
        }

        if let Some(reason) = skip_reason {
            bump(&mut skipped_summary, reason);
            continue;
        }

        let (bucket, sort_key, selection_reason) = priority_bucket(issue, &now);
        candidate_rows.push(CandidateRow {
            bucket,
            sort_key,
            issue,
            issue_id,
            issue_key,
            project_key: project_key_for(issue),
            selection_reason,
            is_pr_producing: is_pr_producing(issue),
            is_review_fix_exempt: is_review_fix_exempt(issue),
            is_special: is_special_promotion(issue),
        });
    }

    candidate_rows.sort_by_key(|a| (a.bucket, a.sort_key.clone()));

    let mut selected: Vec<Value> = Vec::new();
    let mut simulated_ai = ai_todo_total;
    let mut simulated_pr = pr_producing_todo_total;
    let mut simulated_exempt = review_fix_exempt_todo_total;
    let mut normal_count = 0_i64;
    let mut special_count = 0_i64;
    let normal_limit = NORMAL_PROMOTION_LIMIT;

    for row in &candidate_rows {
        if row.is_special {
            if special_count >= SPECIAL_PROMOTION_LIMIT {
                bump(&mut skipped_summary, "special_run_limit_reached");
                continue;
            }
        } else if normal_count >= normal_limit {
            bump(&mut skipped_summary, "normal_run_limit_reached");
            continue;
        }

        if is_ai_todo(row.issue) && simulated_ai + 1 > AI_TODO_CAP {
            bump(&mut skipped_summary, "ai_todo_cap_reached");
            continue;
        }
        if row.is_pr_producing && simulated_pr + 1 > PR_PRODUCING_TODO_CAP {
            bump(&mut skipped_summary, "pr_todo_cap_reached");
            continue;
        }
        if row.is_review_fix_exempt && simulated_exempt + 1 > REVIEW_FIX_EXEMPT_TODO_CAP {
            bump(&mut skipped_summary, "review_fix_exempt_cap_reached");
            continue;
        }

        selected.push(json!({
            "issue_id": row.issue_id,
            "issue_key": row.issue_key,
            "project_key": row.project_key,
            "title": string_field(row.issue, "title"),
            "selection_reason": row.selection_reason,
        }));
        if row.is_special {
            special_count += 1;
        } else {
            normal_count += 1;
        }
        if is_ai_todo(row.issue) {
            simulated_ai += 1;
        }
        if row.is_pr_producing {
            simulated_pr += 1;
        }
        if row.is_review_fix_exempt {
            simulated_exempt += 1;
        }
    }

    let promotion_ids: Vec<String> = selected
        .iter()
        .filter_map(|s| {
            s.get("issue_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let mut age_boost_actions = Vec::new();
    let mut age_boost_skipped = 0_i64;
    for (issue, issue_id, issue_key) in age_boost_pending {
        if promotion_ids.iter().any(|id| id == &issue_id) {
            age_boost_skipped += 1;
            continue;
        }
        if let Some(action) = build_age_boost_action(issue, &now) {
            age_boost_actions.push(action);
        } else {
            age_boost_skipped += 1;
        }
        let _ = issue_key;
    }

    let chosen_action = if !selected.is_empty() {
        "promote_backlog"
    } else if !age_boost_actions.is_empty() {
        "age_boost_only"
    } else {
        "no_candidate"
    };
    let reason = if selected.is_empty() {
        json!("no_candidate")
    } else {
        Value::Null
    };
    let recommended_next_action = if selected.is_empty() && age_boost_actions.is_empty() {
        "create_replenishment_or_final_report"
    } else if selected.is_empty() {
        "apply_age_boost_only"
    } else {
        "apply_promotions"
    };
    let replenishment_candidate = if selected.is_empty() && age_boost_actions.is_empty() {
        replenish_candidate(&skipped_summary, candidate_rows.len())
    } else {
        Value::Null
    };
    let candidate_issue_keys: Vec<Value> = selected
        .iter()
        .map(|item| item["issue_key"].clone())
        .collect();

    Ok(json!({
        "schema_version": 1,
        "decision": "continue",
        "reason": reason.clone(),
        "fast_exit_required": false,
        "recommended_next_action": recommended_next_action,
        "run_only_result": {
            "decision": "continue",
            "reason": reason,
            "selected": !selected.is_empty(),
            "promotion_candidate_count": selected.len(),
            "chosen_action": chosen_action,
            "candidate_issue_keys": candidate_issue_keys,
        },
        "run_context": {
            "now_jst": now.to_rfc3339(),
            "deterministic_mode": true,
            "selection_input_mode": "snapshot",
            "consumed_snapshot_hash": hash,
            "consumed_snapshot_hash_mode": "content",
            "snapshot_schema_version": SNAPSHOT_SCHEMA_VERSION,
        },
        "cap_state": cap_state,
        "promotion_candidates": selected,
        "age_boost_action_count": age_boost_actions.len(),
        "age_boost_skipped_count": age_boost_skipped,
        "replenishment_candidate": replenishment_candidate,
        "skipped_summary": skipped_summary,
        "metrics": {
            "backlog_count": backlog.len(),
            "candidate_count": candidate_rows.len(),
            "promotion_candidate_count": promotion_ids.len(),
            "chosen_action": chosen_action,
        },
        "warnings": warnings,
    }))
}

struct CandidateRow<'a> {
    bucket: i64,
    sort_key: (i64, String),
    issue: &'a Value,
    issue_id: String,
    issue_key: String,
    project_key: String,
    selection_reason: &'static str,
    is_pr_producing: bool,
    is_review_fix_exempt: bool,
    is_special: bool,
}

fn stop_compact(
    now: &DateTime<FixedOffset>,
    hash: &str,
    reason: Value,
    cap_state: Value,
    warnings: &[Value],
) -> Value {
    json!({
        "schema_version": 1,
        "decision": "stop",
        "reason": reason.clone(),
        "fast_exit_required": true,
        "recommended_next_action": "final_report_only",
        "run_only_result": {
            "decision": "stop",
            "reason": reason,
            "selected": false,
            "promotion_candidate_count": 0,
            "chosen_action": "final_report_only",
            "candidate_issue_keys": [],
        },
        "run_context": {
            "now_jst": now.to_rfc3339(),
            "deterministic_mode": true,
            "selection_input_mode": "snapshot",
            "consumed_snapshot_hash": hash,
            "consumed_snapshot_hash_mode": "content",
            "snapshot_schema_version": SNAPSHOT_SCHEMA_VERSION,
        },
        "cap_state": cap_state,
        "promotion_candidates": [],
        "age_boost_action_count": 0,
        "age_boost_skipped_count": 0,
        "replenishment_candidate": null,
        "skipped_summary": {},
        "metrics": {
            "backlog_count": null,
            "candidate_count": null,
            "promotion_candidate_count": null,
            "chosen_action": null,
        },
        "warnings": warnings,
    })
}

fn evaluate_controller_admission(rows: &[Value], now: &DateTime<FixedOffset>) -> Value {
    let controller_key = "backlog-promoter";
    let title = "Schedule - Backlog Promoter";
    let me_rows: Vec<&Value> = rows
        .iter()
        .filter(|row| {
            row.get("controller_key").and_then(Value::as_str) == Some(controller_key)
                || row.get("autopilot_title").and_then(Value::as_str) == Some(title)
        })
        .collect();
    if me_rows.is_empty() {
        return json!({
            "admit": true,
            "reason": "controller_not_in_inventory",
            "controller_key": controller_key,
            "warning": "controller row missing from inventory",
        });
    }
    let runtime_lane = me_rows[0]
        .get("runtime_lane")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let me_id = me_rows[0]
        .get("autopilot_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let other_active: Vec<String> = rows
        .iter()
        .filter(|row| row.get("runtime_lane").and_then(Value::as_str) == Some(runtime_lane))
        .filter(|row| {
            let ck = row
                .get("controller_key")
                .and_then(Value::as_str)
                .unwrap_or("");
            let aid = row
                .get("autopilot_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            ck != controller_key && aid != me_id
        })
        .filter(|row| is_active_controller_row(row, now))
        .filter_map(|row| {
            row.get("autopilot_title")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    if !other_active.is_empty() {
        let mut titles = other_active;
        titles.sort();
        titles.dedup();
        return json!({
            "admit": false,
            "reason": "same_runtime_active_controller",
            "controller_key": controller_key,
            "runtime_lane": runtime_lane,
            "active_controllers": titles,
        });
    }
    json!({
        "admit": true,
        "reason": null,
        "controller_key": controller_key,
        "runtime_lane": runtime_lane,
    })
}

fn is_active_controller_row(row: &Value, now: &DateTime<FixedOffset>) -> bool {
    let status = row
        .get("last_run_status")
        .and_then(Value::as_str)
        .unwrap_or("");
    if ACTIVE_AUTOPILOT_RUN.contains(&status) {
        return true;
    }
    if let Some(last) = row
        .get("last_run_at")
        .and_then(Value::as_str)
        .and_then(|s| parse_now(s).ok())
    {
        return same_valve_tick(&last, now);
    }
    false
}

fn same_valve_tick(left: &DateTime<FixedOffset>, right: &DateTime<FixedOffset>) -> bool {
    valve_tick_start(left) == valve_tick_start(right)
}

fn valve_tick_start(dt: &DateTime<FixedOffset>) -> DateTime<FixedOffset> {
    let minute = (dt.minute() / 30) * 30;
    dt.date_naive()
        .and_hms_opt(dt.hour(), minute, 0)
        .and_then(|naive| dt.timezone().from_local_datetime(&naive).single())
        .unwrap_or(*dt)
}

fn replenish_candidate(skipped_summary: &Map<String, Value>, candidate_count: usize) -> Value {
    if candidate_count > 0 {
        return json!({"action": "none", "reason": "promotion_candidates_blocked_by_caps"});
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

fn priority_bucket(
    issue: &Value,
    now: &DateTime<FixedOffset>,
) -> (i64, (i64, String), &'static str) {
    let age_days = backlog_age_days(issue, now);
    let created = string_field(issue, "created_at");
    if metadata_bool(issue, "age_boosted") == Some(true) {
        return (1, (-age_days, created), "age_boosted");
    }
    if is_review_fix(issue) {
        return (2, (0, created), "review_fix");
    }
    if is_dependency_unblocking(issue) {
        return (3, (0, created), "dependency_unblock");
    }
    if let Some(target) = metadata_str(issue, "today_review_target") {
        if target == now.date_naive().to_string() {
            let order = metadata_i64(issue, "today_review_order").unwrap_or(9999);
            return (4, (order, created), "today_committed_batch");
        }
    }
    if let Some(seq) = metadata_i64(issue, "sequence_index") {
        return (5, (seq, created), "sequence_index");
    }
    (6, (-age_days, created), "older_backlog")
}

fn build_age_boost_action(issue: &Value, now: &DateTime<FixedOffset>) -> Option<Value> {
    if metadata_headroom(issue, AGE_BOOST_KEYS) < 0 {
        return None;
    }
    Some(json!({
        "issue_id": string_field(issue, "id"),
        "issue_key": issue_key(issue),
        "metadata_key_count": count_metadata_items(issue),
        "set": {
            "age_boosted": true,
            "age_boosted_at": now.to_rfc3339(),
        }
    }))
}

fn unresolved_dependencies(issue: &Value, status_by_id: &Map<String, Value>) -> bool {
    let blocked_ids = metadata_list(issue, "blocked_by_issue_ids");
    if !blocked_ids.is_empty() {
        for blocked_id in blocked_ids {
            let status = status_by_id
                .get(&blocked_id)
                .and_then(Value::as_str)
                .unwrap_or("");
            if !DONE_LIKE.contains(&status) {
                return true;
            }
        }
        return false;
    }
    !metadata_list(issue, "blocked_by_local").is_empty()
}

fn run_has_active_or_queued(presence: &Map<String, Value>, issue_key: &str) -> bool {
    let Some(row) = presence.get(issue_key) else {
        return false;
    };
    if let Some(flag) = row.get("has_active_or_queued_run").and_then(Value::as_bool) {
        return flag;
    }
    let active = row
        .get("active_run_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let queued = row
        .get("queued_run_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    active > 0 || queued > 0
}

fn is_ai_todo(issue: &Value) -> bool {
    if metadata_str(issue, "work_owner") == Some("human") {
        return false;
    }
    if issue.get("assignee_type").and_then(Value::as_str) == Some("member") {
        return false;
    }
    true
}

fn is_pr_producing(issue: &Value) -> bool {
    if metadata_bool(issue, "pr_required") == Some(true) {
        return true;
    }
    matches!(
        metadata_str(issue, "task_kind"),
        Some("implementation" | "pr_delivery")
    )
}

fn is_review_fix(issue: &Value) -> bool {
    metadata_str(issue, "task_kind") == Some("review_fix")
}

fn is_review_fix_exempt(issue: &Value) -> bool {
    if is_review_fix(issue) && metadata_bool(issue, "review_fix_counts_toward_cap") == Some(false) {
        return true;
    }
    metadata_str(issue, "scheduled_task_family") == Some("coderabbit-pr-fix-monitor")
}

fn is_dependency_unblocking(issue: &Value) -> bool {
    for key in [
        "unblocks_local",
        "unblocks_issue_ids",
        "unblocks_issue_keys",
    ] {
        if !metadata_list(issue, key).is_empty() {
            return true;
        }
    }
    let text = format!(
        "{}\n{}",
        string_field(issue, "title").to_lowercase(),
        string_field(issue, "description").to_lowercase()
    );
    text.contains("unblock") || text.contains("dependency")
}

fn is_special_promotion(issue: &Value) -> bool {
    is_review_fix(issue) || is_dependency_unblocking(issue)
}

fn blocked_text_reason(issue: &Value) -> bool {
    let title = string_field(issue, "title").to_lowercase();
    let body = issue_body_text(issue);
    TITLE_BLOCK_HINTS.iter().any(|h| title.contains(h))
        || BODY_BLOCK_HINTS.iter().any(|h| body.contains(h))
}

fn issue_body_text(issue: &Value) -> String {
    let mut text = string_field(issue, "description");
    if text.starts_with("---\n") {
        if let Some((_, rest)) = text.split_once("\n---\n") {
            text = rest.to_string();
        }
    }
    text.to_lowercase()
}

fn issue_signatures(issue: &Value) -> Vec<String> {
    let mut sigs = Vec::new();
    if let (Some(family), Some(scope)) = (
        metadata_str(issue, "scheduled_task_family"),
        metadata_str(issue, "scheduled_task_scope"),
    ) {
        sigs.push(format!("family_scope:{family}::{scope}"));
    }
    if let Some(source) = metadata_str(issue, "source_issue_id") {
        sigs.push(format!("source_issue_id:{source}"));
    }
    if let Some(pr_url) = metadata_str(issue, "pr_url") {
        sigs.push(format!("pr_url:{}", pr_url.trim()));
    }
    if let Some(same_scope) = metadata_str(issue, "same_scope_key") {
        sigs.push(format!("same_scope_key:{same_scope}"));
    }
    sigs
}

fn project_key_for(issue: &Value) -> String {
    // Match Python `autopilot_common.project_key_for` (default: no PR fallback).
    for key in ["project_key", "roadmap_project_slug"] {
        if let Some(value) = metadata_str(issue, key) {
            return value.to_string();
        }
    }
    if let Some(dedupe_key) = metadata_str(issue, "dedupe_key") {
        if let Some((head, _)) = dedupe_key.split_once(':') {
            if !head.is_empty() {
                return head.to_string();
            }
        }
    }
    if let Some(source_path) = metadata_str(issue, "source_path") {
        if source_path.contains("/OSS/") {
            let parts: Vec<&str> = source_path.split('/').collect();
            if let Some(idx) = parts.iter().position(|part| *part == "OSS") {
                if let Some(next) = parts.get(idx + 1).copied().filter(|s| !s.is_empty()) {
                    return next.to_string();
                }
            }
        }
    }
    let title = string_field(issue, "title").trim().to_ascii_lowercase();
    if let Some(key) = title_hyphenated_project_prefix(&title) {
        return key;
    }
    String::new()
}

/// Python regex: `^([a-z0-9]+(?:-[a-z0-9]+)+):`
fn title_hyphenated_project_prefix(title_lower: &str) -> Option<String> {
    let bytes = title_lower.as_bytes();
    let mut i = 0usize;
    let mut hyphen_segments = 0usize;
    while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    while i < bytes.len() && bytes[i] == b'-' {
        let start = i + 1;
        i = start;
        while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
            i += 1;
        }
        if i == start {
            return None;
        }
        hyphen_segments += 1;
    }
    if hyphen_segments == 0 {
        return None;
    }
    if i < bytes.len() && bytes[i] == b':' {
        return Some(title_lower[..i].to_string());
    }
    None
}

fn backlog_age_days(issue: &Value, now: &DateTime<FixedOffset>) -> i64 {
    let created_raw = string_field(issue, "created_at");
    if created_raw.is_empty() {
        return 0;
    }
    let Ok(created) = parse_now(&created_raw) else {
        return 0;
    };
    (*now - created).num_days().max(0)
}

fn parse_now(raw: &str) -> Result<DateTime<FixedOffset>, ParityError> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(dt);
    }
    // Accept trailing Z via UTC.
    if let Ok(dt) = raw.parse::<DateTime<Utc>>() {
        return Ok(dt.fixed_offset());
    }
    Err(ParityError::Invalid(format!("invalid timestamp: {raw}")))
}

fn status_of(issue: &Value) -> String {
    issue
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase()
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

fn metadata_map(issue: &Value) -> Map<String, Value> {
    issue
        .get("metadata")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn metadata_value<'a>(issue: &'a Value, key: &str) -> Option<&'a Value> {
    issue.get("metadata")?.as_object()?.get(key)
}

fn metadata_str<'a>(issue: &'a Value, key: &str) -> Option<&'a str> {
    match metadata_value(issue, key)? {
        Value::Null => None,
        Value::String(s) if s.is_empty() => None,
        Value::String(s) => Some(s.as_str()),
        other => other.as_str(),
    }
}

fn metadata_bool(issue: &Value, key: &str) -> Option<bool> {
    metadata_value(issue, key)?.as_bool()
}

fn metadata_i64(issue: &Value, key: &str) -> Option<i64> {
    let value = metadata_value(issue, key)?;
    value
        .as_i64()
        .or_else(|| value.as_str()?.parse::<i64>().ok())
}

/// Match Python `parse_metadata_value` + `as_list` for Multica metadata fields.
/// Importantly, Multica often stores empty lists as the JSON string `"[]"`.
fn coerce_metadata_list(value: &Value) -> Vec<String> {
    match value {
        Value::Null => Vec::new(),
        Value::Array(items) => items
            .iter()
            .filter_map(|v| match v {
                Value::String(s) if !s.is_empty() => Some(s.clone()),
                other => other.as_str().map(str::to_string).filter(|s| !s.is_empty()),
            })
            .collect(),
        Value::String(raw) => {
            let stripped = raw.trim();
            if stripped.is_empty() || stripped == "[]" {
                return Vec::new();
            }
            if stripped.starts_with('[') || stripped.starts_with('{') {
                if let Ok(parsed) = serde_json::from_str::<Value>(stripped) {
                    return coerce_metadata_list(&parsed);
                }
            }
            vec![stripped.to_string()]
        }
        other => other
            .as_str()
            .map(|s| vec![s.to_string()])
            .unwrap_or_default(),
    }
}

fn metadata_list(issue: &Value, key: &str) -> Vec<String> {
    match metadata_value(issue, key) {
        Some(value) => coerce_metadata_list(value),
        None => Vec::new(),
    }
}

fn count_metadata_items(issue: &Value) -> usize {
    metadata_map(issue).len()
}

fn metadata_headroom(issue: &Value, keys_to_add: &[&str]) -> isize {
    let existing = metadata_map(issue);
    let missing = keys_to_add
        .iter()
        .filter(|key| !existing.contains_key(**key))
        .count();
    METADATA_KEY_LIMIT as isize - existing.len() as isize - missing as isize
}

fn bump(map: &mut Map<String, Value>, key: &str) {
    let next = map.get(key).and_then(Value::as_u64).unwrap_or(0) + 1;
    map.insert(key.to_string(), json!(next));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_candidate_snapshot_selects_am_201() {
        let raw = include_str!("../testdata/one_candidate.snapshot.json");
        let snapshot: Value = serde_json::from_str(raw).unwrap();
        let actual = select_compact_from_snapshot(&snapshot).unwrap();
        assert_eq!(actual["promotion_candidates"][0]["issue_key"], "AM-201");
        assert_eq!(
            actual["promotion_candidates"][0]["selection_reason"],
            "older_backlog"
        );
        assert_eq!(actual["cap_state"]["ai_todo_total"], 1);
        assert_eq!(actual["skipped_summary"]["human_owned"], 1);
        assert_eq!(actual["run_context"]["selection_input_mode"], "snapshot");
        assert_eq!(
            actual["run_context"]["consumed_snapshot_hash"],
            content_hash(&snapshot)
        );
    }

    #[test]
    fn metadata_list_treats_json_empty_array_string_as_empty() {
        let issue = json!({
            "metadata": {
                "blocked_by_local": "[]",
                "blocked_by_issue_ids": "[]",
                "unblocks_issue_ids": "[\"x\"]"
            }
        });
        assert!(metadata_list(&issue, "blocked_by_local").is_empty());
        assert!(metadata_list(&issue, "blocked_by_issue_ids").is_empty());
        assert_eq!(
            metadata_list(&issue, "unblocks_issue_ids"),
            vec!["x".to_string()]
        );
        assert!(!unresolved_dependencies(&issue, &Map::new()));
    }

    #[test]
    fn empty_blocked_by_local_string_does_not_skip_candidate() {
        let raw = include_str!("../testdata/one_candidate.snapshot.json");
        let mut snapshot: Value = serde_json::from_str(raw).unwrap();
        // Live Multica stores empty lists as the JSON string "[]".
        if let Some(issues) = snapshot.get_mut("issues").and_then(Value::as_array_mut) {
            for issue in issues.iter_mut() {
                if issue.get("identifier").and_then(Value::as_str) == Some("AM-201") {
                    let meta = issue
                        .as_object_mut()
                        .unwrap()
                        .entry("metadata")
                        .or_insert_with(|| json!({}));
                    meta.as_object_mut()
                        .unwrap()
                        .insert("blocked_by_local".to_string(), json!("[]"));
                }
            }
        }
        let actual = select_compact_from_snapshot(&snapshot).unwrap();
        assert_eq!(actual["promotion_candidates"][0]["issue_key"], "AM-201");
        assert!(actual
            .get("skipped_summary")
            .and_then(|s| s.get("blocked_dependency"))
            .is_none());
    }

    #[test]
    fn project_key_for_uses_dedupe_key_prefix() {
        let issue = json!({
            "title": "Surface masked scheduled-router slot warnings",
            "metadata": {
                "dedupe_key": "pi-scheduled-router:4_Project/OSS/pi-scheduled-router/Issues/08.md",
                "blocked_by_local": "[]"
            }
        });
        assert_eq!(project_key_for(&issue), "pi-scheduled-router");
    }

    #[test]
    fn project_key_for_uses_source_path_oss_segment() {
        let issue = json!({
            "title": "Something",
            "metadata": {
                "source_path": "4_Project/OSS/agentmesh/Issues/01.md"
            }
        });
        assert_eq!(project_key_for(&issue), "agentmesh");
    }

    #[test]
    fn project_key_for_uses_hyphenated_title_prefix() {
        let issue = json!({
            "title": "pi-scheduled-router: polish warnings",
            "metadata": {}
        });
        assert_eq!(project_key_for(&issue), "pi-scheduled-router");
    }
}
