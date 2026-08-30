//! App-local SQLite control ledger.
//!
//! Stores schedule leases, scope claims, watermarks, authority mode, decisions, and
//! rollback correlation metadata only. Never stores prompts, comments, task output, or secrets.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use thiserror::Error;

/// Plugin/schema version exposed in compact output.
pub const LOCAL_CONTROL_LEDGER_VERSION: &str = "local-control-ledger.v0";
const INPUT_SCHEMA_VERSION: &str = "local-control-ledger-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "local-control-ledger-output.v0";
const MAX_ID_CHARS: usize = 128;
const MAX_SCOPE_CHARS: usize = 256;
const MAX_HASH_CHARS: usize = 128;
const MAX_CODE_CHARS: usize = 64;
const MAX_TS_CHARS: usize = 64;

/// Allowed authority modes for promotion ladder.
pub const AUTHORITY_MODES: &[&str] = &["shadow", "observer", "safe_writer", "queue", "todo_runner"];

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("validation: {0}")]
    Validation(String),
}

fn issue(code: &str, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn compact(
    operation: &str,
    valid: bool,
    exit_reason: &str,
    issues: Vec<Value>,
    data: Value,
) -> Value {
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "app_version": LOCAL_CONTROL_LEDGER_VERSION,
        "operation": operation,
        "valid": valid,
        "exit_reason": exit_reason,
        "issue_count": issues.len(),
        "issues": issues,
        "data": data,
    })
}

fn validate_id(field: &str, value: &str) -> Result<(), LedgerError> {
    if value.is_empty() || value.chars().count() > MAX_ID_CHARS {
        return Err(LedgerError::Validation(format!(
            "{field} must be 1..={MAX_ID_CHARS} chars"
        )));
    }
    Ok(())
}

fn validate_scope(value: &str) -> Result<(), LedgerError> {
    if value.is_empty() || value.chars().count() > MAX_SCOPE_CHARS {
        return Err(LedgerError::Validation(format!(
            "scope_key must be 1..={MAX_SCOPE_CHARS} chars"
        )));
    }
    Ok(())
}

fn validate_hash(field: &str, value: &str) -> Result<(), LedgerError> {
    if value.is_empty() || value.chars().count() > MAX_HASH_CHARS {
        return Err(LedgerError::Validation(format!(
            "{field} must be 1..={MAX_HASH_CHARS} chars"
        )));
    }
    Ok(())
}

fn validate_code(field: &str, value: &str) -> Result<(), LedgerError> {
    if value.is_empty() || value.chars().count() > MAX_CODE_CHARS {
        return Err(LedgerError::Validation(format!(
            "{field} must be 1..={MAX_CODE_CHARS} chars"
        )));
    }
    Ok(())
}

fn validate_ts(value: &str) -> Result<(), LedgerError> {
    if value.is_empty() || value.chars().count() > MAX_TS_CHARS {
        return Err(LedgerError::Validation(format!(
            "ts must be 1..={MAX_TS_CHARS} chars"
        )));
    }
    Ok(())
}

fn validate_authority_mode(value: &str) -> Result<(), LedgerError> {
    if !AUTHORITY_MODES.contains(&value) {
        return Err(LedgerError::Validation(format!(
            "authority_mode must be one of {AUTHORITY_MODES:?}"
        )));
    }
    Ok(())
}

/// Initialize ledger schema at the given path.
pub fn init_ledger(path: &Path) -> Result<(), LedgerError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "
        PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS schedule_leases (
            lease_id TEXT PRIMARY KEY,
            holder TEXT NOT NULL,
            acquired_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            released_at TEXT
        );
        CREATE TABLE IF NOT EXISTS scope_claims (
            scope_key TEXT PRIMARY KEY,
            claim_id TEXT NOT NULL,
            holder TEXT NOT NULL,
            acquired_at TEXT NOT NULL,
            released_at TEXT
        );
        CREATE TABLE IF NOT EXISTS watermarks (
            scope_key TEXT NOT NULL,
            watermark_key TEXT NOT NULL,
            value_hash TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (scope_key, watermark_key)
        );
        CREATE TABLE IF NOT EXISTS authority_modes (
            controller_id TEXT PRIMARY KEY,
            mode TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS decisions (
            decision_id TEXT PRIMARY KEY,
            controller_id TEXT NOT NULL,
            decision_code TEXT NOT NULL,
            result_code TEXT NOT NULL,
            input_hash TEXT NOT NULL,
            output_hash TEXT NOT NULL,
            recorded_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS rollback_events (
            event_id TEXT PRIMARY KEY,
            controller_id TEXT NOT NULL,
            reason_code TEXT NOT NULL,
            correlation_id TEXT NOT NULL,
            recorded_at TEXT NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn open_ledger(path: &Path) -> Result<Connection, LedgerError> {
    init_ledger(path)?;
    Ok(Connection::open(path)?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerInput {
    schema_version: String,
    operation: String,
    ledger_path: String,
    #[serde(default)]
    controller_id: Option<String>,
    #[serde(default)]
    lease_id: Option<String>,
    #[serde(default)]
    holder: Option<String>,
    #[serde(default)]
    acquired_at: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    scope_key: Option<String>,
    #[serde(default)]
    claim_id: Option<String>,
    #[serde(default)]
    watermark_key: Option<String>,
    #[serde(default)]
    value_hash: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    authority_mode: Option<String>,
    #[serde(default)]
    decision_id: Option<String>,
    #[serde(default)]
    decision_code: Option<String>,
    #[serde(default)]
    result_code: Option<String>,
    #[serde(default)]
    input_hash: Option<String>,
    #[serde(default)]
    output_hash: Option<String>,
    #[serde(default)]
    recorded_at: Option<String>,
    #[serde(default)]
    event_id: Option<String>,
    #[serde(default)]
    reason_code: Option<String>,
    #[serde(default)]
    correlation_id: Option<String>,
}

/// Run one ledger operation from opaque plugin input.
pub fn run_local_control_ledger(value: &Value) -> Value {
    let Ok(input) = serde_json::from_value::<LedgerInput>(value.clone()) else {
        return compact(
            "init",
            false,
            "input_invalid",
            vec![issue(
                "input_invalid",
                "input must match local-control-ledger-input.v0",
            )],
            json!(null),
        );
    };

    let operation = input.operation.as_str();
    let mut issues = Vec::new();
    if input.schema_version != INPUT_SCHEMA_VERSION {
        issues.push(issue(
            "input_invalid",
            format!("schema_version must be {INPUT_SCHEMA_VERSION}"),
        ));
    }
    let allowed = [
        "init",
        "acquire_lease",
        "release_lease",
        "claim_scope",
        "release_claim",
        "set_watermark",
        "get_watermark",
        "get_authority_mode",
        "set_authority_mode",
        "record_decision",
        "record_rollback",
    ];
    if !allowed.contains(&operation) {
        issues.push(issue("unknown_operation", "unsupported operation"));
    }
    if input.ledger_path.is_empty() {
        issues.push(issue("ledger_path_missing", "ledger_path is required"));
    }
    if !issues.is_empty() {
        return compact(operation, false, "input_invalid", issues, json!(null));
    }

    let ledger_path = Path::new(&input.ledger_path);
    match dispatch(operation, ledger_path, &input) {
        Ok(data) => compact(operation, true, "ok", Vec::new(), data),
        Err(LedgerError::Validation(message)) => compact(
            operation,
            false,
            "validation_failed",
            vec![issue("validation_failed", message)],
            json!(null),
        ),
        Err(LedgerError::Sqlite(err)) => compact(
            operation,
            false,
            "sqlite_error",
            vec![issue("sqlite_error", err.to_string())],
            json!(null),
        ),
        Err(LedgerError::Io(err)) => compact(
            operation,
            false,
            "io_error",
            vec![issue("io_error", err.to_string())],
            json!(null),
        ),
    }
}

fn dispatch(operation: &str, path: &Path, input: &LedgerInput) -> Result<Value, LedgerError> {
    match operation {
        "init" => {
            init_ledger(path)?;
            Ok(json!({"initialized": true}))
        }
        "acquire_lease" => {
            let lease_id = require(input.lease_id.as_deref(), "lease_id")?;
            let holder = require(input.holder.as_deref(), "holder")?;
            let acquired_at = require(input.acquired_at.as_deref(), "acquired_at")?;
            let expires_at = require(input.expires_at.as_deref(), "expires_at")?;
            validate_id("lease_id", lease_id)?;
            validate_id("holder", holder)?;
            validate_ts(acquired_at)?;
            validate_ts(expires_at)?;
            let conn = open_ledger(path)?;
            let active: Option<String> = conn
                .query_row(
                    "SELECT lease_id FROM schedule_leases WHERE released_at IS NULL AND expires_at > ?1 LIMIT 1",
                    params![acquired_at],
                    |row| row.get(0),
                )
                .optional()?;
            if active.is_some() {
                return Err(LedgerError::Validation("lease_already_held".into()));
            }
            conn.execute(
                "INSERT INTO schedule_leases (lease_id, holder, acquired_at, expires_at, released_at) VALUES (?1, ?2, ?3, ?4, NULL)",
                params![lease_id, holder, acquired_at, expires_at],
            )?;
            Ok(json!({"lease_id": lease_id, "acquired": true}))
        }
        "release_lease" => {
            let lease_id = require(input.lease_id.as_deref(), "lease_id")?;
            let updated_at = require(input.updated_at.as_deref(), "updated_at")?;
            validate_id("lease_id", lease_id)?;
            validate_ts(updated_at)?;
            let conn = open_ledger(path)?;
            let changed = conn.execute(
                "UPDATE schedule_leases SET released_at = ?1 WHERE lease_id = ?2 AND released_at IS NULL",
                params![updated_at, lease_id],
            )?;
            if changed == 0 {
                return Err(LedgerError::Validation("lease_not_found".into()));
            }
            Ok(json!({"lease_id": lease_id, "released": true}))
        }
        "claim_scope" => {
            let scope_key = require(input.scope_key.as_deref(), "scope_key")?;
            let claim_id = require(input.claim_id.as_deref(), "claim_id")?;
            let holder = require(input.holder.as_deref(), "holder")?;
            let acquired_at = require(input.acquired_at.as_deref(), "acquired_at")?;
            validate_scope(scope_key)?;
            validate_id("claim_id", claim_id)?;
            validate_id("holder", holder)?;
            validate_ts(acquired_at)?;
            let conn = open_ledger(path)?;
            let active: Option<String> = conn
                .query_row(
                    "SELECT scope_key FROM scope_claims WHERE released_at IS NULL LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(existing) = active {
                if existing != scope_key {
                    return Err(LedgerError::Validation("scope_claim_conflict".into()));
                }
            }
            conn.execute(
                "INSERT INTO scope_claims (scope_key, claim_id, holder, acquired_at, released_at)
                 VALUES (?1, ?2, ?3, ?4, NULL)
                 ON CONFLICT(scope_key) DO UPDATE SET
                   claim_id = excluded.claim_id,
                   holder = excluded.holder,
                   acquired_at = excluded.acquired_at,
                   released_at = NULL
                 WHERE scope_claims.released_at IS NOT NULL",
                params![scope_key, claim_id, holder, acquired_at],
            )?;
            Ok(json!({"scope_key": scope_key, "claimed": true}))
        }
        "release_claim" => {
            let scope_key = require(input.scope_key.as_deref(), "scope_key")?;
            let updated_at = require(input.updated_at.as_deref(), "updated_at")?;
            validate_scope(scope_key)?;
            validate_ts(updated_at)?;
            let conn = open_ledger(path)?;
            let changed = conn.execute(
                "UPDATE scope_claims SET released_at = ?1 WHERE scope_key = ?2 AND released_at IS NULL",
                params![updated_at, scope_key],
            )?;
            if changed == 0 {
                return Err(LedgerError::Validation("claim_not_found".into()));
            }
            Ok(json!({"scope_key": scope_key, "released": true}))
        }
        "set_watermark" => {
            let scope_key = require(input.scope_key.as_deref(), "scope_key")?;
            let watermark_key = require(input.watermark_key.as_deref(), "watermark_key")?;
            let value_hash = require(input.value_hash.as_deref(), "value_hash")?;
            let updated_at = require(input.updated_at.as_deref(), "updated_at")?;
            validate_scope(scope_key)?;
            validate_id("watermark_key", watermark_key)?;
            validate_hash("value_hash", value_hash)?;
            validate_ts(updated_at)?;
            let conn = open_ledger(path)?;
            conn.execute(
                "INSERT INTO watermarks (scope_key, watermark_key, value_hash, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(scope_key, watermark_key) DO UPDATE SET
                   value_hash = excluded.value_hash,
                   updated_at = excluded.updated_at",
                params![scope_key, watermark_key, value_hash, updated_at],
            )?;
            Ok(json!({"scope_key": scope_key, "watermark_key": watermark_key}))
        }
        "get_watermark" => {
            let scope_key = require(input.scope_key.as_deref(), "scope_key")?;
            let watermark_key = require(input.watermark_key.as_deref(), "watermark_key")?;
            validate_scope(scope_key)?;
            validate_id("watermark_key", watermark_key)?;
            let conn = open_ledger(path)?;
            let row: Option<(String, String)> = conn
                .query_row(
                    "SELECT value_hash, updated_at FROM watermarks WHERE scope_key = ?1 AND watermark_key = ?2",
                    params![scope_key, watermark_key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            match row {
                Some((value_hash, updated_at)) => Ok(json!({
                    "scope_key": scope_key,
                    "watermark_key": watermark_key,
                    "value_hash": value_hash,
                    "updated_at": updated_at,
                    "found": true
                })),
                None => Ok(json!({
                    "scope_key": scope_key,
                    "watermark_key": watermark_key,
                    "found": false
                })),
            }
        }
        "get_authority_mode" => {
            let controller_id = require(input.controller_id.as_deref(), "controller_id")?;
            validate_id("controller_id", controller_id)?;
            let conn = open_ledger(path)?;
            let row: Option<(String, String)> = conn
                .query_row(
                    "SELECT mode, updated_at FROM authority_modes WHERE controller_id = ?1",
                    params![controller_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            match row {
                Some((mode, updated_at)) => Ok(json!({
                    "controller_id": controller_id,
                    "authority_mode": mode,
                    "updated_at": updated_at,
                    "found": true
                })),
                None => Ok(json!({
                    "controller_id": controller_id,
                    "authority_mode": "shadow",
                    "found": false
                })),
            }
        }
        "set_authority_mode" => {
            let controller_id = require(input.controller_id.as_deref(), "controller_id")?;
            let authority_mode = require(input.authority_mode.as_deref(), "authority_mode")?;
            let updated_at = require(input.updated_at.as_deref(), "updated_at")?;
            validate_id("controller_id", controller_id)?;
            validate_authority_mode(authority_mode)?;
            validate_ts(updated_at)?;
            let conn = open_ledger(path)?;
            conn.execute(
                "INSERT INTO authority_modes (controller_id, mode, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(controller_id) DO UPDATE SET mode = excluded.mode, updated_at = excluded.updated_at",
                params![controller_id, authority_mode, updated_at],
            )?;
            Ok(json!({"controller_id": controller_id, "authority_mode": authority_mode}))
        }
        "record_decision" => {
            let controller_id = require(input.controller_id.as_deref(), "controller_id")?;
            let decision_id = require(input.decision_id.as_deref(), "decision_id")?;
            let decision_code = require(input.decision_code.as_deref(), "decision_code")?;
            let result_code = require(input.result_code.as_deref(), "result_code")?;
            let input_hash = require(input.input_hash.as_deref(), "input_hash")?;
            let output_hash = require(input.output_hash.as_deref(), "output_hash")?;
            let recorded_at = require(input.recorded_at.as_deref(), "recorded_at")?;
            validate_id("controller_id", controller_id)?;
            validate_id("decision_id", decision_id)?;
            validate_code("decision_code", decision_code)?;
            validate_code("result_code", result_code)?;
            validate_hash("input_hash", input_hash)?;
            validate_hash("output_hash", output_hash)?;
            validate_ts(recorded_at)?;
            let conn = open_ledger(path)?;
            conn.execute(
                "INSERT OR REPLACE INTO decisions
                 (decision_id, controller_id, decision_code, result_code, input_hash, output_hash, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    decision_id,
                    controller_id,
                    decision_code,
                    result_code,
                    input_hash,
                    output_hash,
                    recorded_at
                ],
            )?;
            Ok(json!({"decision_id": decision_id, "recorded": true}))
        }
        "record_rollback" => {
            let controller_id = require(input.controller_id.as_deref(), "controller_id")?;
            let event_id = require(input.event_id.as_deref(), "event_id")?;
            let reason_code = require(input.reason_code.as_deref(), "reason_code")?;
            let correlation_id = require(input.correlation_id.as_deref(), "correlation_id")?;
            let recorded_at = require(input.recorded_at.as_deref(), "recorded_at")?;
            validate_id("controller_id", controller_id)?;
            validate_id("event_id", event_id)?;
            validate_code("reason_code", reason_code)?;
            validate_id("correlation_id", correlation_id)?;
            validate_ts(recorded_at)?;
            let conn = open_ledger(path)?;
            conn.execute(
                "INSERT OR REPLACE INTO rollback_events
                 (event_id, controller_id, reason_code, correlation_id, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    event_id,
                    controller_id,
                    reason_code,
                    correlation_id,
                    recorded_at
                ],
            )?;
            Ok(json!({"event_id": event_id, "recorded": true}))
        }
        _ => Err(LedgerError::Validation("unknown_operation".into())),
    }
}

fn require<'a>(value: Option<&'a str>, field: &str) -> Result<&'a str, LedgerError> {
    value.ok_or_else(|| LedgerError::Validation(format!("{field} is required")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger_input(operation: &str, path: &Path) -> Value {
        json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": operation,
            "ledger_path": path.to_string_lossy(),
        })
    }

    #[test]
    fn init_and_authority_mode_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        let init = run_local_control_ledger(&ledger_input("init", &path));
        assert_eq!(init["valid"], json!(true));

        let mut set = ledger_input("set_authority_mode", &path);
        set["controller_id"] = json!("workflow_audit");
        set["authority_mode"] = json!("observer");
        set["updated_at"] = json!("2026-08-30T12:00:00+09:00");
        assert_eq!(run_local_control_ledger(&set)["valid"], json!(true));

        let mut get = ledger_input("get_authority_mode", &path);
        get["controller_id"] = json!("workflow_audit");
        let got = run_local_control_ledger(&get);
        assert_eq!(got["data"]["authority_mode"], json!("observer"));
    }

    #[test]
    fn lease_conflict_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        run_local_control_ledger(&ledger_input("init", &path));

        let mut first = ledger_input("acquire_lease", &path);
        first["lease_id"] = json!("lease-1");
        first["holder"] = json!("scheduler");
        first["acquired_at"] = json!("2026-08-30T12:00:00+09:00");
        first["expires_at"] = json!("2026-08-30T12:05:00+09:00");
        assert_eq!(run_local_control_ledger(&first)["valid"], json!(true));

        let mut second = first.clone();
        second["lease_id"] = json!("lease-2");
        let conflict = run_local_control_ledger(&second);
        assert_eq!(conflict["valid"], json!(false));
        assert_eq!(conflict["exit_reason"], json!("validation_failed"));
    }

    #[test]
    fn decision_record_stores_hashes_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        run_local_control_ledger(&ledger_input("init", &path));
        let mut record = ledger_input("record_decision", &path);
        record["controller_id"] = json!("workflow_audit");
        record["decision_id"] = json!("dec-1");
        record["decision_code"] = json!("audit_preflight");
        record["result_code"] = json!("no_mutation");
        record["input_hash"] = json!("sha256:abc");
        record["output_hash"] = json!("sha256:def");
        record["recorded_at"] = json!("2026-08-30T12:00:00+09:00");
        assert_eq!(run_local_control_ledger(&record)["valid"], json!(true));
    }
}
