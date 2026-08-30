//! App-local SQLite control ledger.
//!
//! Stores schedule leases, scope claims, watermarks, authority mode, decisions, and
//! rollback correlation metadata only. Never stores prompts, comments, task output, or secrets.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use thiserror::Error;

/// Plugin/schema version exposed in compact output.
pub const LOCAL_CONTROL_LEDGER_VERSION: &str = "local-control-ledger.v0";
const INPUT_SCHEMA_VERSION: &str = "local-control-ledger-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "local-control-ledger-output.v0";
/// SQLite schema version stored in `ledger_meta`.
pub const LEDGER_DB_SCHEMA_VERSION: &str = "2";
const BUSY_TIMEOUT_MS: i32 = 5_000;
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
        "ledger_schema_version": LEDGER_DB_SCHEMA_VERSION,
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

/// Parse an RFC 3339 timestamp into epoch seconds for canonical ordering.
pub fn parse_rfc3339_epoch(ts: &str) -> Option<i64> {
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
            if ts.as_bytes().get(22)? != &b':' {
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

/// Returns true when `candidate` is strictly after `reference` using canonical RFC 3339 ordering.
pub fn ts_is_after(candidate: &str, reference: &str) -> bool {
    match (
        parse_rfc3339_epoch(candidate),
        parse_rfc3339_epoch(reference),
    ) {
        (Some(c), Some(r)) => c > r,
        _ => candidate > reference,
    }
}

fn configure_connection(conn: &Connection) -> Result<(), LedgerError> {
    conn.busy_timeout(std::time::Duration::from_millis(
        u64::try_from(BUSY_TIMEOUT_MS).unwrap(),
    ))?;
    Ok(())
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, LedgerError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, LedgerError> {
    let sql = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn idempotency_pk_is_composite(conn: &Connection) -> Result<bool, LedgerError> {
    if !table_exists(conn, "idempotency_claims")? {
        return Ok(false);
    }
    let sql = "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'idempotency_claims'";
    let ddl: Option<String> = conn.query_row(sql, [], |row| row.get(0)).optional()?;
    Ok(ddl
        .as_deref()
        .is_some_and(|ddl| ddl.contains("PRIMARY KEY (controller_id, decision_hash)")))
}

fn rebuild_idempotency_composite_pk(conn: &Connection) -> Result<(), LedgerError> {
    if idempotency_pk_is_composite(conn)? {
        return Ok(());
    }
    if !table_exists(conn, "idempotency_claims")? {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS idempotency_claims (
                controller_id TEXT NOT NULL,
                decision_hash TEXT NOT NULL,
                recorded_at TEXT NOT NULL,
                PRIMARY KEY (controller_id, decision_hash)
            );
            ",
        )?;
        return Ok(());
    }
    conn.execute_batch(
        "
        CREATE TABLE idempotency_claims_v2 (
            controller_id TEXT NOT NULL,
            decision_hash TEXT NOT NULL,
            recorded_at TEXT NOT NULL,
            PRIMARY KEY (controller_id, decision_hash)
        );
        INSERT OR IGNORE INTO idempotency_claims_v2 (controller_id, decision_hash, recorded_at)
            SELECT controller_id, decision_hash, recorded_at FROM idempotency_claims;
        DROP TABLE idempotency_claims;
        ALTER TABLE idempotency_claims_v2 RENAME TO idempotency_claims;
        ",
    )?;
    Ok(())
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), LedgerError> {
    if !column_exists(conn, table, column)? {
        conn.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition};"
        ))?;
    }
    Ok(())
}

fn migrate_to_v2_idempotent(conn: &Connection) -> Result<(), LedgerError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS ledger_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )?;
    add_column_if_missing(
        conn,
        "scope_claims",
        "expires_at",
        "TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z'",
    )?;
    add_column_if_missing(
        conn,
        "decisions",
        "authority_mode",
        "TEXT NOT NULL DEFAULT 'shadow'",
    )?;
    add_column_if_missing(
        conn,
        "decisions",
        "hard_gate_pass",
        "INTEGER NOT NULL DEFAULT 1",
    )?;
    if !table_exists(conn, "violation_events")? {
        conn.execute_batch(
            "
            CREATE TABLE violation_events (
                event_id TEXT PRIMARY KEY,
                controller_id TEXT NOT NULL,
                authority_mode TEXT NOT NULL DEFAULT 'shadow',
                violation_type TEXT NOT NULL,
                decision_hash TEXT NOT NULL,
                recorded_at TEXT NOT NULL
            );
            ",
        )?;
    } else {
        add_column_if_missing(
            conn,
            "violation_events",
            "authority_mode",
            "TEXT NOT NULL DEFAULT 'shadow'",
        )?;
    }
    rebuild_idempotency_composite_pk(conn)?;
    conn.execute(
        "INSERT OR REPLACE INTO ledger_meta (key, value) VALUES ('schema_version', ?1)",
        params![LEDGER_DB_SCHEMA_VERSION],
    )?;
    Ok(())
}

fn migrate_foundation_v1_to_v2(conn: &Connection) -> Result<(), LedgerError> {
    migrate_to_v2_idempotent(conn)
}

fn migrate_authority_v1_to_v2(conn: &Connection) -> Result<(), LedgerError> {
    migrate_to_v2_idempotent(conn)
}

fn ensure_schema_version(conn: &Connection) -> Result<(), LedgerError> {
    if !table_exists(conn, "ledger_meta")? {
        return migrate_foundation_v1_to_v2(conn);
    }
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM ledger_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match stored.as_deref() {
        Some(version) if version == LEDGER_DB_SCHEMA_VERSION => {
            if column_exists(conn, "decisions", "authority_mode")?
                && idempotency_pk_is_composite(conn)?
            {
                Ok(())
            } else {
                migrate_to_v2_idempotent(conn)
            }
        }
        Some("1") => migrate_authority_v1_to_v2(conn),
        None if table_exists(conn, "schedule_leases")? => migrate_foundation_v1_to_v2(conn),
        Some(version) => Err(LedgerError::Validation(format!(
            "ledger schema_version mismatch: expected {LEDGER_DB_SCHEMA_VERSION}, found {version}"
        ))),
        None => migrate_foundation_v1_to_v2(conn),
    }
}

/// Initialize ledger schema at the given path.
pub fn init_ledger(path: &Path) -> Result<(), LedgerError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let conn = Connection::open(path)?;
    configure_connection(&conn)?;
    conn.execute_batch(
        "
        PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS ledger_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
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
            expires_at TEXT NOT NULL,
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
            authority_mode TEXT NOT NULL,
            decision_code TEXT NOT NULL,
            result_code TEXT NOT NULL,
            input_hash TEXT NOT NULL,
            output_hash TEXT NOT NULL,
            hard_gate_pass INTEGER NOT NULL,
            recorded_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS rollback_events (
            event_id TEXT PRIMARY KEY,
            controller_id TEXT NOT NULL,
            reason_code TEXT NOT NULL,
            correlation_id TEXT NOT NULL,
            recorded_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS violation_events (
            event_id TEXT PRIMARY KEY,
            controller_id TEXT NOT NULL,
            authority_mode TEXT NOT NULL,
            violation_type TEXT NOT NULL,
            decision_hash TEXT NOT NULL,
            recorded_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS idempotency_claims (
            controller_id TEXT NOT NULL,
            decision_hash TEXT NOT NULL,
            recorded_at TEXT NOT NULL,
            PRIMARY KEY (controller_id, decision_hash)
        );
        ",
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO ledger_meta (key, value) VALUES ('schema_version', ?1)",
        params![LEDGER_DB_SCHEMA_VERSION],
    )?;
    Ok(())
}

fn open_ledger(path: &Path) -> Result<Connection, LedgerError> {
    init_ledger(path)?;
    let conn = Connection::open(path)?;
    configure_connection(&conn)?;
    ensure_schema_version(&conn)?;
    Ok(conn)
}

fn reclaim_expired_leases(conn: &Connection, now: &str) -> Result<usize, LedgerError> {
    Ok(conn.execute(
        "UPDATE schedule_leases SET released_at = ?1
         WHERE released_at IS NULL AND expires_at <= ?1",
        params![now],
    )?)
}

fn reclaim_expired_claims(conn: &Connection, now: &str) -> Result<usize, LedgerError> {
    Ok(conn.execute(
        "UPDATE scope_claims SET released_at = ?1
         WHERE released_at IS NULL AND expires_at <= ?1",
        params![now],
    )?)
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
    #[serde(default)]
    since_ts: Option<String>,
    #[serde(default)]
    until_ts: Option<String>,
    #[serde(default)]
    violation_type: Option<String>,
    #[serde(default)]
    decision_hash: Option<String>,
    #[serde(default)]
    hard_gate_pass: Option<bool>,
    #[serde(default)]
    target_mode: Option<String>,
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
        "get_promotion_metrics",
        "claim_idempotency",
        "record_violation",
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
        Ok((data, exit_reason, valid)) => compact(operation, valid, exit_reason, Vec::new(), data),
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

fn dispatch(
    operation: &str,
    path: &Path,
    input: &LedgerInput,
) -> Result<(Value, &'static str, bool), LedgerError> {
    match operation {
        "init" => {
            init_ledger(path)?;
            Ok((
                json!({
                    "initialized": true,
                    "ledger_schema_version": LEDGER_DB_SCHEMA_VERSION,
                }),
                "ok",
                true,
            ))
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
            if !ts_is_after(expires_at, acquired_at) {
                return Err(LedgerError::Validation(
                    "expires_at must be after acquired_at".into(),
                ));
            }
            let mut conn = open_ledger(path)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let reclaimed = reclaim_expired_leases(&tx, acquired_at)?;
            let changed = tx.execute(
                "INSERT INTO schedule_leases (lease_id, holder, acquired_at, expires_at, released_at)
                 VALUES (?1, ?2, ?3, ?4, NULL)
                 ON CONFLICT(lease_id) DO UPDATE SET
                   holder = excluded.holder,
                   acquired_at = excluded.acquired_at,
                   expires_at = excluded.expires_at,
                   released_at = NULL
                 WHERE schedule_leases.released_at IS NOT NULL OR schedule_leases.expires_at <= ?3",
                params![lease_id, holder, acquired_at, expires_at],
            )?;
            if changed == 0 {
                tx.commit()?;
                return Ok((
                    json!({"lease_id": lease_id, "acquired": false, "reclaimed_expired": reclaimed > 0}),
                    "lease_already_held",
                    false,
                ));
            }
            tx.commit()?;
            Ok((
                json!({"lease_id": lease_id, "acquired": true, "reclaimed_expired": reclaimed > 0}),
                "ok",
                true,
            ))
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
            Ok((json!({"lease_id": lease_id, "released": true}), "ok", true))
        }
        "claim_scope" => {
            let scope_key = require(input.scope_key.as_deref(), "scope_key")?;
            let claim_id = require(input.claim_id.as_deref(), "claim_id")?;
            let holder = require(input.holder.as_deref(), "holder")?;
            let acquired_at = require(input.acquired_at.as_deref(), "acquired_at")?;
            let expires_at = require(input.expires_at.as_deref(), "expires_at")?;
            validate_scope(scope_key)?;
            validate_id("claim_id", claim_id)?;
            validate_id("holder", holder)?;
            validate_ts(acquired_at)?;
            validate_ts(expires_at)?;
            if !ts_is_after(expires_at, acquired_at) {
                return Err(LedgerError::Validation(
                    "expires_at must be after acquired_at".into(),
                ));
            }
            let mut conn = open_ledger(path)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let reclaimed = reclaim_expired_claims(&tx, acquired_at)?;
            let changed = tx.execute(
                "INSERT INTO scope_claims (scope_key, claim_id, holder, acquired_at, expires_at, released_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL)
                 ON CONFLICT(scope_key) DO UPDATE SET
                   claim_id = excluded.claim_id,
                   holder = excluded.holder,
                   acquired_at = excluded.acquired_at,
                   expires_at = excluded.expires_at,
                   released_at = NULL
                 WHERE scope_claims.released_at IS NOT NULL OR scope_claims.expires_at <= ?4",
                params![scope_key, claim_id, holder, acquired_at, expires_at],
            )?;
            if changed == 0 {
                tx.commit()?;
                return Ok((
                    json!({"scope_key": scope_key, "claimed": false, "reclaimed_expired": reclaimed > 0}),
                    "scope_claim_conflict",
                    false,
                ));
            }
            tx.commit()?;
            Ok((
                json!({"scope_key": scope_key, "claimed": true, "reclaimed_expired": reclaimed > 0}),
                "ok",
                true,
            ))
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
            Ok((
                json!({"scope_key": scope_key, "released": true}),
                "ok",
                true,
            ))
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
            Ok((
                json!({"scope_key": scope_key, "watermark_key": watermark_key}),
                "ok",
                true,
            ))
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
                Some((value_hash, updated_at)) => Ok((
                    json!({
                        "scope_key": scope_key,
                        "watermark_key": watermark_key,
                        "value_hash": value_hash,
                        "updated_at": updated_at,
                        "found": true
                    }),
                    "ok",
                    true,
                )),
                None => Ok((
                    json!({
                        "scope_key": scope_key,
                        "watermark_key": watermark_key,
                        "found": false
                    }),
                    "ok",
                    true,
                )),
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
                Some((mode, updated_at)) => Ok((
                    json!({
                        "controller_id": controller_id,
                        "authority_mode": mode,
                        "updated_at": updated_at,
                        "found": true
                    }),
                    "ok",
                    true,
                )),
                None => Ok((
                    json!({
                        "controller_id": controller_id,
                        "authority_mode": "shadow",
                        "found": false
                    }),
                    "ok",
                    true,
                )),
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
            Ok((
                json!({"controller_id": controller_id, "authority_mode": authority_mode}),
                "ok",
                true,
            ))
        }
        "record_decision" => {
            let controller_id = require(input.controller_id.as_deref(), "controller_id")?;
            let decision_id = require(input.decision_id.as_deref(), "decision_id")?;
            let authority_mode = require(input.authority_mode.as_deref(), "authority_mode")?;
            let decision_code = require(input.decision_code.as_deref(), "decision_code")?;
            let result_code = require(input.result_code.as_deref(), "result_code")?;
            let input_hash = require(input.input_hash.as_deref(), "input_hash")?;
            let output_hash = require(input.output_hash.as_deref(), "output_hash")?;
            let hard_gate_pass = input.hard_gate_pass.unwrap_or(true);
            let recorded_at = require(input.recorded_at.as_deref(), "recorded_at")?;
            validate_id("controller_id", controller_id)?;
            validate_id("decision_id", decision_id)?;
            validate_authority_mode(authority_mode)?;
            validate_code("decision_code", decision_code)?;
            validate_code("result_code", result_code)?;
            validate_hash("input_hash", input_hash)?;
            validate_hash("output_hash", output_hash)?;
            validate_ts(recorded_at)?;
            let conn = open_ledger(path)?;
            conn.execute(
                "INSERT OR REPLACE INTO decisions
                 (decision_id, controller_id, authority_mode, decision_code, result_code, input_hash, output_hash, hard_gate_pass, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    decision_id,
                    controller_id,
                    authority_mode,
                    decision_code,
                    result_code,
                    input_hash,
                    output_hash,
                    i64::from(hard_gate_pass),
                    recorded_at
                ],
            )?;
            Ok((
                json!({"decision_id": decision_id, "recorded": true}),
                "ok",
                true,
            ))
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
            Ok((json!({"event_id": event_id, "recorded": true}), "ok", true))
        }
        "get_promotion_metrics" => {
            let controller_id = require(input.controller_id.as_deref(), "controller_id")?;
            let target_mode = require(input.target_mode.as_deref(), "target_mode")?;
            let since_ts = require(input.since_ts.as_deref(), "since_ts")?;
            let until_ts = require(input.until_ts.as_deref(), "until_ts")?;
            validate_id("controller_id", controller_id)?;
            validate_authority_mode(target_mode)?;
            validate_ts(since_ts)?;
            validate_ts(until_ts)?;
            let conn = open_ledger(path)?;
            let shadow_decision_code = format!("{target_mode}_shadow_run_once");
            let decision_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM decisions
                 WHERE controller_id = ?1 AND authority_mode = ?2 AND decision_code = ?5
                   AND recorded_at >= ?3 AND recorded_at <= ?4",
                params![
                    controller_id,
                    target_mode,
                    since_ts,
                    until_ts,
                    shadow_decision_code
                ],
                |row| row.get(0),
            )?;
            let hard_gate_failures: i64 = conn.query_row(
                "SELECT COUNT(*) FROM decisions
                 WHERE controller_id = ?1 AND authority_mode = ?2 AND decision_code = ?5
                   AND recorded_at >= ?3 AND recorded_at <= ?4
                   AND hard_gate_pass = 0",
                params![
                    controller_id,
                    target_mode,
                    since_ts,
                    until_ts,
                    shadow_decision_code
                ],
                |row| row.get(0),
            )?;
            let unauthorized_writes: i64 = conn.query_row(
                "SELECT COUNT(*) FROM violation_events
                 WHERE controller_id = ?1 AND authority_mode = ?2
                   AND violation_type = 'unauthorized_write'
                   AND recorded_at >= ?3 AND recorded_at <= ?4",
                params![controller_id, target_mode, since_ts, until_ts],
                |row| row.get(0),
            )?;
            let duplicate_mutations: i64 = conn.query_row(
                "SELECT COUNT(*) FROM violation_events
                 WHERE controller_id = ?1 AND authority_mode = ?2
                   AND violation_type = 'duplicate_mutation'
                   AND recorded_at >= ?3 AND recorded_at <= ?4",
                params![controller_id, target_mode, since_ts, until_ts],
                |row| row.get(0),
            )?;
            let duplicate_starts: i64 = conn.query_row(
                "SELECT COUNT(*) FROM violation_events
                 WHERE controller_id = ?1 AND authority_mode = ?2
                   AND violation_type = 'duplicate_start'
                   AND recorded_at >= ?3 AND recorded_at <= ?4",
                params![controller_id, target_mode, since_ts, until_ts],
                |row| row.get(0),
            )?;
            let hard_gate_parity_pct = if decision_count == 0 {
                0
            } else {
                ((decision_count - hard_gate_failures) * 100) / decision_count
            };
            Ok((
                json!({
                    "controller_id": controller_id,
                    "target_mode": target_mode,
                    "decision_count": decision_count,
                    "hard_gate_failures": hard_gate_failures,
                    "hard_gate_parity_pct": hard_gate_parity_pct,
                    "unauthorized_writes": unauthorized_writes,
                    "duplicate_mutations": duplicate_mutations,
                    "duplicate_starts": duplicate_starts,
                }),
                "ok",
                true,
            ))
        }
        "claim_idempotency" => {
            let controller_id = require(input.controller_id.as_deref(), "controller_id")?;
            let decision_hash = require(input.decision_hash.as_deref(), "decision_hash")?;
            let recorded_at = require(input.recorded_at.as_deref(), "recorded_at")?;
            validate_id("controller_id", controller_id)?;
            validate_hash("decision_hash", decision_hash)?;
            validate_ts(recorded_at)?;
            let conn = open_ledger(path)?;
            let inserted = conn.execute(
                "INSERT OR IGNORE INTO idempotency_claims (controller_id, decision_hash, recorded_at)
                 VALUES (?1, ?2, ?3)",
                params![controller_id, decision_hash, recorded_at],
            )?;
            Ok((
                json!({
                    "decision_hash": decision_hash,
                    "claimed": inserted == 1,
                    "duplicate": inserted == 0,
                }),
                if inserted == 1 {
                    "ok"
                } else {
                    "duplicate_suppressed"
                },
                inserted == 1,
            ))
        }
        "record_violation" => {
            let controller_id = require(input.controller_id.as_deref(), "controller_id")?;
            let authority_mode = require(input.authority_mode.as_deref(), "authority_mode")?;
            let event_id = require(input.event_id.as_deref(), "event_id")?;
            let violation_type = require(input.violation_type.as_deref(), "violation_type")?;
            let decision_hash = require(input.decision_hash.as_deref(), "decision_hash")?;
            let recorded_at = require(input.recorded_at.as_deref(), "recorded_at")?;
            validate_id("controller_id", controller_id)?;
            validate_authority_mode(authority_mode)?;
            validate_id("event_id", event_id)?;
            validate_code("violation_type", violation_type)?;
            validate_hash("decision_hash", decision_hash)?;
            validate_ts(recorded_at)?;
            let allowed = [
                "unauthorized_write",
                "duplicate_mutation",
                "duplicate_start",
            ];
            if !allowed.contains(&violation_type) {
                return Err(LedgerError::Validation(format!(
                    "violation_type must be one of {allowed:?}"
                )));
            }
            let conn = open_ledger(path)?;
            conn.execute(
                "INSERT OR REPLACE INTO violation_events
                 (event_id, controller_id, authority_mode, violation_type, decision_hash, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    event_id,
                    controller_id,
                    authority_mode,
                    violation_type,
                    decision_hash,
                    recorded_at
                ],
            )?;
            Ok((json!({"event_id": event_id, "recorded": true}), "ok", true))
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
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn ledger_input(operation: &str, path: &Path) -> Value {
        json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": operation,
            "ledger_path": path.to_string_lossy(),
        })
    }

    fn acquire_lease_with_holder(
        path: &Path,
        lease_id: &str,
        holder: &str,
        acquired_at: &str,
        expires_at: &str,
    ) -> Value {
        let mut input = ledger_input("acquire_lease", path);
        input["lease_id"] = json!(lease_id);
        input["holder"] = json!(holder);
        input["acquired_at"] = json!(acquired_at);
        input["expires_at"] = json!(expires_at);
        run_local_control_ledger(&input)
    }

    fn acquire_lease(path: &Path, lease_id: &str, acquired_at: &str, expires_at: &str) -> Value {
        acquire_lease_with_holder(path, lease_id, "scheduler", acquired_at, expires_at)
    }

    fn claim_scope(
        path: &Path,
        scope_key: &str,
        claim_id: &str,
        holder: &str,
        acquired_at: &str,
        expires_at: &str,
    ) -> Value {
        let mut input = ledger_input("claim_scope", path);
        input["scope_key"] = json!(scope_key);
        input["claim_id"] = json!(claim_id);
        input["holder"] = json!(holder);
        input["acquired_at"] = json!(acquired_at);
        input["expires_at"] = json!(expires_at);
        run_local_control_ledger(&input)
    }

    fn materialize_foundation_v1(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            CREATE TABLE schedule_leases (
                lease_id TEXT PRIMARY KEY,
                holder TEXT NOT NULL,
                acquired_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                released_at TEXT
            );
            CREATE TABLE scope_claims (
                scope_key TEXT PRIMARY KEY,
                claim_id TEXT NOT NULL,
                holder TEXT NOT NULL,
                acquired_at TEXT NOT NULL,
                released_at TEXT
            );
            CREATE TABLE watermarks (
                scope_key TEXT NOT NULL,
                watermark_key TEXT NOT NULL,
                value_hash TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (scope_key, watermark_key)
            );
            CREATE TABLE authority_modes (
                controller_id TEXT PRIMARY KEY,
                mode TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE decisions (
                decision_id TEXT PRIMARY KEY,
                controller_id TEXT NOT NULL,
                decision_code TEXT NOT NULL,
                result_code TEXT NOT NULL,
                input_hash TEXT NOT NULL,
                output_hash TEXT NOT NULL,
                recorded_at TEXT NOT NULL
            );
            CREATE TABLE rollback_events (
                event_id TEXT PRIMARY KEY,
                controller_id TEXT NOT NULL,
                reason_code TEXT NOT NULL,
                correlation_id TEXT NOT NULL,
                recorded_at TEXT NOT NULL
            );
            INSERT INTO schedule_leases (lease_id, holder, acquired_at, expires_at, released_at)
                VALUES ('lease-old', 'scheduler', '2026-08-29T12:00:00+09:00', '2026-08-30T12:00:00+09:00', NULL);
            INSERT INTO scope_claims (scope_key, claim_id, holder, acquired_at, released_at)
                VALUES ('scope-old', 'claim-old', 'controller-a', '2026-08-29T12:00:00+09:00', NULL);
            INSERT INTO watermarks (scope_key, watermark_key, value_hash, updated_at)
                VALUES ('scope-old', 'wm-1', 'sha256:wm', '2026-08-29T12:00:00+09:00');
            INSERT INTO authority_modes (controller_id, mode, updated_at)
                VALUES ('workflow_audit', 'observer', '2026-08-29T12:00:00+09:00');
            INSERT INTO decisions
                (decision_id, controller_id, decision_code, result_code, input_hash, output_hash, recorded_at)
                VALUES ('dec-old', 'workflow_audit', 'run_once', 'ok', 'sha256:in', 'sha256:out', '2026-08-29T12:00:00+09:00');
            INSERT INTO rollback_events
                (event_id, controller_id, reason_code, correlation_id, recorded_at)
                VALUES ('rb-old', 'workflow_audit', 'manual', 'corr-1', '2026-08-29T12:00:00+09:00');
            ",
        )
        .unwrap();
    }

    fn materialize_authority_v1(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            CREATE TABLE ledger_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT INTO ledger_meta (key, value) VALUES ('schema_version', '1');
            CREATE TABLE schedule_leases (
                lease_id TEXT PRIMARY KEY,
                holder TEXT NOT NULL,
                acquired_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                released_at TEXT
            );
            CREATE TABLE scope_claims (
                scope_key TEXT PRIMARY KEY,
                claim_id TEXT NOT NULL,
                holder TEXT NOT NULL,
                acquired_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                released_at TEXT
            );
            CREATE TABLE watermarks (
                scope_key TEXT NOT NULL,
                watermark_key TEXT NOT NULL,
                value_hash TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (scope_key, watermark_key)
            );
            CREATE TABLE authority_modes (
                controller_id TEXT PRIMARY KEY,
                mode TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE decisions (
                decision_id TEXT PRIMARY KEY,
                controller_id TEXT NOT NULL,
                decision_code TEXT NOT NULL,
                result_code TEXT NOT NULL,
                input_hash TEXT NOT NULL,
                output_hash TEXT NOT NULL,
                recorded_at TEXT NOT NULL
            );
            CREATE TABLE rollback_events (
                event_id TEXT PRIMARY KEY,
                controller_id TEXT NOT NULL,
                reason_code TEXT NOT NULL,
                correlation_id TEXT NOT NULL,
                recorded_at TEXT NOT NULL
            );
            CREATE TABLE violation_events (
                event_id TEXT PRIMARY KEY,
                controller_id TEXT NOT NULL,
                violation_type TEXT NOT NULL,
                decision_hash TEXT NOT NULL,
                recorded_at TEXT NOT NULL
            );
            CREATE TABLE idempotency_claims (
                decision_hash TEXT PRIMARY KEY,
                controller_id TEXT NOT NULL,
                recorded_at TEXT NOT NULL
            );
            INSERT INTO decisions
                (decision_id, controller_id, decision_code, result_code, input_hash, output_hash, recorded_at)
                VALUES ('dec-v1', 'workflow_audit', 'run_once', 'ok', 'sha256:in1', 'sha256:out1', '2026-08-29T12:00:00+09:00');
            INSERT INTO rollback_events
                (event_id, controller_id, reason_code, correlation_id, recorded_at)
                VALUES ('rb-v1', 'workflow_audit', 'manual', 'corr-v1', '2026-08-29T12:00:00+09:00');
            INSERT INTO violation_events
                (event_id, controller_id, violation_type, decision_hash, recorded_at)
                VALUES ('vio-v1', 'workflow_audit', 'duplicate_start', 'sha256:dup', '2026-08-29T12:00:00+09:00');
            INSERT INTO idempotency_claims (decision_hash, controller_id, recorded_at)
                VALUES ('sha256:idempotent', 'workflow_audit', '2026-08-29T12:00:00+09:00');
            ",
        )
        .unwrap();
    }

    fn open_migrated(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        configure_connection(&conn).unwrap();
        ensure_schema_version(&conn).unwrap();
        conn
    }

    #[test]
    fn init_and_authority_mode_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        let init = run_local_control_ledger(&ledger_input("init", &path));
        assert_eq!(init["valid"], json!(true));
        assert_eq!(init["data"]["ledger_schema_version"], json!("2"));

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
    fn lease_conflict_same_key_returns_acquired_false() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        run_local_control_ledger(&ledger_input("init", &path));

        let first = acquire_lease_with_holder(
            &path,
            "lease-1",
            "controller-a",
            "2026-08-30T12:00:00+09:00",
            "2026-08-30T12:05:00+09:00",
        );
        assert_eq!(first["valid"], json!(true));
        assert_eq!(first["data"]["acquired"], json!(true));

        let conflict = acquire_lease_with_holder(
            &path,
            "lease-1",
            "controller-b",
            "2026-08-30T12:00:00+09:00",
            "2026-08-30T12:05:00+09:00",
        );
        assert_eq!(conflict["valid"], json!(false));
        assert_eq!(conflict["exit_reason"], json!("lease_already_held"));
        assert_eq!(conflict["data"]["acquired"], json!(false));
    }

    #[test]
    fn different_controllers_may_hold_different_leases_concurrently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        run_local_control_ledger(&ledger_input("init", &path));

        let first = acquire_lease_with_holder(
            &path,
            "lease-a",
            "controller-a",
            "2026-08-30T12:00:00+09:00",
            "2026-08-30T12:05:00+09:00",
        );
        let second = acquire_lease_with_holder(
            &path,
            "lease-b",
            "controller-b",
            "2026-08-30T12:00:00+09:00",
            "2026-08-30T12:05:00+09:00",
        );
        assert_eq!(first["data"]["acquired"], json!(true));
        assert_eq!(second["data"]["acquired"], json!(true));
    }

    #[test]
    fn expired_lease_is_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        run_local_control_ledger(&ledger_input("init", &path));

        let first = acquire_lease(
            &path,
            "lease-1",
            "2026-08-30T12:00:00+09:00",
            "2026-08-30T12:01:00+09:00",
        );
        assert_eq!(first["data"]["acquired"], json!(true));

        let second = acquire_lease(
            &path,
            "lease-2",
            "2026-08-30T12:02:00+09:00",
            "2026-08-30T12:07:00+09:00",
        );
        assert_eq!(second["valid"], json!(true));
        assert_eq!(second["data"]["acquired"], json!(true));
        assert_eq!(second["data"]["reclaimed_expired"], json!(true));
    }

    #[test]
    fn concurrent_acquire_same_lease_only_one_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("control.db"));
        run_local_control_ledger(&ledger_input("init", path.as_path()));

        let barrier = Arc::new(Barrier::new(2));
        let path_a = Arc::clone(&path);
        let barrier_a = Arc::clone(&barrier);
        let handle_a = thread::spawn(move || {
            barrier_a.wait();
            acquire_lease_with_holder(
                path_a.as_path(),
                "lease-shared",
                "controller-a",
                "2026-08-30T12:00:00+09:00",
                "2026-08-30T12:05:00+09:00",
            )
        });
        let path_b = Arc::clone(&path);
        let barrier_b = Arc::clone(&barrier);
        let handle_b = thread::spawn(move || {
            barrier_b.wait();
            acquire_lease_with_holder(
                path_b.as_path(),
                "lease-shared",
                "controller-b",
                "2026-08-30T12:00:00+09:00",
                "2026-08-30T12:05:00+09:00",
            )
        });

        let result_a = handle_a.join().unwrap();
        let result_b = handle_b.join().unwrap();
        let wins = [&result_a, &result_b]
            .iter()
            .filter(|r| r["data"]["acquired"] == json!(true))
            .count();
        assert_eq!(wins, 1);
    }

    #[test]
    fn claim_conflict_same_scope_returns_claimed_false() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        run_local_control_ledger(&ledger_input("init", &path));

        assert_eq!(
            claim_scope(
                &path,
                "scope-a",
                "claim-1",
                "controller-a",
                "2026-08-30T12:00:00+09:00",
                "2026-08-30T12:05:00+09:00",
            )["data"]["claimed"],
            json!(true)
        );

        let conflict = claim_scope(
            &path,
            "scope-a",
            "claim-2",
            "controller-b",
            "2026-08-30T12:00:00+09:00",
            "2026-08-30T12:05:00+09:00",
        );
        assert_eq!(conflict["valid"], json!(false));
        assert_eq!(conflict["exit_reason"], json!("scope_claim_conflict"));
        assert_eq!(conflict["data"]["claimed"], json!(false));
    }

    #[test]
    fn different_scopes_may_be_claimed_concurrently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        run_local_control_ledger(&ledger_input("init", &path));

        let first = claim_scope(
            &path,
            "scope-a",
            "claim-1",
            "controller-a",
            "2026-08-30T12:00:00+09:00",
            "2026-08-30T12:05:00+09:00",
        );
        let second = claim_scope(
            &path,
            "scope-b",
            "claim-2",
            "controller-b",
            "2026-08-30T12:00:00+09:00",
            "2026-08-30T12:05:00+09:00",
        );
        assert_eq!(first["data"]["claimed"], json!(true));
        assert_eq!(second["data"]["claimed"], json!(true));
    }

    #[test]
    fn decision_record_stores_hashes_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        run_local_control_ledger(&ledger_input("init", &path));
        let mut record = ledger_input("record_decision", &path);
        record["controller_id"] = json!("workflow_audit");
        record["decision_id"] = json!("dec-1");
        record["authority_mode"] = json!("observer");
        record["decision_code"] = json!("audit_preflight");
        record["hard_gate_pass"] = json!(true);
        record["result_code"] = json!("no_mutation");
        record["input_hash"] = json!("sha256:abc");
        record["output_hash"] = json!("sha256:def");
        record["recorded_at"] = json!("2026-08-30T12:00:00+09:00");
        assert_eq!(run_local_control_ledger(&record)["valid"], json!(true));
    }

    #[test]
    fn migrate_foundation_v1_preserves_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        materialize_foundation_v1(&path);
        let conn = open_migrated(&path);

        let schema_version: String = conn
            .query_row(
                "SELECT value FROM ledger_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_version, LEDGER_DB_SCHEMA_VERSION);

        let decision_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM decisions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(decision_count, 1);
        let rollback_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM rollback_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rollback_count, 1);
        let watermark_hash: String = conn
            .query_row(
                "SELECT value_hash FROM watermarks WHERE scope_key = 'scope-old' AND watermark_key = 'wm-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(watermark_hash, "sha256:wm");
        assert!(table_exists(&conn, "violation_events").unwrap());
        assert!(idempotency_pk_is_composite(&conn).unwrap());
    }

    #[test]
    fn migrate_authority_v1_preserves_rows_and_rebuilds_idempotency() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        materialize_authority_v1(&path);
        let conn = open_migrated(&path);

        let schema_version: String = conn
            .query_row(
                "SELECT value FROM ledger_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_version, LEDGER_DB_SCHEMA_VERSION);
        assert!(idempotency_pk_is_composite(&conn).unwrap());

        let idempotency_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM idempotency_claims", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(idempotency_count, 1);
        let authority_mode: String = conn
            .query_row(
                "SELECT authority_mode FROM violation_events WHERE event_id = 'vio-v1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(authority_mode, "shadow");

        let mut claim = ledger_input("claim_idempotency", &path);
        claim["controller_id"] = json!("workflow_audit");
        claim["decision_hash"] = json!("sha256:idempotent");
        claim["recorded_at"] = json!("2026-08-30T12:00:00+09:00");
        let duplicate = run_local_control_ledger(&claim);
        assert_eq!(duplicate["valid"], json!(false));
        assert_eq!(duplicate["data"]["duplicate"], json!(true));
    }

    #[test]
    fn migrate_v2_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        run_local_control_ledger(&ledger_input("init", &path));
        open_migrated(&path);
        let conn = open_migrated(&path);
        let schema_version: String = conn
            .query_row(
                "SELECT value FROM ledger_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_version, LEDGER_DB_SCHEMA_VERSION);
    }

    #[test]
    fn concurrent_different_scopes_both_win() {
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("control.db"));
        run_local_control_ledger(&ledger_input("init", path.as_path()));

        let barrier = Arc::new(Barrier::new(2));
        let path_a = Arc::clone(&path);
        let barrier_a = Arc::clone(&barrier);
        let handle_a = thread::spawn(move || {
            barrier_a.wait();
            claim_scope(
                path_a.as_path(),
                "scope-a",
                "claim-a",
                "controller-a",
                "2026-08-30T12:00:00+09:00",
                "2026-08-30T12:05:00+09:00",
            )
        });
        let path_b = Arc::clone(&path);
        let barrier_b = Arc::clone(&barrier);
        let handle_b = thread::spawn(move || {
            barrier_b.wait();
            claim_scope(
                path_b.as_path(),
                "scope-b",
                "claim-b",
                "controller-b",
                "2026-08-30T12:00:00+09:00",
                "2026-08-30T12:05:00+09:00",
            )
        });
        let result_a = handle_a.join().unwrap();
        let result_b = handle_b.join().unwrap();
        assert_eq!(result_a["data"]["claimed"], json!(true));
        assert_eq!(result_b["data"]["claimed"], json!(true));
    }

    #[test]
    fn concurrent_same_scope_claim_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("control.db"));
        run_local_control_ledger(&ledger_input("init", path.as_path()));

        let barrier = Arc::new(Barrier::new(2));
        let path_a = Arc::clone(&path);
        let barrier_a = Arc::clone(&barrier);
        let handle_a = thread::spawn(move || {
            barrier_a.wait();
            claim_scope(
                path_a.as_path(),
                "scope-shared",
                "claim-a",
                "controller-a",
                "2026-08-30T12:00:00+09:00",
                "2026-08-30T12:05:00+09:00",
            )
        });
        let path_b = Arc::clone(&path);
        let barrier_b = Arc::clone(&barrier);
        let handle_b = thread::spawn(move || {
            barrier_b.wait();
            claim_scope(
                path_b.as_path(),
                "scope-shared",
                "claim-b",
                "controller-b",
                "2026-08-30T12:00:00+09:00",
                "2026-08-30T12:05:00+09:00",
            )
        });
        let result_a = handle_a.join().unwrap();
        let result_b = handle_b.join().unwrap();
        let wins = [&result_a, &result_b]
            .iter()
            .filter(|r| r["data"]["claimed"] == json!(true))
            .count();
        assert_eq!(wins, 1);
    }

    #[test]
    fn promotion_metrics_filter_by_target_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.db");
        run_local_control_ledger(&ledger_input("init", &path));
        for (idx, mode) in ["observer", "safe_writer"].iter().enumerate() {
            let mut record = ledger_input("record_decision", &path);
            record["controller_id"] = json!("workflow_audit");
            record["decision_id"] = json!(format!("dec-{idx}"));
            record["authority_mode"] = json!(mode);
            record["decision_code"] = json!(format!("{mode}_shadow_run_once"));
            record["result_code"] = json!("ok");
            record["input_hash"] = json!(format!("sha256:in{idx}"));
            record["output_hash"] = json!(format!("sha256:out{idx}"));
            record["hard_gate_pass"] = json!(idx == 0);
            record["recorded_at"] = json!("2026-08-30T12:00:00+09:00");
            assert_eq!(run_local_control_ledger(&record)["valid"], json!(true));
        }
        let mut metrics = ledger_input("get_promotion_metrics", &path);
        metrics["controller_id"] = json!("workflow_audit");
        metrics["target_mode"] = json!("observer");
        metrics["since_ts"] = json!("2026-08-30T11:00:00+09:00");
        metrics["until_ts"] = json!("2026-08-30T13:00:00+09:00");
        let got = run_local_control_ledger(&metrics);
        assert_eq!(got["data"]["decision_count"], json!(1));
        assert_eq!(got["data"]["hard_gate_parity_pct"], json!(100));
    }
}
