//! Observer-mode one-shot production controller wiring.
//!
//! Combines pinned Multica CLI adapter and app-local control ledger. Observer mode
//! performs read-only CLI probes and records bounded decision metadata only.

use agentmesh_local_control_ledger::run_local_control_ledger;
use agentmesh_multica_cli_adapter::{run_multica_cli_adapter, ProcessRunner};
use chrono::{DateTime, FixedOffset};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Plugin/schema version exposed in compact output.
pub const PRODUCTION_CONTROLLER_OBSERVER_VERSION: &str = "production-controller-observer.v0";
const INPUT_SCHEMA_VERSION: &str = "production-controller-observer-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "production-controller-observer-output.v0";
const DEFAULT_LEASE_TTL_SECS: u64 = 300;
const MIN_LEASE_TTL_SECS: u64 = 30;
const MAX_LEASE_TTL_SECS: u64 = 3600;
const DEFAULT_CLI_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserverInput {
    schema_version: String,
    operation: String,
    controller_id: String,
    authority_mode: String,
    ledger_path: String,
    cli_path: String,
    now: String,
    #[serde(default)]
    scope_key: Option<String>,
    #[serde(default)]
    lease_id: Option<String>,
    #[serde(default)]
    prefix_args: Vec<String>,
    #[serde(default)]
    lease_ttl_seconds: Option<u64>,
    #[serde(default)]
    cli_timeout_ms: Option<u64>,
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

fn compact(
    operation: &str,
    valid: bool,
    exit_reason: &str,
    issues: Vec<Value>,
    cli: Value,
    ledger: Value,
) -> Value {
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "app_version": PRODUCTION_CONTROLLER_OBSERVER_VERSION,
        "operation": operation,
        "valid": valid,
        "exit_reason": exit_reason,
        "mutation_performed": false,
        "issue_count": issues.len(),
        "issues": issues,
        "cli": cli,
        "ledger": ledger,
    })
}

fn ledger_op(base: &ObserverInput, operation: &str, extra: Value) -> Value {
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

fn persistence_exit_reason(decision: &Value, watermark: &Value) -> Option<&'static str> {
    if decision["valid"] != json!(true) {
        Some("decision_record_failed")
    } else if watermark["valid"] != json!(true) {
        Some("watermark_persist_failed")
    } else {
        None
    }
}

struct RunGuard<'a> {
    input: &'a ObserverInput,
    lease_id: String,
    scope_key: String,
    armed: bool,
}

impl<'a> RunGuard<'a> {
    fn new(input: &'a ObserverInput, lease_id: String, scope_key: String) -> Self {
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

/// Run observer one-shot wiring with injectable CLI runner for tests.
pub fn run_production_controller_observer(value: &Value, runner: &dyn ProcessRunner) -> Value {
    let Ok(input) = serde_json::from_value::<ObserverInput>(value.clone()) else {
        return compact(
            "run_once",
            false,
            "input_invalid",
            vec![issue(
                "input_invalid",
                "input must match production-controller-observer-input.v0",
            )],
            json!(null),
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
    if operation != "run_once" {
        issues.push(issue("unknown_operation", "operation must be run_once"));
    }
    if input.controller_id.is_empty() {
        issues.push(issue("controller_id_missing", "controller_id is required"));
    }
    if input.now.is_empty() {
        issues.push(issue("now_missing", "now is required"));
    }
    if let Err(message) = resolve_lease_ttl_seconds(input.lease_ttl_seconds) {
        issues.push(issue("lease_ttl_invalid", message));
    }
    if let Err(message) = resolve_cli_timeout_ms(input.cli_timeout_ms) {
        issues.push(issue("cli_timeout_invalid", message));
    }
    if !issues.is_empty() {
        return compact(
            operation,
            false,
            "input_invalid",
            issues,
            json!(null),
            json!(null),
        );
    }

    if input.authority_mode != "observer" {
        return compact(
            operation,
            false,
            "authority_not_observer",
            vec![issue(
                "authority_not_observer",
                "foundation slice supports observer mode only",
            )],
            json!(null),
            json!(null),
        );
    }

    let lease_ttl_seconds = resolve_lease_ttl_seconds(input.lease_ttl_seconds).unwrap();
    let cli_timeout_ms = resolve_cli_timeout_ms(input.cli_timeout_ms).unwrap();
    let expires_at = match lease_expires_at(&input.now, lease_ttl_seconds) {
        Ok(ts) => ts,
        Err(message) => {
            return compact(
                operation,
                false,
                "now_invalid",
                vec![issue("now_invalid", message)],
                json!(null),
                json!(null),
            );
        }
    };

    let init = ledger_op(&input, "init", json!({}));
    if init["valid"] != json!(true) {
        return compact(
            operation,
            false,
            "ledger_init_failed",
            vec![issue("ledger_init_failed", "ledger init failed")],
            json!(null),
            init,
        );
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
        &input,
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
            json!(null),
            lease,
        );
    }

    let claim = ledger_op(
        &input,
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
        let _ = ledger_op(&input, "release_lease", json!({"lease_id": lease_id}));
        return compact(
            operation,
            false,
            &reason,
            vec![issue(&reason, "could not claim scope")],
            json!(null),
            claim,
        );
    }

    let mut guard = RunGuard::new(&input, lease_id.clone(), scope_key.clone());

    let authority = ledger_op(&input, "get_authority_mode", json!({}));
    if authority["valid"] != json!(true) {
        return compact(
            operation,
            false,
            "authority_lookup_failed",
            vec![issue("authority_lookup_failed", "authority lookup failed")],
            json!(null),
            authority,
        );
    }

    let cli_input = json!({
        "schema_version": "multica-cli-adapter-input.v0",
        "operation": "query",
        "cli_path": input.cli_path,
        "prefix_args": input.prefix_args,
        "timeout_ms": cli_timeout_ms,
    });
    let decision_hash = sha256_hex(&json!({
        "controller_id": input.controller_id,
        "decision_code": "observer_run_once",
        "input_hash": sha256_hex(&cli_input),
    }));
    let idempotency = ledger_op(
        &input,
        "claim_idempotency",
        json!({"decision_hash": decision_hash}),
    );
    if idempotency["exit_reason"] == "duplicate_suppressed" {
        guard.disarm();
        let _ = ledger_op(&input, "release_claim", json!({"scope_key": scope_key}));
        let _ = ledger_op(&input, "release_lease", json!({"lease_id": lease_id}));
        return compact(
            operation,
            false,
            "duplicate_suppressed",
            vec![issue(
                "duplicate_suppressed",
                "duplicate observer run suppressed",
            )],
            json!(null),
            json!({"idempotency": idempotency}),
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
            json!(null),
            idempotency,
        );
    }

    let cli_raw = run_multica_cli_adapter(&cli_input, runner);
    let cli = redact_cli_summary(&cli_raw);

    let input_hash = sha256_hex(&cli_input);
    let output_hash = sha256_hex(&cli);
    let decision_id = format!("decision-{lease_id}");
    let result_code = cli["exit_reason"].as_str().unwrap_or("unknown");
    let decision = ledger_op(
        &input,
        "record_decision",
        json!({
            "decision_id": decision_id,
            "authority_mode": "observer",
            "decision_code": "observer_run_once",
            "result_code": result_code,
            "input_hash": input_hash,
            "output_hash": output_hash,
            "hard_gate_pass": cli["valid"] == json!(true),
        }),
    );

    let watermark = ledger_op(
        &input,
        "set_watermark",
        json!({
            "scope_key": scope_key,
            "watermark_key": "last_observer_run",
            "value_hash": output_hash,
        }),
    );

    guard.disarm();
    let _ = ledger_op(&input, "release_claim", json!({"scope_key": scope_key}));
    let _ = ledger_op(&input, "release_lease", json!({"lease_id": lease_id}));

    if let Some(reason) = persistence_exit_reason(&decision, &watermark) {
        return compact(
            operation,
            false,
            reason,
            vec![issue(reason, "ledger persistence failed")],
            cli,
            json!({
                "decision": decision,
                "watermark": watermark,
                "authority": authority,
                "idempotency": idempotency,
            }),
        );
    }

    let exit_reason = if cli["valid"] == json!(true) {
        "observer_success_no_mutation".to_string()
    } else {
        cli["exit_reason"]
            .as_str()
            .unwrap_or("cli_failed")
            .to_string()
    };
    let valid = exit_reason == "observer_success_no_mutation";

    compact(
        operation,
        valid,
        &exit_reason,
        if valid {
            Vec::new()
        } else {
            vec![issue(&exit_reason, "observer run did not complete cleanly")]
        },
        cli,
        json!({
            "decision": decision,
            "watermark": watermark,
            "authority": authority,
            "idempotency": idempotency,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentmesh_local_control_ledger::run_local_control_ledger;
    use agentmesh_multica_cli_adapter::{CliCommandSpec, CliInvokeResult};
    use std::fs;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct FakeRunner {
        exit_code: i32,
        stdout: Vec<u8>,
        timed_out: bool,
    }

    impl ProcessRunner for FakeRunner {
        fn run(
            &self,
            _spec: &CliCommandSpec,
            operation_args: &[String],
            _timeout_ms: u64,
        ) -> Result<CliInvokeResult, String> {
            assert_eq!(
                operation_args,
                &[
                    "issues".to_string(),
                    "list".to_string(),
                    "--json".to_string()
                ]
            );
            let bounded = self.stdout.as_slice();
            Ok(CliInvokeResult {
                exit_code: self.exit_code,
                stdout_json: serde_json::from_slice(bounded).ok(),
                stdout_sha256: format!("sha256:{}", hex::encode(Sha256::digest(bounded))),
                stdout_byte_count: bounded.len(),
                stdout_truncated: false,
                stderr_byte_count: 0,
                timed_out: self.timed_out,
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

    fn base_input(dir: &tempfile::TempDir) -> Value {
        let cli = dir.path().join("multica.exe");
        fs::write(&cli, b"fake").unwrap();
        let ledger = dir.path().join("control.db");
        json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": "run_once",
            "controller_id": "workflow_audit",
            "authority_mode": "observer",
            "ledger_path": ledger.to_string_lossy(),
            "cli_path": cli.canonicalize().unwrap().to_string_lossy(),
            "now": "2026-08-30T12:00:00+09:00",
        })
    }

    #[test]
    fn observer_run_once_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let output = run_production_controller_observer(
            &base_input(&dir),
            &FakeRunner {
                exit_code: 0,
                stdout: br#"{"issues":[]}"#.to_vec(),
                timed_out: false,
            },
        );
        assert_eq!(output["valid"], json!(true));
        assert_eq!(output["exit_reason"], json!("observer_success_no_mutation"));
        assert_eq!(output["mutation_performed"], json!(false));
        assert!(output["cli"].get("stdout_json").is_none());
        assert!(output["cli"]["stdout_sha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
    }

    #[test]
    fn duplicate_observer_run_suppresses_cli() {
        let dir = tempfile::tempdir().unwrap();
        let input = base_input(&dir);
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = CountedFakeRunner {
            calls: calls.clone(),
            exit_code: 0,
            stdout: br#"{"issues":[]}"#.to_vec(),
        };
        assert_eq!(
            run_production_controller_observer(&input, &runner)["valid"],
            json!(true)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let dup = run_production_controller_observer(&input, &runner);
        assert_eq!(dup["exit_reason"], json!("duplicate_suppressed"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rejects_non_observer_authority() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = base_input(&dir);
        input["authority_mode"] = json!("safe_writer");
        let output = run_production_controller_observer(
            &input,
            &FakeRunner {
                exit_code: 0,
                stdout: br#"{}"#.to_vec(),
                timed_out: false,
            },
        );
        assert_eq!(output["exit_reason"], json!("authority_not_observer"));
    }

    #[test]
    fn rejects_unknown_invoke_args_field() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = base_input(&dir);
        input["invoke_args"] = json!(["issues", "list"]);
        let output = run_production_controller_observer(
            &input,
            &FakeRunner {
                exit_code: 0,
                stdout: br#"{}"#.to_vec(),
                timed_out: false,
            },
        );
        assert_eq!(output["exit_reason"], json!("input_invalid"));
    }

    #[test]
    fn lease_conflict_has_named_exit_reason() {
        let dir = tempfile::tempdir().unwrap();
        let input = base_input(&dir);
        let hold = json!({
            "schema_version": "local-control-ledger-input.v0",
            "operation": "acquire_lease",
            "ledger_path": input["ledger_path"],
            "lease_id": "workflow_audit-2026-08-30T12:00:00+09:00",
            "holder": "other-controller",
            "acquired_at": "2026-08-30T12:00:00+09:00",
            "expires_at": "2026-08-30T12:30:00+09:00",
        });
        run_local_control_ledger(&hold);
        let output = run_production_controller_observer(
            &input,
            &FakeRunner {
                exit_code: 0,
                stdout: br#"{}"#.to_vec(),
                timed_out: false,
            },
        );
        assert_eq!(output["exit_reason"], json!("lease_already_held"));
    }

    #[test]
    fn expired_lease_is_reclaimed_before_run() {
        let dir = tempfile::tempdir().unwrap();
        let input = base_input(&dir);
        run_local_control_ledger(&json!({
            "schema_version": "local-control-ledger-input.v0",
            "operation": "init",
            "ledger_path": input["ledger_path"],
        }));
        run_local_control_ledger(&json!({
            "schema_version": "local-control-ledger-input.v0",
            "operation": "acquire_lease",
            "ledger_path": input["ledger_path"],
            "lease_id": "old-lease",
            "holder": "other-controller",
            "acquired_at": "2026-08-30T11:00:00+09:00",
            "expires_at": "2026-08-30T11:05:00+09:00",
        }));
        let output = run_production_controller_observer(
            &input,
            &FakeRunner {
                exit_code: 0,
                stdout: br#"{"issues":[]}"#.to_vec(),
                timed_out: false,
            },
        );
        assert_eq!(output["exit_reason"], json!("observer_success_no_mutation"));
    }

    #[test]
    fn cli_timeout_is_surfaced_without_raw_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let output = run_production_controller_observer(
            &base_input(&dir),
            &FakeRunner {
                exit_code: -1,
                stdout: br#"{"secret":"token"}"#.to_vec(),
                timed_out: true,
            },
        );
        assert_eq!(output["exit_reason"], json!("process_timeout"));
        assert!(output["cli"].get("stdout_json").is_none());
        assert_eq!(output["cli"]["timed_out"], json!(true));
    }

    #[test]
    fn persistence_failure_exit_reasons_are_deterministic() {
        let decision_fail = json!({"valid": false});
        let ok = json!({"valid": true});
        assert_eq!(
            persistence_exit_reason(&decision_fail, &ok),
            Some("decision_record_failed")
        );
        assert_eq!(
            persistence_exit_reason(&ok, &json!({"valid": false})),
            Some("watermark_persist_failed")
        );
        assert_eq!(persistence_exit_reason(&ok, &ok), None);
    }

    #[test]
    fn guard_releases_lease_so_second_run_can_acquire() {
        let dir = tempfile::tempdir().unwrap();
        let input = base_input(&dir);
        run_local_control_ledger(&json!({
            "schema_version": "local-control-ledger-input.v0",
            "operation": "init",
            "ledger_path": input["ledger_path"],
        }));

        struct FailingAfterClaimRunner;
        impl ProcessRunner for FailingAfterClaimRunner {
            fn run(
                &self,
                _spec: &CliCommandSpec,
                _operation_args: &[String],
                _timeout_ms: u64,
            ) -> Result<CliInvokeResult, String> {
                Ok(CliInvokeResult {
                    exit_code: 0,
                    stdout_json: Some(json!({"issues": []})),
                    stdout_sha256: "sha256:00".into(),
                    stdout_byte_count: 2,
                    stdout_truncated: false,
                    stderr_byte_count: 0,
                    timed_out: false,
                })
            }
        }

        let first = run_production_controller_observer(&input, &FailingAfterClaimRunner);
        assert_eq!(first["valid"], json!(true));

        let second = run_production_controller_observer(&input, &FailingAfterClaimRunner);
        assert_eq!(second["exit_reason"], json!("duplicate_suppressed"));
        assert_ne!(second["exit_reason"], json!("lease_already_held"));
    }
}
