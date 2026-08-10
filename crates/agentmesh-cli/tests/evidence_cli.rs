use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn evidence_help_exposes_compile_and_health() {
    Command::cargo_bin("agentmesh")
        .unwrap()
        .args(["evidence", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("compile"))
        .stdout(predicate::str::contains("health"))
        .stdout(predicate::str::contains("evaluate"));
}

#[test]
fn compile_help_exposes_all_qmd_stream_controls() {
    Command::cargo_bin("agentmesh")
        .unwrap()
        .args(["evidence", "compile", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--qmd-command"))
        .stdout(predicate::str::contains("--adaptive-command"))
        .stdout(predicate::str::contains("--no-adaptive"))
        .stdout(predicate::str::contains("--decision-scope"));
}
