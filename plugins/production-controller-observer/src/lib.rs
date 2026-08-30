//! Observer-mode one-shot production controller wiring.
//!
//! Combines pinned Multica CLI adapter and app-local control ledger. Observer mode
//! performs read-only CLI probes and records bounded decision metadata only.

use agentmesh_local_control_ledger::run_local_control_ledger;
use agentmesh_multica_cli_adapter::{run_multica_cli_adapter, ProcessRunner};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Plugin/schema version exposed in compact output.
pub const PRODUCTION_CONTROLLER_OBSERVER_VERSION: &str = "production-controller-observer.v0";
const INPUT_SCHEMA_VERSION: &str = "production-controller-observer-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "production-controller-observer-output.v0";

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
    invoke_args: Vec<String>,
}

fn issue(code: &str, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn sha256_hex(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    format!("sha256:{}", hex::encode(digest))
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
    // Lease must remain active for the duration of the run; use a far-future expiry
    // because foundation input carries a deterministic `now` only.
    let expires_at = "2099-01-01T00:00:00Z".to_string();

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
        let reason = if lease["issues"][0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("lease_already_held"))
        {
            "lease_already_held"
        } else {
            "lease_acquire_failed"
        };
        return compact(
            operation,
            false,
            reason,
            vec![issue(reason, "could not acquire schedule lease")],
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
        }),
    );
    if claim["valid"] != json!(true) {
        let _ = ledger_op(&input, "release_lease", json!({"lease_id": lease_id}));
        return compact(
            operation,
            false,
            "scope_claim_failed",
            vec![issue("scope_claim_failed", "could not claim scope")],
            json!(null),
            claim,
        );
    }

    let authority = ledger_op(&input, "get_authority_mode", json!({}));
    if authority["valid"] != json!(true) {
        let _ = ledger_op(&input, "release_claim", json!({"scope_key": scope_key}));
        let _ = ledger_op(&input, "release_lease", json!({"lease_id": lease_id}));
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
        "operation": if input.invoke_args.is_empty() { "probe" } else { "invoke" },
        "cli_path": input.cli_path,
        "prefix_args": input.prefix_args,
        "invoke_args": input.invoke_args,
    });
    let cli = run_multica_cli_adapter(&cli_input, runner);

    let input_hash = sha256_hex(&cli_input);
    let output_hash = sha256_hex(&cli);
    let decision_id = format!("decision-{lease_id}");
    let result_code = cli["exit_reason"].as_str().unwrap_or("unknown");
    let decision = ledger_op(
        &input,
        "record_decision",
        json!({
            "decision_id": decision_id,
            "decision_code": "observer_run_once",
            "result_code": result_code,
            "input_hash": input_hash,
            "output_hash": output_hash,
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

    let _ = ledger_op(&input, "release_claim", json!({"scope_key": scope_key}));
    let _ = ledger_op(&input, "release_lease", json!({"lease_id": lease_id}));

    let exit_reason = if cli["valid"] == json!(true) && decision["valid"] == json!(true) {
        "observer_success_no_mutation".to_string()
    } else if cli["valid"] != json!(true) {
        cli["exit_reason"]
            .as_str()
            .unwrap_or("cli_failed")
            .to_string()
    } else {
        "decision_record_failed".to_string()
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
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentmesh_local_control_ledger::run_local_control_ledger;
    use agentmesh_multica_cli_adapter::{CliCommandSpec, CliInvokeResult};
    use std::fs;

    struct FakeRunner;

    impl ProcessRunner for FakeRunner {
        fn run(
            &self,
            _spec: &CliCommandSpec,
            _operation_args: &[String],
        ) -> Result<CliInvokeResult, String> {
            Ok(CliInvokeResult {
                exit_code: 0,
                stdout_json: Some(json!({"issues": []})),
                stdout_truncated: false,
                stderr_byte_count: 0,
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
        let output = run_production_controller_observer(&base_input(&dir), &FakeRunner);
        assert_eq!(output["valid"], json!(true));
        assert_eq!(output["exit_reason"], json!("observer_success_no_mutation"));
        assert_eq!(output["mutation_performed"], json!(false));
    }

    #[test]
    fn rejects_non_observer_authority() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = base_input(&dir);
        input["authority_mode"] = json!("safe_writer");
        let output = run_production_controller_observer(&input, &FakeRunner);
        assert_eq!(output["exit_reason"], json!("authority_not_observer"));
    }

    #[test]
    fn lease_conflict_has_named_exit_reason() {
        let dir = tempfile::tempdir().unwrap();
        let input = base_input(&dir);
        let hold = json!({
            "schema_version": "local-control-ledger-input.v0",
            "operation": "acquire_lease",
            "ledger_path": input["ledger_path"],
            "lease_id": "held-lease",
            "holder": "other-controller",
            "acquired_at": "2026-08-30T12:00:00+09:00",
            "expires_at": "2099-01-01T00:00:00Z",
        });
        run_local_control_ledger(&hold);
        let output = run_production_controller_observer(&input, &FakeRunner);
        assert_eq!(output["exit_reason"], json!("lease_already_held"));
    }
}
