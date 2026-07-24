use std::process::Command;

fn run_parse(input: &str) -> std::process::Output {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Command::new(env!("CARGO_BIN_EXE_agentmesh"))
        .arg("request")
        .arg("parse")
        .arg("--input")
        .arg(manifest_dir.join("testdata").join(input))
        .output()
        .expect("agentmesh request parse should run")
}

#[test]
fn request_parse_valid_fixture_matches_exact_snapshot() {
    let output = run_parse("valid_request_input.json");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        include_str!("../testdata/expected_valid_request_parse_output.json")
    );
}

#[test]
fn request_parse_invalid_fixture_matches_exact_snapshot() {
    let output = run_parse("invalid_request_input.json");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        include_str!("../testdata/expected_invalid_request_parse_output.json")
    );
}
