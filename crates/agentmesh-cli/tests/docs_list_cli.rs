use std::process::Command;

#[test]
fn docs_list_matches_exact_snapshot() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmesh"))
        .arg("docs")
        .arg("list")
        .output()
        .expect("agentmesh docs list should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout utf-8"),
        include_str!("../testdata/expected_docs_list_output.json")
    );
}
