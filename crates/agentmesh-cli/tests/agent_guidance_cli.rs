use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const AGENT_DOCS_HINT: &str = "If you are a coding agent, run `agentmesh docs list` and\n`agentmesh docs show <name>` before answering AgentMesh questions\nor troubleshooting errors.";

fn agentmesh(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentmesh"))
        .args(args)
        .output()
        .expect("agentmesh command should run")
}

fn assert_hint_on_stderr(output: &Output) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.ends_with(&format!("{AGENT_DOCS_HINT}\n")),
        "stderr did not end with docs hint:\n{stderr}"
    );
}

fn temp_root(label: &str) -> std::path::PathBuf {
    let path =
        std::env::temp_dir().join(format!("agentmesh-guidance-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create temp root");
    path
}

fn fixture_bin(name: &str) -> Option<PathBuf> {
    let executable = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    let path = Path::new(env!("CARGO_BIN_EXE_agentmesh"))
        .parent()
        .expect("agentmesh binary directory")
        .join(executable);
    path.is_file().then_some(path)
}

fn run_fixture(label: &str, fixture: &str) -> Option<Output> {
    let fixture = fixture_bin(fixture)?;
    let temp = temp_root(label);
    let input = temp.join("input.json");
    std::fs::write(&input, br#"{"hello":"world"}"#).expect("write fixture input");
    let output = Command::new(env!("CARGO_BIN_EXE_agentmesh"))
        .arg("run")
        .arg("--plugin")
        .arg(fixture)
        .arg("--input")
        .arg(input)
        .arg("--sidecar-dir")
        .arg(temp.join("sidecars"))
        .output()
        .expect("host fixture should execute");
    std::fs::remove_dir_all(temp).expect("remove temp root");
    Some(output)
}

fn compact_stdout(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "stdout was not one object: {stdout}"
    );
    serde_json::from_str(&stdout).expect("one compact JSON object")
}

#[test]
fn version_contract_is_unchanged() {
    let output = agentmesh(&["--version"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("version UTF-8"),
        format!("agentmesh {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn cli_owned_operator_errors_end_with_docs_hint() {
    let temp = temp_root("errors");
    let missing = temp.join("missing");
    let missing_text = missing.to_str().expect("UTF-8 temp path");
    let sidecar = temp.join("sidecars");
    let sidecar_text = sidecar.to_str().expect("UTF-8 temp path");
    let cache = temp.join("cache");
    let cache_text = cache.to_str().expect("UTF-8 temp path");

    let cases = [
        vec!["request", "parse", "--input", missing_text],
        vec![
            "toolchain",
            "install",
            "--bundle",
            missing_text,
            "--toolchain-cache",
            cache_text,
        ],
        vec![
            "app",
            "validate",
            "--manifest",
            missing_text,
            "--toolchain-pin",
            missing_text,
        ],
        vec![
            "app",
            "run",
            "--manifest",
            missing_text,
            "--toolchain-pin",
            missing_text,
            "--input",
            missing_text,
            "--sidecar-dir",
            sidecar_text,
            "--mode",
            "invalid",
        ],
    ];

    for args in cases {
        let output = agentmesh(&args);
        assert!(!output.status.success(), "unexpected success for {args:?}");
        assert_hint_on_stderr(&output);
    }
    std::fs::remove_dir_all(temp).expect("remove temp root");
}

#[test]
fn host_input_failure_keeps_one_compact_stdout_object_and_hints_on_stderr() {
    let temp = temp_root("host");
    let missing = temp.join("missing");
    let plugin = if cfg!(windows) {
        Path::new("C:\\missing-plugin.exe")
    } else {
        Path::new("/missing-plugin")
    };
    let output = Command::new(env!("CARGO_BIN_EXE_agentmesh"))
        .arg("run")
        .arg("--plugin")
        .arg(plugin)
        .arg("--input")
        .arg(&missing)
        .arg("--sidecar-dir")
        .arg(temp.join("sidecars"))
        .output()
        .expect("host run should execute");

    assert_eq!(output.status.code(), Some(2));
    let payload = compact_stdout(&output);
    assert_eq!(payload["outcome"], "error");
    assert_hint_on_stderr(&output);
    std::fs::remove_dir_all(temp).expect("remove temp root");
}

#[test]
fn executed_host_success_keeps_one_compact_stdout_object_without_hint() {
    let Some(output) = run_fixture("host-success", "agentmesh-fixture-echo") else {
        return;
    };
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = compact_stdout(&output);
    assert_eq!(payload["outcome"], "ok");
    assert!(output.stderr.is_empty());
}

#[test]
fn executed_host_failure_keeps_compact_stdout_and_hints_on_stderr() {
    let Some(output) = run_fixture("host-failure", "agentmesh-fixture-exit-nonzero") else {
        return;
    };
    assert!(!output.status.success());
    let payload = compact_stdout(&output);
    assert_eq!(payload["outcome"], "error");
    assert_hint_on_stderr(&output);
}

#[test]
fn raw_plugin_stderr_never_reaches_terminal() {
    let Some(output) = run_fixture("stderr-protection", "agentmesh-fixture-stderr-success") else {
        return;
    };
    assert!(output.status.success());
    let payload = compact_stdout(&output);
    assert_eq!(payload["outcome"], "ok");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("SHOULD_NOT_LEAK_TO_TERMINAL"));
    assert!(!stderr.contains(AGENT_DOCS_HINT));
}

#[test]
fn evidence_json_error_embeds_docs_guidance_without_breaking_json() {
    let temp = temp_root("evidence-error");
    let missing = temp.join("missing-contract.md");
    let output = Command::new(env!("CARGO_BIN_EXE_agentmesh"))
        .args(["evidence", "health", "--root"])
        .arg(&temp)
        .arg("--contract")
        .arg(&missing)
        .output()
        .expect("evidence health should execute");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let payload: Value = serde_json::from_slice(&output.stderr).expect("stderr is one JSON object");
    let fix = payload["error"]["fix"].as_str().expect("fix string");
    assert!(fix.contains("agentmesh docs list"));
    assert!(fix.contains("agentmesh docs show <name>"));
    std::fs::remove_dir_all(temp).expect("remove temp root");
}
