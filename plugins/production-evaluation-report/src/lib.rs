//! Evaluation report for local production control rollout.
//!
//! Produces 7-day rollback gate and 30-day result summaries from compact
//! aggregate inputs and optional ledger-derived counters.

use serde::Deserialize;
use serde_json::{json, Value};

/// Plugin/schema version exposed in compact output.
pub const PRODUCTION_EVALUATION_REPORT_VERSION: &str = "production-evaluation-report.v0";
const INPUT_SCHEMA_VERSION: &str = "production-evaluation-report-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "production-evaluation-report-output.v0";

const TOKEN_REDUCTION_TARGET_PCT: f64 = 25.0;
const FAILURE_REWORK_MAX_PP: f64 = 2.0;
const THROUGHPUT_DECLINE_MAX_PCT: f64 = 10.0;
const ATTRIBUTION_COVERAGE_MIN_PCT: f64 = 90.0;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AggregateWindow {
    window_days: u64,
    decision_count: u64,
    token_baseline: f64,
    token_current: f64,
    failure_rate_baseline_pct: f64,
    failure_rate_current_pct: f64,
    throughput_baseline: f64,
    throughput_current: f64,
    attribution_coverage_pct: f64,
    duplicate_count: u64,
    unauthorized_count: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluationInput {
    schema_version: String,
    operation: String,
    controller_id: String,
    rollback_window: AggregateWindow,
    result_window: AggregateWindow,
}

fn issue(code: &str, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn pct_reduction(baseline: f64, current: f64) -> Result<f64, &'static str> {
    if baseline <= 0.0 {
        Err("token_baseline_nonpositive")
    } else {
        Ok(((baseline - current) / baseline) * 100.0)
    }
}

fn pct_decline(baseline: f64, current: f64) -> Result<f64, &'static str> {
    if baseline <= 0.0 {
        Err("throughput_baseline_nonpositive")
    } else {
        Ok(((baseline - current) / baseline) * 100.0)
    }
}

fn evaluate_window(window: &AggregateWindow, is_rollback: bool) -> Result<Value, &'static str> {
    let token_reduction_pct = pct_reduction(window.token_baseline, window.token_current)?;
    let failure_delta_pp = window.failure_rate_current_pct - window.failure_rate_baseline_pct;
    let throughput_decline_pct =
        pct_decline(window.throughput_baseline, window.throughput_current)?;
    let token_pass = token_reduction_pct >= TOKEN_REDUCTION_TARGET_PCT;
    let failure_pass = failure_delta_pp <= FAILURE_REWORK_MAX_PP;
    let throughput_pass = throughput_decline_pct <= THROUGHPUT_DECLINE_MAX_PCT;
    let attribution_pass = window.attribution_coverage_pct >= ATTRIBUTION_COVERAGE_MIN_PCT;
    let duplicate_pass = window.duplicate_count == 0;
    let unauthorized_pass = window.unauthorized_count == 0;
    let pass = token_pass
        && failure_pass
        && throughput_pass
        && attribution_pass
        && duplicate_pass
        && unauthorized_pass;
    Ok(json!({
        "window_days": window.window_days,
        "decision_count": window.decision_count,
        "token_reduction_pct": token_reduction_pct,
        "token_reduction_target_pct": TOKEN_REDUCTION_TARGET_PCT,
        "token_pass": token_pass,
        "failure_delta_pp": failure_delta_pp,
        "failure_rework_max_pp": FAILURE_REWORK_MAX_PP,
        "failure_pass": failure_pass,
        "throughput_decline_pct": throughput_decline_pct,
        "throughput_decline_max_pct": THROUGHPUT_DECLINE_MAX_PCT,
        "throughput_pass": throughput_pass,
        "attribution_coverage_pct": window.attribution_coverage_pct,
        "attribution_coverage_min_pct": ATTRIBUTION_COVERAGE_MIN_PCT,
        "attribution_pass": attribution_pass,
        "duplicate_count": window.duplicate_count,
        "duplicate_pass": duplicate_pass,
        "unauthorized_count": window.unauthorized_count,
        "unauthorized_pass": unauthorized_pass,
        "gate_pass": pass,
        "gate_kind": if is_rollback { "rollback_7d" } else { "final_gate_30d" },
    }))
}

fn compact(
    operation: &str,
    valid: bool,
    exit_reason: &str,
    issues: Vec<Value>,
    report: Value,
) -> Value {
    json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "app_version": PRODUCTION_EVALUATION_REPORT_VERSION,
        "operation": operation,
        "valid": valid,
        "exit_reason": exit_reason,
        "issue_count": issues.len(),
        "issues": issues,
        "report": report,
    })
}

/// Run evaluation report from compact aggregate inputs.
pub fn run_production_evaluation_report(value: &Value) -> Value {
    let Ok(input) = serde_json::from_value::<EvaluationInput>(value.clone()) else {
        return compact(
            "evaluate",
            false,
            "input_invalid",
            vec![issue(
                "input_invalid",
                "input must match production-evaluation-report-input.v0",
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
    if operation != "evaluate" {
        issues.push(issue("unknown_operation", "operation must be evaluate"));
    }
    if input.controller_id.is_empty() {
        issues.push(issue("controller_id_missing", "controller_id is required"));
    }
    if input.rollback_window.window_days != 7 {
        issues.push(issue(
            "rollback_window_invalid",
            "rollback_window.window_days must be 7",
        ));
    }
    if input.result_window.window_days != 30 {
        issues.push(issue(
            "result_window_invalid",
            "result_window.window_days must be 30",
        ));
    }
    if !issues.is_empty() {
        return compact(operation, false, "input_invalid", issues, json!(null));
    }

    let rollback = match evaluate_window(&input.rollback_window, true) {
        Ok(v) => v,
        Err(code) => {
            return compact(
                operation,
                false,
                code,
                vec![issue(code, "rollback window baseline invalid")],
                json!(null),
            );
        }
    };
    let result = match evaluate_window(&input.result_window, false) {
        Ok(v) => v,
        Err(code) => {
            return compact(
                operation,
                false,
                code,
                vec![issue(code, "result window baseline invalid")],
                json!(null),
            );
        }
    };
    let rollback_pass = rollback["gate_pass"] == json!(true);
    let result_pass = result["gate_pass"] == json!(true);
    let overall_pass = rollback_pass && result_pass;
    let report = json!({
        "controller_id": input.controller_id,
        "rollback_gate_7d": rollback,
        "final_gate_30d": result,
        "overall_pass": overall_pass,
    });
    compact(
        operation,
        overall_pass,
        if overall_pass {
            "evaluation_pass"
        } else if !rollback_pass {
            "rollback_gate_7d_failed"
        } else {
            "final_gate_30d_failed"
        },
        if overall_pass {
            Vec::new()
        } else {
            vec![issue(
                "evaluation_threshold_failed",
                "one or more evaluation thresholds failed",
            )]
        },
        report,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_window(days: u64) -> Value {
        json!({
            "window_days": days,
            "decision_count": 100,
            "token_baseline": 1000.0,
            "token_current": 700.0,
            "failure_rate_baseline_pct": 5.0,
            "failure_rate_current_pct": 6.0,
            "throughput_baseline": 100.0,
            "throughput_current": 95.0,
            "attribution_coverage_pct": 95.0,
            "duplicate_count": 0,
            "unauthorized_count": 0
        })
    }

    #[test]
    fn passing_evaluation_meets_thresholds() {
        let input = json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": "evaluate",
            "controller_id": "workflow_audit",
            "rollback_window": sample_window(7),
            "result_window": sample_window(30),
        });
        let output = run_production_evaluation_report(&input);
        assert_eq!(output["valid"], json!(true));
        assert_eq!(output["exit_reason"], json!("evaluation_pass"));
        assert_eq!(output["report"]["overall_pass"], json!(true));
    }

    #[test]
    fn failing_token_reduction_blocks_pass() {
        let mut rollback = sample_window(7);
        rollback["token_current"] = json!(900.0);
        let input = json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": "evaluate",
            "controller_id": "workflow_audit",
            "rollback_window": rollback,
            "result_window": sample_window(30),
        });
        let output = run_production_evaluation_report(&input);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["exit_reason"], json!("rollback_gate_7d_failed"));
    }

    #[test]
    fn unauthorized_count_fails_gate() {
        let mut result = sample_window(30);
        result["unauthorized_count"] = json!(1);
        let input = json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": "evaluate",
            "controller_id": "workflow_audit",
            "rollback_window": sample_window(7),
            "result_window": result,
        });
        let output = run_production_evaluation_report(&input);
        assert_eq!(
            output["report"]["final_gate_30d"]["unauthorized_pass"],
            json!(false)
        );
    }

    #[test]
    fn zero_token_baseline_fails_closed() {
        let mut rollback = sample_window(7);
        rollback["token_baseline"] = json!(0.0);
        let input = json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": "evaluate",
            "controller_id": "workflow_audit",
            "rollback_window": rollback,
            "result_window": sample_window(30),
        });
        let output = run_production_evaluation_report(&input);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["exit_reason"], json!("token_baseline_nonpositive"));
    }
}
