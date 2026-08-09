use serde_json::Value;
use std::process::Command;

const AGENT_DOCS_HINT: &str = "If you are a coding agent, run `agentmesh docs list` and\n`agentmesh docs show <name>` before answering AgentMesh questions\nor troubleshooting errors.";

const CATALOG_NAMES: [&str; 6] = [
    "agentmesh-app-v0",
    "backlog-promoter-snapshot-v0",
    "protocol-v0",
    "public-0x-readiness-gate",
    "public-0x-readiness-report",
    "threat-model-v0",
];

fn show(name: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_agentmesh"))
        .args(["docs", "show", name])
        .output()
        .expect("agentmesh docs show should run")
}

#[test]
fn every_catalog_document_can_be_shown_by_exact_name() {
    let listed = Command::new(env!("CARGO_BIN_EXE_agentmesh"))
        .args(["docs", "list"])
        .output()
        .expect("agentmesh docs list should run");
    assert!(listed.status.success());
    let catalog: Value = serde_json::from_slice(&listed.stdout).expect("valid list JSON");
    let documents = catalog["results"].as_array().expect("results array");

    for name in CATALOG_NAMES {
        let output = show(name);
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let payload: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
        let listed_document = documents
            .iter()
            .find(|document| document["name"] == name)
            .expect("name exists in docs list");
        assert_eq!(payload["schema_version"], "agentmesh-docs-show.v0");
        assert_eq!(payload["name"], name);
        assert_eq!(payload["description"], listed_document["description"]);
        assert_eq!(payload["source"], listed_document["source"]);
        assert!(payload["content"]
            .as_str()
            .is_some_and(|content| !content.is_empty()));
    }
}

#[test]
fn protocol_content_is_embedded_and_independent_of_working_directory() {
    let cwd = std::env::temp_dir().join(format!("agentmesh-docs-show-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).expect("temporary cwd");
    let output = Command::new(env!("CARGO_BIN_EXE_agentmesh"))
        .args(["docs", "show", "protocol-v0"])
        .current_dir(&cwd)
        .output()
        .expect("agentmesh docs show should run from foreign cwd");
    std::fs::remove_dir_all(&cwd).expect("remove temporary cwd");
    assert!(output.status.success());
    let payload: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(payload["source"], "docs/protocol-v0.md");
    assert_eq!(
        payload["content"],
        include_str!("../../../docs/protocol-v0.md")
    );
}

#[test]
fn unknown_case_unicode_and_path_names_are_deterministic_not_found_errors() {
    for name in [
        "PROTOCOL-V0",
        "存在しない",
        "../README.md",
        "docs/protocol-v0.md",
        "a/b",
    ] {
        let output = show(name);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty());
        let payload: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
        assert_eq!(payload["schema_version"], "agentmesh-docs-error.v0");
        assert_eq!(payload["error"]["code"], "document_not_found");
        assert_eq!(
            payload["error"]["message"],
            format!("Unknown document name: {name}")
        );
        assert_eq!(payload["error"]["name"], name);
    }
}

#[test]
fn missing_name_is_rejected_without_machine_readable_stdout() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmesh"))
        .args(["docs", "show"])
        .output()
        .expect("agentmesh docs show should reject missing name");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("required arguments"));
}

#[test]
fn top_level_help_points_agents_to_embedded_docs() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmesh"))
        .arg("--help")
        .output()
        .expect("agentmesh help should run");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf-8");
    assert!(stdout.contains(AGENT_DOCS_HINT));
}
