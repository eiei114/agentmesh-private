//! Production authority one-shot wiring with promotion gates and Cursor recovery.
//!
//! Combines pinned Multica CLI adapter and app-local control ledger. Supports
//! observer through todo_runner authority modes with deterministic promotion
//! gates, allowed-operation argv mapping, and health-gated Cursor retry.
//!
//! Failed non-Cursor mutation runs that claimed idempotency but did not complete
//! cleanly leave an ambiguous consumed claim; operators must inspect ledger
//! decisions and Multica state and perform explicit manual recovery. There is no
//! generic automatic retry for those mutations.

#![allow(clippy::too_many_arguments)]

use agentmesh_local_control_ledger::{run_local_control_ledger, AUTHORITY_MODES};
use agentmesh_multica_cli_adapter::{run_multica_cli_adapter, ProcessRunner, QUERY_OPERATION_ARGS};
use chrono::{DateTime, FixedOffset};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Plugin/schema version exposed in compact output.
pub const PRODUCTION_AUTHORITY_VERSION: &str = "production-authority.v0";
const INPUT_SCHEMA_VERSION: &str = "production-authority-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "production-authority-output.v0";
const DEFAULT_LEASE_TTL_SECS: u64 = 300;
const MIN_LEASE_TTL_SECS: u64 = 30;
const MAX_LEASE_TTL_SECS: u64 = 3600;
const DEFAULT_CLI_TIMEOUT_MS: u64 = 60_000;

const CURSOR_FAILURE_CLASSES: &[&str] = &["availability_bridge_failure"];
const CURSOR_HEALTH_TRANSITIONS: &[&str] = &["down_to_healthy"];

const AUTHORITY_PREDECESSORS: &[(&str, &str)] = &[
    ("observer", "shadow"),
    ("safe_writer", "observer"),
    ("queue", "safe_writer"),
    ("todo_runner", "queue"),
];

#[derive(Debug, Clone, Copy)]
struct PromotionGate {
    min_days: i64,
    min_decisions: i64,
    max_unauthorized_writes: i64,
    max_duplicate_mutations: i64,
    max_duplicate_starts: i64,
    min_hard_gate_parity_pct: i64,
}

fn gate_for_mode(mode: &str) -> Option<PromotionGate> {
    match mode {
        "observer" => Some(PromotionGate {
            min_days: 3,
            min_decisions: 20,
            max_unauthorized_writes: i64::MAX,
            max_duplicate_mutations: i64::MAX,
            max_duplicate_starts: i64::MAX,
            min_hard_gate_parity_pct: 100,
        }),
        "safe_writer" => Some(PromotionGate {
            min_days: 7,
            min_decisions: 50,
            max_unauthorized_writes: 0,
            max_duplicate_mutations: i64::MAX,
            max_duplicate_starts: i64::MAX,
            min_hard_gate_parity_pct: 100,
        }),
        "queue" => Some(PromotionGate {
            min_days: 7,
            min_decisions: 50,
            max_unauthorized_writes: i64::MAX,
            max_duplicate_mutations: 0,
            max_duplicate_starts: i64::MAX,
            min_hard_gate_parity_pct: 100,
        }),
        "todo_runner" => Some(PromotionGate {
            min_days: 14,
            min_decisions: 100,
            max_unauthorized_writes: i64::MAX,
            max_duplicate_mutations: i64::MAX,
            max_duplicate_starts: 0,
            min_hard_gate_parity_pct: 100,
        }),
        _ => None,
    }
}

fn allowed_multica_operation(mode: &str, multica_operation: &str) -> bool {
    match mode {
        "observer" => false,
        "safe_writer" => matches!(
            multica_operation,
            "safe_writer_done_reconcile" | "safe_writer_issue_create" | "safe_writer_issue_import"
        ),
        "queue" => multica_operation == "queue_backlog_promote",
        "todo_runner" => {
            matches!(
                multica_operation,
                "todo_runner_assign" | "todo_runner_rerun"
            )
        }
        _ => false,
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityInput {
    schema_version: String,
    operation: String,
    controller_id: String,
    execution_kind: String,
    authority_mode: String,
    ledger_path: String,
    cli_path: String,
    now: String,
    #[serde(default)]
    scope_key: Option<String>,
    #[serde(default)]
    lease_id: Option<String>,
    #[serde(default)]
    lease_ttl_seconds: Option<u64>,
    #[serde(default)]
    cli_timeout_ms: Option<u64>,
    #[serde(default)]
    multica_operation: Option<String>,
    #[serde(default)]
    operation_params: Option<Value>,
    #[serde(default)]
    target_mode: Option<String>,
    #[serde(default)]
    cursor_recovery: Option<CursorRecoveryInput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorRecoveryInput {
    issue_id: String,
    failure_class: String,
    health_transition: String,
    no_artifacts: bool,
    same_issue: bool,
}

fn issue(code: &str, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn sha256_hex(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    format!("sha256:{}", hex::encode(digest))
}

fn resolve_lease_ttl_seconds(raw: Option<u64>) -> Result<u64, String> {
    let ttl = raw.unwrap_or(DEFAULT_LEASE_TTL_SECS);
    if (MIN_LEASE_TTL_SECS..=MAX_LEASE_TTL_SECS).contains(&ttl) {
        Ok(ttl)
    } else {
        Err(format!(
            "lease_ttl_seconds must be {MIN_LEASE_TTL_SECS}..={MAX_LEASE_TTL_SECS}"
        ))
    }
}

fn resolve_cli_timeout_ms(raw: Option<u64>) -> Result<u64, String> {
    let timeout = raw.unwrap_or(DEFAULT_CLI_TIMEOUT_MS);
    agentmesh_proto::Limits::validate_run_timeout_ms(timeout).map_err(|e| e.to_string())
}

fn lease_expires_at(now: &str, ttl_seconds: u64) -> Result<String, String> {
    let parsed = DateTime::parse_from_rfc3339(now).map_err(|e| e.to_string())?;
    let expires: DateTime<FixedOffset> =
        parsed + chrono::Duration::seconds(i64::try_from(ttl_seconds).unwrap());
    Ok(expires.to_rfc3339())
}

fn since_ts(now: &str, days: i64) -> Result<String, String> {
    let parsed = DateTime::parse_from_rfc3339(now).map_err(|e| e.to_string())?;
    let since = parsed - chrono::Duration::days(days);
    Ok(since.to_rfc3339())
}

fn compact(
    operation: &str,
    valid: bool,
    exit_reason: &str,
    issues: Vec<Value>,
    mutation_performed: bool,
    cli: Value,
    ledger: Value,
    extra: Value,
) -> Value {
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "app_version": PRODUCTION_AUTHORITY_VERSION,
        "operation": operation,
        "valid": valid,
        "exit_reason": exit_reason,
        "mutation_performed": mutation_performed,
        "issue_count": issues.len(),
        "issues": issues,
        "cli": cli,
        "ledger": ledger,
        "extra": extra,
    })
}

fn ledger_op(base: &AuthorityInput, operation: &str, extra: Value) -> Value {
    let mut payload = json!({
        "schema_version": "local-control-ledger-input.v0",
        "operation": operation,
        "ledger_path": base.ledger_path,
        "controller_id": base.controller_id,
        "updated_at": base.now,
        "recorded_at": base.now,
        "acquired_at": base.now,
    });
    if let Some(obj) = payload.as_object_mut() {
        if let Some(extra_obj) = extra.as_object() {
            for (key, value) in extra_obj {
                obj.insert(key.clone(), value.clone());
            }
        }
    }
    run_local_control_ledger(&payload)
}

fn redact_cli_summary(cli: &Value) -> Value {
    json!({
        "schema_version": cli["schema_version"],
        "operation": cli["operation"],
        "valid": cli["valid"],
        "exit_reason": cli["exit_reason"],
        "exit_code": cli["exit_code"],
        "stdout_sha256": cli["stdout_sha256"],
        "stdout_byte_count": cli["stdout_byte_count"],
        "stdout_truncated": cli["stdout_truncated"],
        "stderr_byte_count": cli["stderr_byte_count"],
        "json_parse_ok": cli["json_parse_ok"],
        "json_top_level_kind": cli["json_top_level_kind"],
        "timed_out": cli["timed_out"],
    })
}

struct RunGuard<'a> {
    input: &'a AuthorityInput,
    lease_id: String,
    scope_key: String,
    armed: bool,
}

impl<'a> RunGuard<'a> {
    fn new(input: &'a AuthorityInput, lease_id: String, scope_key: String) -> Self {
        Self {
            input,
            lease_id,
            scope_key,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = ledger_op(
                self.input,
                "release_claim",
                json!({"scope_key": self.scope_key}),
            );
            let _ = ledger_op(
                self.input,
                "release_lease",
                json!({"lease_id": self.lease_id}),
            );
        }
    }
}

fn validate_base_input(input: &AuthorityInput) -> Result<(), (Vec<Value>, String)> {
    let mut issues = Vec::new();
    if input.schema_version != INPUT_SCHEMA_VERSION {
        issues.push(issue(
            "input_invalid",
            format!("schema_version must be {INPUT_SCHEMA_VERSION}"),
        ));
    }
    if input.controller_id.is_empty() {
        issues.push(issue("controller_id_missing", "controller_id is required"));
    }
    if !matches!(input.execution_kind.as_str(), "shadow" | "live") {
        issues.push(issue(
            "execution_kind_invalid",
            "execution_kind must be shadow or live",
        ));
    }
    if input.now.is_empty() {
        issues.push(issue("now_missing", "now is required"));
    }
    if !AUTHORITY_MODES.contains(&input.authority_mode.as_str()) {
        issues.push(issue(
            "authority_mode_invalid",
            format!("authority_mode must be one of {AUTHORITY_MODES:?}"),
        ));
    }
    if let Err(message) = resolve_lease_ttl_seconds(input.lease_ttl_seconds) {
        issues.push(issue("lease_ttl_invalid", message));
    }
    if let Err(message) = resolve_cli_timeout_ms(input.cli_timeout_ms) {
        issues.push(issue("cli_timeout_invalid", message));
    }
    if !issues.is_empty() {
        return Err((issues, "input_invalid".into()));
    }
    Ok(())
}

fn predecessor_mode(target_mode: &str) -> Option<&'static str> {
    AUTHORITY_PREDECESSORS
        .iter()
        .find_map(|(mode, pred)| (*mode == target_mode).then_some(*pred))
}

fn evaluate_promotion(
    input: &AuthorityInput,
    target_mode: &str,
) -> Result<(bool, Value), (Vec<Value>, String)> {
    let gate = gate_for_mode(target_mode).ok_or_else(|| {
        (
            vec![issue(
                "target_mode_invalid",
                "target_mode must be observer, safe_writer, queue, or todo_runner",
            )],
            "target_mode_invalid".into(),
        )
    })?;
    let predecessor = predecessor_mode(target_mode).ok_or_else(|| {
        (
            vec![issue("target_mode_invalid", "unknown promotion target")],
            "target_mode_invalid".into(),
        )
    })?;
    let authority = ledger_op(input, "get_authority_mode", json!({}));
    if authority["valid"] != json!(true) {
        return Err((
            vec![issue(
                "authority_lookup_failed",
                "could not load authority mode",
            )],
            "authority_lookup_failed".into(),
        ));
    }
    let stored_mode = authority["data"]["authority_mode"]
        .as_str()
        .unwrap_or("shadow");
    if stored_mode != predecessor {
        return Err((
            vec![issue(
                "predecessor_chain_failed",
                format!("stored authority {stored_mode} != required predecessor {predecessor}"),
            )],
            "predecessor_chain_failed".into(),
        ));
    }
    let since = since_ts(&input.now, gate.min_days)
        .map_err(|message| (vec![issue("now_invalid", message)], "now_invalid".into()))?;
    let metrics = ledger_op(
        input,
        "get_promotion_metrics",
        json!({
            "target_mode": target_mode,
            "since_ts": since,
            "until_ts": input.now,
        }),
    );
    if metrics["valid"] != json!(true) {
        return Err((
            vec![issue(
                "promotion_metrics_failed",
                "could not load promotion metrics",
            )],
            "promotion_metrics_failed".into(),
        ));
    }
    let data = &metrics["data"];
    let decision_count = data["decision_count"].as_i64().unwrap_or(0);
    let hard_gate_parity_pct = data["hard_gate_parity_pct"].as_i64().unwrap_or(0);
    let unauthorized_writes = data["unauthorized_writes"].as_i64().unwrap_or(0);
    let duplicate_mutations = data["duplicate_mutations"].as_i64().unwrap_or(0);
    let duplicate_starts = data["duplicate_starts"].as_i64().unwrap_or(0);
    let pass = decision_count >= gate.min_decisions
        && hard_gate_parity_pct >= gate.min_hard_gate_parity_pct
        && unauthorized_writes <= gate.max_unauthorized_writes
        && duplicate_mutations <= gate.max_duplicate_mutations
        && duplicate_starts <= gate.max_duplicate_starts;
    Ok((
        pass,
        json!({
            "target_mode": target_mode,
            "required_predecessor": predecessor,
            "stored_authority_mode": stored_mode,
            "gate": {
                "min_days": gate.min_days,
                "min_decisions": gate.min_decisions,
                "min_hard_gate_parity_pct": gate.min_hard_gate_parity_pct,
                "max_unauthorized_writes": gate.max_unauthorized_writes,
                "max_duplicate_mutations": gate.max_duplicate_mutations,
                "max_duplicate_starts": gate.max_duplicate_starts,
            },
            "metrics": data,
            "promotion_ready": pass,
        }),
    ))
}

fn run_cursor_recovery(
    input: &AuthorityInput,
    recovery: &CursorRecoveryInput,
    runner: &dyn ProcessRunner,
) -> Value {
    let operation = input.operation.as_str();
    if !CURSOR_FAILURE_CLASSES.contains(&recovery.failure_class.as_str()) {
        return compact(
            operation,
            false,
            "cursor_recovery_failure_class_excluded",
            vec![issue(
                "cursor_recovery_failure_class_excluded",
                "only availability_bridge_failure qualifies",
            )],
            false,
            json!(null),
            json!(null),
            json!(null),
        );
    }
    if !CURSOR_HEALTH_TRANSITIONS.contains(&recovery.health_transition.as_str()) {
        return compact(
            operation,
            false,
            "cursor_recovery_health_transition_excluded",
            vec![issue(
                "cursor_recovery_health_transition_excluded",
                "health transition must be down_to_healthy",
            )],
            false,
            json!(null),
            json!(null),
            json!(null),
        );
    }
    if !recovery.no_artifacts || !recovery.same_issue {
        return compact(
            operation,
            false,
            "cursor_recovery_precondition_failed",
            vec![issue(
                "cursor_recovery_precondition_failed",
                "no_artifacts and same_issue must be true",
            )],
            false,
            json!(null),
            json!(null),
            json!(null),
        );
    }

    const REQUIRED_CURSOR_RECOVERY_MODE: &str = "todo_runner";
    let requested_mode = input.authority_mode.as_str();
    if requested_mode != REQUIRED_CURSOR_RECOVERY_MODE {
        let decision_hash = sha256_hex(&json!({
            "requested_mode": requested_mode,
            "required_mode": REQUIRED_CURSOR_RECOVERY_MODE,
            "operation": "cursor_recovery",
        }));
        let violation = ledger_op(
            input,
            "record_violation",
            json!({
                "event_id": format!("unauth-{decision_hash}"),
                "authority_mode": requested_mode,
                "violation_type": "unauthorized_write",
                "decision_hash": decision_hash,
            }),
        );
        return compact(
            operation,
            false,
            "unauthorized_operation",
            vec![issue(
                "unauthorized_operation",
                format!("cursor_recovery requires {REQUIRED_CURSOR_RECOVERY_MODE}"),
            )],
            false,
            json!(null),
            json!({"violation": violation}),
            json!(null),
        );
    }

    let authority = ledger_op(input, "get_authority_mode", json!({}));
    if authority["valid"] != json!(true) {
        return compact(
            operation,
            false,
            "authority_lookup_failed",
            vec![issue("authority_lookup_failed", "authority lookup failed")],
            false,
            json!(null),
            authority,
            json!(null),
        );
    }
    let stored_mode = authority["data"]["authority_mode"]
        .as_str()
        .unwrap_or("shadow");
    if stored_mode != REQUIRED_CURSOR_RECOVERY_MODE {
        let decision_hash = sha256_hex(&json!({
            "stored_mode": stored_mode,
            "required_mode": REQUIRED_CURSOR_RECOVERY_MODE,
            "operation": "cursor_recovery",
        }));
        let violation = ledger_op(
            input,
            "record_violation",
            json!({
                "event_id": format!("unauth-{decision_hash}"),
                "authority_mode": requested_mode,
                "violation_type": "unauthorized_write",
                "decision_hash": decision_hash,
            }),
        );
        return compact(
            operation,
            false,
            "authority_mode_mismatch",
            vec![issue(
                "authority_mode_mismatch",
                format!(
                    "ledger authority {stored_mode} != required {REQUIRED_CURSOR_RECOVERY_MODE}"
                ),
            )],
            false,
            json!(null),
            json!({"authority": authority, "violation": violation}),
            json!(null),
        );
    }

    let lease_ttl_seconds = resolve_lease_ttl_seconds(input.lease_ttl_seconds).unwrap();
    let expires_at = match lease_expires_at(&input.now, lease_ttl_seconds) {
        Ok(ts) => ts,
        Err(message) => {
            return compact(
                operation,
                false,
                "now_invalid",
                vec![issue("now_invalid", message)],
                false,
                json!(null),
                json!(null),
                json!(null),
            );
        }
    };

    let init = ledger_op(input, "init", json!({}));
    if init["valid"] != json!(true) {
        return compact(
            operation,
            false,
            "ledger_init_failed",
            vec![issue("ledger_init_failed", "ledger init failed")],
            false,
            json!(null),
            init,
            json!(null),
        );
    }

    let lease_id = input
        .lease_id
        .clone()
        .unwrap_or_else(|| format!("{}-cursor-{}", input.controller_id, recovery.issue_id));
    let scope_key = format!("cursor_recovery:{}", recovery.issue_id);

    let lease = ledger_op(
        input,
        "acquire_lease",
        json!({
            "lease_id": lease_id,
            "holder": input.controller_id,
            "expires_at": expires_at,
        }),
    );
    if lease["valid"] != json!(true) {
        let reason = lease["exit_reason"]
            .as_str()
            .unwrap_or("lease_acquire_failed")
            .to_string();
        return compact(
            operation,
            false,
            &reason,
            vec![issue(&reason, "could not acquire schedule lease")],
            false,
            json!(null),
            lease,
            json!(null),
        );
    }

    let claim = ledger_op(
        input,
        "claim_scope",
        json!({
            "scope_key": scope_key,
            "claim_id": format!("claim-{lease_id}"),
            "holder": input.controller_id,
            "expires_at": expires_at,
        }),
    );
    if claim["valid"] != json!(true) {
        let reason = claim["exit_reason"]
            .as_str()
            .unwrap_or("scope_claim_failed")
            .to_string();
        let _ = ledger_op(input, "release_lease", json!({"lease_id": lease_id}));
        return compact(
            operation,
            false,
            &reason,
            vec![issue(&reason, "could not claim scope")],
            false,
            json!(null),
            claim,
            json!(null),
        );
    }

    let mut guard = RunGuard::new(input, lease_id.clone(), scope_key.clone());

    let retry_key = format!("cursor_retry:{}", recovery.issue_id);
    let prior = ledger_op(
        input,
        "get_watermark",
        json!({
            "scope_key": input.controller_id,
            "watermark_key": retry_key,
        }),
    );
    if prior["valid"] == json!(true) && prior["data"]["found"] == json!(true) {
        return compact(
            operation,
            false,
            "cursor_recovery_retry_already_used",
            vec![issue(
                "cursor_recovery_retry_already_used",
                "exactly one retry allowed per issue",
            )],
            false,
            json!(null),
            prior,
            json!(null),
        );
    }
    let decision_hash = sha256_hex(&json!({
        "controller_id": input.controller_id,
        "issue_id": recovery.issue_id,
        "operation": "cursor_recovery_rerun",
    }));
    let idempotency = ledger_op(
        input,
        "claim_idempotency",
        json!({"decision_hash": decision_hash}),
    );
    if idempotency["exit_reason"] == "duplicate_suppressed" {
        guard.disarm();
        let _ = ledger_op(input, "release_claim", json!({"scope_key": scope_key}));
        let _ = ledger_op(input, "release_lease", json!({"lease_id": lease_id}));
        return compact(
            operation,
            false,
            "duplicate_suppressed",
            vec![issue(
                "duplicate_suppressed",
                "duplicate cursor recovery suppressed",
            )],
            false,
            json!(null),
            json!({"idempotency": idempotency}),
            json!(null),
        );
    }
    if idempotency["valid"] != json!(true) {
        return compact(
            operation,
            false,
            "cursor_recovery_idempotency_failed",
            vec![issue(
                "cursor_recovery_idempotency_failed",
                idempotency["exit_reason"]
                    .as_str()
                    .unwrap_or("idempotency claim failed or duplicate"),
            )],
            false,
            json!(null),
            idempotency,
            json!(null),
        );
    }
    let cli_timeout_ms =
        resolve_cli_timeout_ms(input.cli_timeout_ms).unwrap_or(DEFAULT_CLI_TIMEOUT_MS);
    let cli_input = json!({
        "schema_version": "multica-cli-adapter-input.v0",
        "operation": "invoke",
        "cli_path": input.cli_path,
        "timeout_ms": cli_timeout_ms,
        "multica_operation": "cursor_recovery_rerun",
        "operation_params": json!({"issue_id": recovery.issue_id}),
    });
    let cli_raw = run_multica_cli_adapter(&cli_input, runner);
    let cli = redact_cli_summary(&cli_raw);
    let cli_ok = cli["valid"] == json!(true);
    let watermark = ledger_op(
        input,
        "set_watermark",
        json!({
            "scope_key": input.controller_id,
            "watermark_key": retry_key,
            "value_hash": decision_hash,
        }),
    );
    let output_hash = sha256_hex(&cli);
    let decision = ledger_op(
        input,
        "record_decision",
        json!({
            "decision_id": format!("cursor-recovery-{}", recovery.issue_id),
            "authority_mode": input.authority_mode,
            "decision_code": "cursor_recovery_rerun",
            "result_code": cli["exit_reason"].as_str().unwrap_or("unknown"),
            "input_hash": decision_hash,
            "output_hash": output_hash,
            "hard_gate_pass": cli_ok,
        }),
    );
    guard.disarm();
    let _ = ledger_op(input, "release_claim", json!({"scope_key": scope_key}));
    let _ = ledger_op(input, "release_lease", json!({"lease_id": lease_id}));
    let exit_reason = if cli_ok {
        "cursor_recovery_rerun_ok".to_string()
    } else {
        cli["exit_reason"]
            .as_str()
            .unwrap_or("cursor_recovery_rerun_failed")
            .to_string()
    };
    compact(
        operation,
        cli_ok,
        &exit_reason,
        if cli_ok {
            Vec::new()
        } else {
            vec![issue(&exit_reason, "cursor recovery rerun failed")]
        },
        false,
        cli,
        json!({"claim": idempotency, "watermark": watermark, "decision": decision}),
        json!({
            "issue_id": recovery.issue_id,
            "retry_consumed": true,
        }),
    )
}

fn acquire_lease_scope(input: &AuthorityInput) -> Result<(RunGuard<'_>, Value), Value> {
    let operation = input.operation.as_str();
    let lease_ttl_seconds = resolve_lease_ttl_seconds(input.lease_ttl_seconds).unwrap();
    let expires_at = lease_expires_at(&input.now, lease_ttl_seconds).map_err(|message| {
        compact(
            operation,
            false,
            "now_invalid",
            vec![issue("now_invalid", message)],
            false,
            json!(null),
            json!(null),
            json!(null),
        )
    })?;

    let init = ledger_op(input, "init", json!({}));
    if init["valid"] != json!(true) {
        return Err(compact(
            operation,
            false,
            "ledger_init_failed",
            vec![issue("ledger_init_failed", "ledger init failed")],
            false,
            json!(null),
            init,
            json!(null),
        ));
    }

    let lease_id = input
        .lease_id
        .clone()
        .unwrap_or_else(|| format!("{}-{}", input.controller_id, input.now));
    let scope_key = input
        .scope_key
        .clone()
        .unwrap_or_else(|| input.controller_id.clone());

    let lease = ledger_op(
        input,
        "acquire_lease",
        json!({
            "lease_id": lease_id,
            "holder": input.controller_id,
            "expires_at": expires_at,
        }),
    );
    if lease["valid"] != json!(true) {
        let reason = lease["exit_reason"]
            .as_str()
            .unwrap_or("lease_acquire_failed")
            .to_string();
        return Err(compact(
            operation,
            false,
            &reason,
            vec![issue(&reason, "could not acquire schedule lease")],
            false,
            json!(null),
            lease,
            json!(null),
        ));
    }

    let claim = ledger_op(
        input,
        "claim_scope",
        json!({
            "scope_key": scope_key,
            "claim_id": format!("claim-{lease_id}"),
            "holder": input.controller_id,
            "expires_at": expires_at,
        }),
    );
    if claim["valid"] != json!(true) {
        let reason = claim["exit_reason"]
            .as_str()
            .unwrap_or("scope_claim_failed")
            .to_string();
        let _ = ledger_op(input, "release_lease", json!({"lease_id": lease_id}));
        return Err(compact(
            operation,
            false,
            &reason,
            vec![issue(&reason, "could not claim scope")],
            false,
            json!(null),
            claim,
            json!(null),
        ));
    }

    Ok((RunGuard::new(input, lease_id, scope_key), json!(null)))
}

fn run_once(input: &AuthorityInput, runner: &dyn ProcessRunner) -> Value {
    let operation = input.operation.as_str();
    if input.authority_mode == "shadow" {
        return compact(
            operation,
            false,
            "authority_shadow_noop",
            vec![issue(
                "authority_shadow_noop",
                "shadow mode performs no run",
            )],
            false,
            json!(null),
            json!(null),
            json!(null),
        );
    }

    let mode = input.authority_mode.as_str();
    let execution_kind = input.execution_kind.as_str();
    let is_observer = mode == "observer";
    let multica_operation = input.multica_operation.as_deref();
    if is_observer && multica_operation.is_some() {
        return compact(
            operation,
            false,
            "unauthorized_operation",
            vec![issue(
                "unauthorized_operation",
                "observer mode is read-only",
            )],
            false,
            json!(null),
            json!(null),
            json!(null),
        );
    }
    if !is_observer {
        let Some(op) = multica_operation else {
            return compact(
                operation,
                false,
                "multica_operation_missing",
                vec![issue(
                    "multica_operation_missing",
                    "mutation modes require multica_operation",
                )],
                false,
                json!(null),
                json!(null),
                json!(null),
            );
        };
        if !allowed_multica_operation(mode, op) {
            let decision_hash = sha256_hex(&json!({"mode": mode, "multica_operation": op}));
            let violation = ledger_op(
                input,
                "record_violation",
                json!({
                    "event_id": format!("unauth-{decision_hash}"),
                    "authority_mode": mode,
                    "violation_type": "unauthorized_write",
                    "decision_hash": decision_hash,
                }),
            );
            return compact(
                operation,
                false,
                "unauthorized_operation",
                vec![issue(
                    "unauthorized_operation",
                    format!("{op} is not allowed in {mode}"),
                )],
                false,
                json!(null),
                json!({"violation": violation}),
                json!(null),
            );
        }
    }

    let mut guard = match acquire_lease_scope(input) {
        Ok((guard, _)) => guard,
        Err(output) => return output,
    };

    let authority = ledger_op(input, "get_authority_mode", json!({}));
    if authority["valid"] != json!(true) {
        return compact(
            operation,
            false,
            "authority_lookup_failed",
            vec![issue("authority_lookup_failed", "authority lookup failed")],
            false,
            json!(null),
            authority,
            json!(null),
        );
    }
    let stored_mode = authority["data"]["authority_mode"]
        .as_str()
        .unwrap_or("shadow");
    if execution_kind == "shadow" {
        let Some(predecessor) = predecessor_mode(mode) else {
            return compact(
                operation,
                false,
                "predecessor_chain_failed",
                vec![issue(
                    "predecessor_chain_failed",
                    format!("no promotion predecessor for target mode {mode}"),
                )],
                false,
                json!(null),
                json!({"authority": authority}),
                json!(null),
            );
        };
        if stored_mode != predecessor {
            return compact(
                operation,
                false,
                "predecessor_chain_failed",
                vec![issue(
                    "predecessor_chain_failed",
                    format!("stored authority {stored_mode} != required predecessor {predecessor}"),
                )],
                false,
                json!(null),
                json!({"authority": authority}),
                json!(null),
            );
        }
    } else if stored_mode != mode {
        let decision_hash =
            sha256_hex(&json!({"stored_mode": stored_mode, "requested_mode": mode}));
        let violation = ledger_op(
            input,
            "record_violation",
            json!({
                "event_id": format!("unauth-{decision_hash}"),
                "authority_mode": mode,
                "violation_type": "unauthorized_write",
                "decision_hash": decision_hash,
            }),
        );
        return compact(
            operation,
            false,
            "authority_mode_mismatch",
            vec![issue(
                "authority_mode_mismatch",
                format!("ledger authority {stored_mode} != requested {mode}"),
            )],
            false,
            json!(null),
            json!({"authority": authority, "violation": violation}),
            json!(null),
        );
    }

    let cli_timeout_ms = resolve_cli_timeout_ms(input.cli_timeout_ms).unwrap();
    let cli_input = if is_observer {
        json!({
            "schema_version": "multica-cli-adapter-input.v0",
            "operation": "query",
            "cli_path": input.cli_path,
            "timeout_ms": cli_timeout_ms,
        })
    } else {
        json!({
            "schema_version": "multica-cli-adapter-input.v0",
            "operation": "invoke",
            "cli_path": input.cli_path,
            "timeout_ms": cli_timeout_ms,
            "multica_operation": multica_operation,
            "operation_params": input.operation_params.clone().unwrap_or(json!({})),
        })
    };

    let decision_hash = sha256_hex(&cli_input);
    let idempotency = ledger_op(
        input,
        "claim_idempotency",
        json!({"decision_hash": decision_hash}),
    );
    if idempotency["exit_reason"] == "duplicate_suppressed" {
        let violation = ledger_op(
            input,
            "record_violation",
            json!({
                "event_id": format!("dup-{}", decision_hash),
                "authority_mode": mode,
                "violation_type": match mode {
                    "queue" => "duplicate_mutation",
                    "todo_runner" => "duplicate_start",
                    _ => "duplicate_mutation",
                },
                "decision_hash": decision_hash,
            }),
        );
        guard.disarm();
        let _ = ledger_op(
            input,
            "release_claim",
            json!({"scope_key": guard.scope_key.clone()}),
        );
        let _ = ledger_op(
            input,
            "release_lease",
            json!({"lease_id": guard.lease_id.clone()}),
        );
        return compact(
            operation,
            false,
            "duplicate_suppressed",
            vec![issue(
                "duplicate_suppressed",
                "duplicate mutation suppressed",
            )],
            false,
            json!(null),
            json!({"idempotency": idempotency, "violation": violation}),
            json!(null),
        );
    }

    if idempotency["valid"] != json!(true) {
        return compact(
            operation,
            false,
            "idempotency_claim_failed",
            vec![issue(
                "idempotency_claim_failed",
                idempotency["exit_reason"]
                    .as_str()
                    .unwrap_or("idempotency claim invalid"),
            )],
            false,
            json!(null),
            idempotency,
            json!(null),
        );
    }

    if execution_kind == "shadow" {
        let output_hash = sha256_hex(&json!({"shadow_evidence": true, "mode": mode}));
        let input_hash = sha256_hex(&cli_input);
        let decision_id = format!("decision-shadow-{}", guard.lease_id);

        let decision = ledger_op(
            input,
            "record_decision",
            json!({
                "decision_id": decision_id,
                "authority_mode": mode,
                "decision_code": format!("{mode}_shadow_run_once"),
                "result_code": "shadow_evidence_ok",
                "input_hash": input_hash,
                "output_hash": output_hash,
                "hard_gate_pass": true,
            }),
        );

        let watermark_key = if is_observer {
            "last_observer_run".to_string()
        } else {
            format!("last_{mode}_run")
        };
        let watermark = ledger_op(
            input,
            "set_watermark",
            json!({
                "scope_key": guard.scope_key.clone(),
                "watermark_key": watermark_key,
                "value_hash": output_hash,
            }),
        );

        guard.disarm();
        let _ = ledger_op(
            input,
            "release_claim",
            json!({"scope_key": guard.scope_key.clone()}),
        );
        let _ = ledger_op(
            input,
            "release_lease",
            json!({"lease_id": guard.lease_id.clone()}),
        );

        if decision["valid"] != json!(true) || watermark["valid"] != json!(true) {
            return compact(
                operation,
                false,
                "decision_record_failed",
                vec![issue("decision_record_failed", "ledger persistence failed")],
                false,
                json!(null),
                json!({"decision": decision, "watermark": watermark, "authority": authority}),
                json!(null),
            );
        }

        let exit_reason = if is_observer {
            "observer_success_no_mutation".to_string()
        } else {
            format!("{mode}_shadow_evidence_ok")
        };

        return compact(
            operation,
            true,
            &exit_reason,
            Vec::new(),
            false,
            json!(null),
            json!({
                "decision": decision,
                "watermark": watermark,
                "authority": authority,
                "idempotency": idempotency,
            }),
            json!({
                "authority_mode": mode,
                "multica_operation": multica_operation,
                "query_args": if is_observer { Some(QUERY_OPERATION_ARGS) } else { None },
            }),
        );
    }

    let cli_raw = run_multica_cli_adapter(&cli_input, runner);
    let cli = redact_cli_summary(&cli_raw);
    let mutation_performed = !is_observer && cli["valid"] == json!(true);

    let input_hash = sha256_hex(&cli_input);
    let output_hash = sha256_hex(&cli);
    let decision_id = format!("decision-{}", guard.lease_id);
    let result_code = cli["exit_reason"].as_str().unwrap_or("unknown");
    let hard_gate_pass = if is_observer {
        cli["valid"] == json!(true)
    } else {
        cli["valid"] == json!(true)
    };
    let decision = ledger_op(
        input,
        "record_decision",
        json!({
            "decision_id": decision_id,
            "authority_mode": mode,
            "decision_code": format!("{mode}_run_once"),
            "result_code": result_code,
            "input_hash": input_hash,
            "output_hash": output_hash,
            "hard_gate_pass": hard_gate_pass,
        }),
    );

    let watermark_key = if is_observer {
        "last_observer_run".to_string()
    } else {
        format!("last_{mode}_run")
    };
    let watermark = ledger_op(
        input,
        "set_watermark",
        json!({
            "scope_key": guard.scope_key.clone(),
            "watermark_key": watermark_key,
            "value_hash": output_hash,
        }),
    );

    guard.disarm();
    let _ = ledger_op(
        input,
        "release_claim",
        json!({"scope_key": guard.scope_key.clone()}),
    );
    let _ = ledger_op(
        input,
        "release_lease",
        json!({"lease_id": guard.lease_id.clone()}),
    );

    if decision["valid"] != json!(true) || watermark["valid"] != json!(true) {
        return compact(
            operation,
            false,
            "decision_record_failed",
            vec![issue("decision_record_failed", "ledger persistence failed")],
            mutation_performed,
            cli,
            json!({"decision": decision, "watermark": watermark, "authority": authority}),
            json!(null),
        );
    }

    let exit_reason = if is_observer {
        if cli["valid"] == json!(true) {
            "observer_success_no_mutation".to_string()
        } else {
            cli["exit_reason"]
                .as_str()
                .unwrap_or("cli_failed")
                .to_string()
        }
    } else if cli["valid"] == json!(true) {
        format!("{mode}_mutation_ok")
    } else {
        cli["exit_reason"]
            .as_str()
            .unwrap_or("cli_failed")
            .to_string()
    };
    let valid = if is_observer {
        exit_reason == "observer_success_no_mutation"
    } else {
        exit_reason.ends_with("_mutation_ok")
    };

    compact(
        operation,
        valid,
        &exit_reason,
        if valid {
            Vec::new()
        } else {
            vec![issue(
                &exit_reason,
                "authority run did not complete cleanly",
            )]
        },
        mutation_performed,
        cli,
        json!({
            "decision": decision,
            "watermark": watermark,
            "authority": authority,
            "idempotency": idempotency,
        }),
        json!({
            "authority_mode": mode,
            "multica_operation": multica_operation,
            "query_args": if is_observer { Some(QUERY_OPERATION_ARGS) } else { None },
        }),
    )
}

/// Run production authority wiring with injectable CLI runner for tests.
pub fn run_production_authority(value: &Value, runner: &dyn ProcessRunner) -> Value {
    let Ok(input) = serde_json::from_value::<AuthorityInput>(value.clone()) else {
        return compact(
            "run_once",
            false,
            "input_invalid",
            vec![issue(
                "input_invalid",
                "input must match production-authority-input.v0",
            )],
            false,
            json!(null),
            json!(null),
            json!(null),
        );
    };

    let operation = input.operation.as_str();
    if let Err((issues, reason)) = validate_base_input(&input) {
        return compact(
            operation,
            false,
            &reason,
            issues,
            false,
            json!(null),
            json!(null),
            json!(null),
        );
    }

    match operation {
        "run_once" => run_once(&input, runner),
        "check_promotion" => {
            let target_mode = match input.target_mode.as_deref() {
                Some(mode) => mode,
                None => {
                    return compact(
                        operation,
                        false,
                        "target_mode_missing",
                        vec![issue("target_mode_missing", "target_mode is required")],
                        false,
                        json!(null),
                        json!(null),
                        json!(null),
                    );
                }
            };
            match evaluate_promotion(&input, target_mode) {
                Ok((pass, report)) => compact(
                    operation,
                    pass,
                    if pass {
                        "promotion_ready"
                    } else {
                        "promotion_not_ready"
                    },
                    if pass {
                        Vec::new()
                    } else {
                        vec![issue("promotion_not_ready", "promotion gate not satisfied")]
                    },
                    false,
                    json!(null),
                    json!(null),
                    report,
                ),
                Err((issues, reason)) => compact(
                    operation,
                    false,
                    &reason,
                    issues,
                    false,
                    json!(null),
                    json!(null),
                    json!(null),
                ),
            }
        }
        "cursor_recovery" => {
            let Some(recovery) = input.cursor_recovery.as_ref() else {
                return compact(
                    operation,
                    false,
                    "cursor_recovery_missing",
                    vec![issue(
                        "cursor_recovery_missing",
                        "cursor_recovery block is required",
                    )],
                    false,
                    json!(null),
                    json!(null),
                    json!(null),
                );
            };
            run_cursor_recovery(&input, recovery, runner)
        }
        _ => compact(
            operation,
            false,
            "unknown_operation",
            vec![issue(
                "unknown_operation",
                "operation must be run_once, check_promotion, or cursor_recovery",
            )],
            false,
            json!(null),
            json!(null),
            json!(null),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentmesh_local_control_ledger::run_local_control_ledger;
    use agentmesh_multica_cli_adapter::{CliCommandSpec, CliInvokeResult};
    use chrono::{DateTime, Duration, FixedOffset};
    use std::fs;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct FakeRunner {
        exit_code: i32,
        stdout: Vec<u8>,
        expected_args: Option<Vec<String>>,
    }

    impl ProcessRunner for FakeRunner {
        fn run(
            &self,
            _spec: &CliCommandSpec,
            operation_args: &[String],
            _timeout_ms: u64,
        ) -> Result<CliInvokeResult, String> {
            if let Some(expected) = &self.expected_args {
                assert_eq!(operation_args, expected);
            }
            let bounded = self.stdout.as_slice();
            Ok(CliInvokeResult {
                exit_code: self.exit_code,
                stdout_json: serde_json::from_slice(bounded).ok(),
                stdout_sha256: format!("sha256:{}", hex::encode(Sha256::digest(bounded))),
                stdout_byte_count: bounded.len(),
                stdout_truncated: false,
                stderr_byte_count: 0,
                timed_out: false,
            })
        }
    }

    struct CountedFakeRunner {
        calls: Arc<AtomicUsize>,
        exit_code: i32,
        stdout: Vec<u8>,
    }

    impl ProcessRunner for CountedFakeRunner {
        fn run(
            &self,
            _spec: &CliCommandSpec,
            _operation_args: &[String],
            _timeout_ms: u64,
        ) -> Result<CliInvokeResult, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let bounded = self.stdout.as_slice();
            Ok(CliInvokeResult {
                exit_code: self.exit_code,
                stdout_json: serde_json::from_slice(bounded).ok(),
                stdout_sha256: format!("sha256:{}", hex::encode(Sha256::digest(bounded))),
                stdout_byte_count: bounded.len(),
                stdout_truncated: false,
                stderr_byte_count: 0,
                timed_out: false,
            })
        }
    }

    fn base_input(dir: &tempfile::TempDir, mode: &str) -> Value {
        let cli = dir.path().join("multica.exe");
        fs::write(&cli, b"fake").unwrap();
        let ledger = dir.path().join("control.db");
        json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": "run_once",
            "controller_id": "workflow_audit",
            "execution_kind": "live",
            "authority_mode": mode,
            "ledger_path": ledger.to_string_lossy(),
            "cli_path": cli.canonicalize().unwrap().to_string_lossy(),
            "now": "2026-08-30T12:00:00+09:00",
        })
    }

    fn init_authority(path: &str, mode: &str) {
        run_local_control_ledger(&json!({
            "schema_version": "local-control-ledger-input.v0",
            "operation": "init",
            "ledger_path": path,
        }));
        run_local_control_ledger(&json!({
            "schema_version": "local-control-ledger-input.v0",
            "operation": "set_authority_mode",
            "ledger_path": path,
            "controller_id": "workflow_audit",
            "authority_mode": mode,
            "updated_at": "2026-08-30T12:00:00+09:00",
        }));
    }

    #[test]
    fn observer_run_once_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let input = base_input(&dir, "observer");
        init_authority(input["ledger_path"].as_str().unwrap(), "observer");
        let output = run_production_authority(
            &input,
            &FakeRunner {
                exit_code: 0,
                stdout: br#"{"issues":[]}"#.to_vec(),
                expected_args: Some(vec!["issues".into(), "list".into(), "--json".into()]),
            },
        );
        assert_eq!(output["valid"], json!(true));
        assert_eq!(output["mutation_performed"], json!(false));
    }

    #[test]
    fn safe_writer_uses_allowed_done_reconcile_argv() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = base_input(&dir, "safe_writer");
        init_authority(input["ledger_path"].as_str().unwrap(), "safe_writer");
        input["multica_operation"] = json!("safe_writer_done_reconcile");
        input["operation_params"] = json!({"issue_id": "AM-42"});
        let output = run_production_authority(
            &input,
            &FakeRunner {
                exit_code: 0,
                stdout: br#"{"ok":true}"#.to_vec(),
                expected_args: Some(vec![
                    "issue".into(),
                    "update".into(),
                    "AM-42".into(),
                    "--status".into(),
                    "done".into(),
                    "--no-start".into(),
                    "--output".into(),
                    "json".into(),
                ]),
            },
        );
        assert_eq!(output["valid"], json!(true));
        assert_eq!(output["mutation_performed"], json!(true));
    }

    #[test]
    fn rejects_unauthorized_operation_for_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = base_input(&dir, "queue");
        init_authority(input["ledger_path"].as_str().unwrap(), "queue");
        input["multica_operation"] = json!("safe_writer_done_reconcile");
        input["operation_params"] = json!({"issue_id": "AM-42"});
        let output = run_production_authority(
            &input,
            &FakeRunner {
                exit_code: 0,
                stdout: br#"{}"#.to_vec(),
                expected_args: None,
            },
        );
        assert_eq!(output["exit_reason"], json!("unauthorized_operation"));
    }

    #[test]
    fn promotion_boundary_requires_minimum_decisions() {
        let dir = tempfile::tempdir().unwrap();
        let input = base_input(&dir, "observer");
        init_authority(input["ledger_path"].as_str().unwrap(), "shadow");
        let mut check = input.clone();
        check["operation"] = json!("check_promotion");
        check["target_mode"] = json!("observer");
        let output = run_production_authority(
            &check,
            &FakeRunner {
                exit_code: 0,
                stdout: vec![],
                expected_args: None,
            },
        );
        assert_eq!(output["exit_reason"], json!("promotion_not_ready"));
    }

    #[test]
    fn cursor_recovery_different_issues_each_invoke_runner_once() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = CountedFakeRunner {
            calls: calls.clone(),
            exit_code: 0,
            stdout: br#"{"ok":true}"#.to_vec(),
        };
        for issue_id in ["AM-99", "AM-100"] {
            let mut input = base_input(&dir, "todo_runner");
            init_authority(input["ledger_path"].as_str().unwrap(), "todo_runner");
            input["operation"] = json!("cursor_recovery");
            input["cursor_recovery"] = json!({
                "issue_id": issue_id,
                "failure_class": "availability_bridge_failure",
                "health_transition": "down_to_healthy",
                "no_artifacts": true,
                "same_issue": true
            });
            let output = run_production_authority(&input, &runner);
            assert_eq!(output["exit_reason"], json!("cursor_recovery_rerun_ok"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cursor_recovery_idempotency_duplicate_skips_runner() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = base_input(&dir, "todo_runner");
        init_authority(input["ledger_path"].as_str().unwrap(), "todo_runner");
        input["operation"] = json!("cursor_recovery");
        input["cursor_recovery"] = json!({
            "issue_id": "AM-101",
            "failure_class": "availability_bridge_failure",
            "health_transition": "down_to_healthy",
            "no_artifacts": true,
            "same_issue": true
        });
        let decision_hash = sha256_hex(&json!({
            "controller_id": input["controller_id"],
            "issue_id": "AM-101",
            "operation": "cursor_recovery_rerun",
        }));
        run_local_control_ledger(&json!({
            "schema_version": "local-control-ledger-input.v0",
            "operation": "init",
            "ledger_path": input["ledger_path"],
        }));
        run_local_control_ledger(&json!({
            "schema_version": "local-control-ledger-input.v0",
            "operation": "claim_idempotency",
            "ledger_path": input["ledger_path"],
            "controller_id": input["controller_id"],
            "decision_hash": decision_hash,
            "recorded_at": input["now"],
        }));
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = CountedFakeRunner {
            calls: calls.clone(),
            exit_code: 0,
            stdout: br#"{"ok":true}"#.to_vec(),
        };
        let output = run_production_authority(&input, &runner);
        assert_eq!(output["exit_reason"], json!("duplicate_suppressed"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cursor_recovery_allows_one_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = base_input(&dir, "todo_runner");
        init_authority(input["ledger_path"].as_str().unwrap(), "todo_runner");
        input["operation"] = json!("cursor_recovery");
        input["cursor_recovery"] = json!({
            "issue_id": "AM-99",
            "failure_class": "availability_bridge_failure",
            "health_transition": "down_to_healthy",
            "no_artifacts": true,
            "same_issue": true
        });
        let runner = FakeRunner {
            exit_code: 0,
            stdout: br#"{"ok":true}"#.to_vec(),
            expected_args: Some(vec![
                "issue".into(),
                "rerun".into(),
                "AM-99".into(),
                "--output".into(),
                "json".into(),
            ]),
        };
        let first = run_production_authority(&input, &runner);
        assert_eq!(first["exit_reason"], json!("cursor_recovery_rerun_ok"));
        assert_eq!(first["extra"]["retry_consumed"], json!(true));
        let second = run_production_authority(&input, &runner);
        assert_eq!(
            second["exit_reason"],
            json!("cursor_recovery_retry_already_used")
        );
    }

    #[test]
    fn cursor_recovery_failed_rerun_still_consumes_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = base_input(&dir, "todo_runner");
        init_authority(input["ledger_path"].as_str().unwrap(), "todo_runner");
        input["operation"] = json!("cursor_recovery");
        input["cursor_recovery"] = json!({
            "issue_id": "AM-100",
            "failure_class": "availability_bridge_failure",
            "health_transition": "down_to_healthy",
            "no_artifacts": true,
            "same_issue": true
        });
        let runner = FakeRunner {
            exit_code: 1,
            stdout: br#"{"error":"fail"}"#.to_vec(),
            expected_args: Some(vec![
                "issue".into(),
                "rerun".into(),
                "AM-100".into(),
                "--output".into(),
                "json".into(),
            ]),
        };
        let first = run_production_authority(&input, &runner);
        assert_eq!(first["valid"], json!(false));
        assert_eq!(first["extra"]["retry_consumed"], json!(true));
        let second = run_production_authority(&input, &runner);
        assert_eq!(
            second["exit_reason"],
            json!("cursor_recovery_retry_already_used")
        );
    }

    #[test]
    fn wrong_mode_mutation_records_unauthorized_violation() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = base_input(&dir, "queue");
        init_authority(input["ledger_path"].as_str().unwrap(), "safe_writer");
        input["multica_operation"] = json!("queue_backlog_promote");
        input["operation_params"] = json!({"issue_id": "AM-7"});
        let output = run_production_authority(
            &input,
            &FakeRunner {
                exit_code: 0,
                stdout: br#"{}"#.to_vec(),
                expected_args: None,
            },
        );
        assert_eq!(output["exit_reason"], json!("authority_mode_mismatch"));
        assert!(output["ledger"]["violation"]["valid"] == json!(true));
    }

    #[test]
    fn duplicate_mutation_is_suppressed_for_queue() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = base_input(&dir, "queue");
        init_authority(input["ledger_path"].as_str().unwrap(), "queue");
        input["multica_operation"] = json!("queue_backlog_promote");
        input["operation_params"] = json!({"issue_id": "AM-7"});
        let runner = FakeRunner {
            exit_code: 0,
            stdout: br#"{"ok":true}"#.to_vec(),
            expected_args: Some(vec![
                "issue".into(),
                "update".into(),
                "AM-7".into(),
                "--status".into(),
                "todo".into(),
                "--no-start".into(),
                "--output".into(),
                "json".into(),
            ]),
        };
        assert_eq!(
            run_production_authority(&input, &runner)["valid"],
            json!(true)
        );
        let dup = run_production_authority(&input, &runner);
        assert_eq!(dup["exit_reason"], json!("duplicate_suppressed"));
    }

    #[test]
    fn promotion_ready_from_observer_with_shadow_safe_writer_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let base = base_input(&dir, "safe_writer");
        init_authority(base["ledger_path"].as_str().unwrap(), "observer");

        let calls = Arc::new(AtomicUsize::new(0));
        let runner = CountedFakeRunner {
            calls: calls.clone(),
            exit_code: 0,
            stdout: br#"{}"#.to_vec(),
        };

        let base_now: DateTime<FixedOffset> =
            DateTime::parse_from_rfc3339("2026-08-30T12:00:00+09:00").unwrap();
        let since = base_now - Duration::days(7);

        for i in 0..50 {
            let mut input = base_input(&dir, "safe_writer");
            input["execution_kind"] = json!("shadow");
            input["multica_operation"] = json!("safe_writer_done_reconcile");
            input["operation_params"] = json!({"issue_id": format!("AM-{}", i)});
            input["lease_id"] = json!(format!("lease-{}", i));
            input["now"] = json!((since + Duration::days((i * 7) / 49)).to_rfc3339());

            let output = run_production_authority(&input, &runner);
            assert_eq!(output["valid"], json!(true));
            assert_eq!(output["mutation_performed"], json!(false));
        }

        let mut check = base_input(&dir, "observer");
        check["operation"] = json!("check_promotion");
        check["target_mode"] = json!("safe_writer");
        check["execution_kind"] = json!("shadow");
        check["now"] = json!(base_now.to_rfc3339());

        let output = run_production_authority(&check, &runner);
        assert_eq!(output["valid"], json!(true));
        assert_eq!(output["exit_reason"], json!("promotion_ready"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let metrics = &output["extra"]["metrics"];
        assert_eq!(metrics["decision_count"], json!(50));
        assert_eq!(metrics["hard_gate_parity_pct"], json!(100));
        assert_eq!(metrics["unauthorized_writes"], json!(0));
    }

    #[test]
    fn promotion_rejects_insufficient_shadow_safe_writer_duration() {
        let dir = tempfile::tempdir().unwrap();
        let base = base_input(&dir, "safe_writer");
        init_authority(base["ledger_path"].as_str().unwrap(), "observer");

        let calls = Arc::new(AtomicUsize::new(0));
        let runner = CountedFakeRunner {
            calls: calls.clone(),
            exit_code: 0,
            stdout: br#"{}"#.to_vec(),
        };

        let base_now: DateTime<FixedOffset> =
            DateTime::parse_from_rfc3339("2026-08-30T12:00:00+09:00").unwrap();
        let too_old = base_now - Duration::days(8);

        for i in 0..50 {
            let mut input = base_input(&dir, "safe_writer");
            input["execution_kind"] = json!("shadow");
            input["multica_operation"] = json!("safe_writer_done_reconcile");
            input["operation_params"] = json!({"issue_id": format!("AM-{}", i)});
            input["lease_id"] = json!(format!("lease-{}", i));
            input["now"] = json!(too_old.to_rfc3339());

            let output = run_production_authority(&input, &runner);
            assert_eq!(output["valid"], json!(true));
            assert_eq!(output["mutation_performed"], json!(false));
        }

        let mut check = base_input(&dir, "observer");
        check["operation"] = json!("check_promotion");
        check["target_mode"] = json!("safe_writer");
        check["execution_kind"] = json!("shadow");
        check["now"] = json!(base_now.to_rfc3339());

        let output = run_production_authority(&check, &runner);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["exit_reason"], json!("promotion_not_ready"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(output["extra"]["metrics"]["decision_count"], json!(0));
    }

    #[test]
    fn promotion_rejects_insufficient_shadow_safe_writer_count() {
        let dir = tempfile::tempdir().unwrap();
        let base = base_input(&dir, "safe_writer");
        init_authority(base["ledger_path"].as_str().unwrap(), "observer");

        let calls = Arc::new(AtomicUsize::new(0));
        let runner = CountedFakeRunner {
            calls: calls.clone(),
            exit_code: 0,
            stdout: br#"{}"#.to_vec(),
        };

        let base_now: DateTime<FixedOffset> =
            DateTime::parse_from_rfc3339("2026-08-30T12:00:00+09:00").unwrap();
        let since = base_now - Duration::days(7);

        for i in 0..49 {
            let mut input = base_input(&dir, "safe_writer");
            input["execution_kind"] = json!("shadow");
            input["multica_operation"] = json!("safe_writer_done_reconcile");
            input["operation_params"] = json!({"issue_id": format!("AM-{}", i)});
            input["lease_id"] = json!(format!("lease-{}", i));
            input["now"] = json!((since + Duration::days((i * 7) / 48)).to_rfc3339());

            let output = run_production_authority(&input, &runner);
            assert_eq!(output["valid"], json!(true));
            assert_eq!(output["mutation_performed"], json!(false));
        }

        let mut check = base_input(&dir, "observer");
        check["operation"] = json!("check_promotion");
        check["target_mode"] = json!("safe_writer");
        check["execution_kind"] = json!("shadow");
        check["now"] = json!(base_now.to_rfc3339());

        let output = run_production_authority(&check, &runner);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["exit_reason"], json!("promotion_not_ready"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(output["extra"]["metrics"]["decision_count"], json!(49));
    }

    #[test]
    fn promotion_rejects_shadow_safe_writer_hard_gate_parity() {
        let dir = tempfile::tempdir().unwrap();
        let base = base_input(&dir, "safe_writer");
        let ledger_path = base["ledger_path"].as_str().unwrap();
        init_authority(ledger_path, "observer");

        // No ProcessRunner call needed.
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = CountedFakeRunner {
            calls: calls.clone(),
            exit_code: 0,
            stdout: br#"{}"#.to_vec(),
        };

        let base_now: DateTime<FixedOffset> =
            DateTime::parse_from_rfc3339("2026-08-30T12:00:00+09:00").unwrap();
        let since = base_now - Duration::days(7);
        let target_mode = "safe_writer";
        let shadow_decision_code = format!("{target_mode}_shadow_run_once");

        for i in 0..50 {
            let decision_now = since + Duration::days((i * 7) / 49);
            run_local_control_ledger(&json!({
                "schema_version": "local-control-ledger-input.v0",
                "operation": "record_decision",
                "ledger_path": ledger_path,
                "controller_id": "workflow_audit",
                "decision_id": format!("dec-{}", i),
                "authority_mode": target_mode,
                "decision_code": shadow_decision_code.clone(),
                "result_code": "shadow_evidence_ok",
                "input_hash": format!("sha256:in-{}", i),
                "output_hash": format!("sha256:out-{}", i),
                "hard_gate_pass": i != 0,
                "recorded_at": decision_now.to_rfc3339(),
            }));
        }

        let mut check = base_input(&dir, "observer");
        check["operation"] = json!("check_promotion");
        check["target_mode"] = json!(target_mode);
        check["execution_kind"] = json!("shadow");
        check["now"] = json!(base_now.to_rfc3339());

        let output = run_production_authority(&check, &runner);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["exit_reason"], json!("promotion_not_ready"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            output["extra"]["metrics"]["hard_gate_parity_pct"],
            json!(98)
        );
    }

    #[test]
    fn promotion_rejects_unauthorized_write_from_wrong_mode_live_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let base = base_input(&dir, "safe_writer");
        init_authority(base["ledger_path"].as_str().unwrap(), "observer");

        let calls = Arc::new(AtomicUsize::new(0));
        let runner = CountedFakeRunner {
            calls: calls.clone(),
            exit_code: 0,
            stdout: br#"{}"#.to_vec(),
        };

        let base_now: DateTime<FixedOffset> =
            DateTime::parse_from_rfc3339("2026-08-30T12:00:00+09:00").unwrap();
        let since = base_now - Duration::days(7);

        for i in 0..50 {
            let mut input = base_input(&dir, "safe_writer");
            input["execution_kind"] = json!("shadow");
            input["multica_operation"] = json!("safe_writer_done_reconcile");
            input["operation_params"] = json!({"issue_id": format!("AM-{}", i)});
            input["lease_id"] = json!(format!("lease-{}", i));
            input["now"] = json!((since + Duration::days((i * 7) / 49)).to_rfc3339());

            let output = run_production_authority(&input, &runner);
            assert_eq!(output["valid"], json!(true));
            assert_eq!(output["mutation_performed"], json!(false));
        }

        let mut wrong = base_input(&dir, "safe_writer");
        wrong["execution_kind"] = json!("live");
        wrong["multica_operation"] = json!("safe_writer_done_reconcile");
        wrong["operation_params"] = json!({"issue_id": "AM-live-viol"});
        wrong["lease_id"] = json!("lease-wrong");
        wrong["now"] = json!(base_now.to_rfc3339());

        let wrong_out = run_production_authority(&wrong, &runner);
        assert_eq!(wrong_out["valid"], json!(false));
        assert_eq!(wrong_out["exit_reason"], json!("authority_mode_mismatch"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let mut check = base_input(&dir, "observer");
        check["operation"] = json!("check_promotion");
        check["target_mode"] = json!("safe_writer");
        check["execution_kind"] = json!("shadow");
        check["now"] = json!(base_now.to_rfc3339());

        let output = run_production_authority(&check, &runner);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["exit_reason"], json!("promotion_not_ready"));
        assert_eq!(output["extra"]["metrics"]["unauthorized_writes"], json!(1));
    }

    #[test]
    fn ledger_error_blocks_runner_before_live_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = base_input(&dir, "safe_writer");
        input["execution_kind"] = json!("live");
        input["multica_operation"] = json!("safe_writer_done_reconcile");
        input["operation_params"] = json!({"issue_id": "AM-1"});

        // Make ledger path a directory => sqlite open/initialization fails.
        let ledger_path = input["ledger_path"].as_str().unwrap();
        let _ = std::fs::create_dir_all(ledger_path);

        let calls = Arc::new(AtomicUsize::new(0));
        let runner = CountedFakeRunner {
            calls: calls.clone(),
            exit_code: 0,
            stdout: br#"{}"#.to_vec(),
        };

        let output = run_production_authority(&input, &runner);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cursor_recovery_rejects_non_todo_runner_input_mode() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = CountedFakeRunner {
            calls: calls.clone(),
            exit_code: 0,
            stdout: br#"{"ok":true}"#.to_vec(),
        };
        let mut input = base_input(&dir, "queue");
        init_authority(input["ledger_path"].as_str().unwrap(), "queue");
        input["operation"] = json!("cursor_recovery");
        input["cursor_recovery"] = json!({
            "issue_id": "AM-200",
            "failure_class": "availability_bridge_failure",
            "health_transition": "down_to_healthy",
            "no_artifacts": true,
            "same_issue": true
        });
        let output = run_production_authority(&input, &runner);
        assert_eq!(output["exit_reason"], json!("unauthorized_operation"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cursor_recovery_rejects_stored_authority_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = CountedFakeRunner {
            calls: calls.clone(),
            exit_code: 0,
            stdout: br#"{"ok":true}"#.to_vec(),
        };
        let mut input = base_input(&dir, "todo_runner");
        init_authority(input["ledger_path"].as_str().unwrap(), "observer");
        input["operation"] = json!("cursor_recovery");
        input["cursor_recovery"] = json!({
            "issue_id": "AM-201",
            "failure_class": "availability_bridge_failure",
            "health_transition": "down_to_healthy",
            "no_artifacts": true,
            "same_issue": true
        });
        let output = run_production_authority(&input, &runner);
        assert_eq!(output["exit_reason"], json!("authority_mode_mismatch"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn scheduler_rollback_parses_compact_envelope_recorded() {
        let success = include_str!("../testdata/rollback_compact_success.json");
        let failure = include_str!("../testdata/rollback_compact_failure.json");

        assert!(rollback_ledger_recorded_from_compact(success, 0));
        assert!(!rollback_ledger_recorded_from_compact(failure, 1));
        assert!(!rollback_ledger_recorded_from_compact(
            r#"{"data":{"recorded":true},"outcome":"ok"}"#,
            0
        ));
        assert!(!rollback_ledger_recorded_from_compact(success, 1));
    }

    #[test]
    #[cfg(windows)]
    fn scheduler_rollback_powershell_parse_matches_rust_contract() {
        let parse_script =
            include_str!("../../../scripts/task-scheduler/rollback-ledger-parse.ps1");
        let success = include_str!("../testdata/rollback_compact_success.json").trim();
        let failure = include_str!("../testdata/rollback_compact_failure.json").trim();

        assert!(run_rollback_ledger_parse_fixture(parse_script, success, 0));
        assert!(!run_rollback_ledger_parse_fixture(parse_script, failure, 1));
    }

    fn rollback_ledger_recorded_from_compact(stdout: &str, exit_code: i32) -> bool {
        if exit_code != 0 {
            return false;
        }
        let envelope: Value = match serde_json::from_str(stdout.trim()) {
            Ok(value) => value,
            Err(_) => return false,
        };
        if envelope.get("outcome").and_then(Value::as_str) != Some("ok") {
            return false;
        }
        envelope
            .pointer("/payload/data/recorded")
            .and_then(Value::as_bool)
            == Some(true)
    }

    #[cfg(windows)]
    fn run_rollback_ledger_parse_fixture(script: &str, stdout: &str, exit_code: i32) -> bool {
        let script_path = std::env::temp_dir().join(format!(
            "agentmesh-rollback-parse-{}.ps1",
            std::process::id()
        ));
        std::fs::write(&script_path, script).unwrap();
        let stdout_path = std::env::temp_dir().join(format!(
            "agentmesh-rollback-stdout-{}.json",
            std::process::id()
        ));
        std::fs::write(&stdout_path, stdout).unwrap();
        let ps_command = format!(
            ". '{}'; Test-RollbackLedgerRecorded -Stdout (Get-Content -LiteralPath '{}' -Raw) -ExitCode {}",
            script_path.display(),
            stdout_path.display(),
            exit_code
        );
        let output = std::process::Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps_command])
            .output()
            .expect("pwsh must be available to evaluate rollback ledger parsing");
        assert!(
            output.status.success(),
            "rollback parse fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let parsed = String::from_utf8_lossy(&output.stdout)
            .trim()
            .eq_ignore_ascii_case("true");
        let _ = std::fs::remove_file(script_path);
        let _ = std::fs::remove_file(stdout_path);
        parsed
    }

    #[test]
    fn scheduler_install_script_uses_scheduled_task_apis() {
        let script =
            include_str!("../../../scripts/task-scheduler/install-production-controller.ps1");
        assert!(script.contains("New-ScheduledTaskAction"));
        assert!(script.contains("Register-ScheduledTask"));
        assert!(!script.contains("-Execute ('\"' +"));
    }
}
