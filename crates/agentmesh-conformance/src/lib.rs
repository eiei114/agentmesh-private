//! Internal conformance helpers and workspace boundary checks.

use std::path::PathBuf;
use std::process::Command;

/// Resolve a built fixture binary from `CARGO_TARGET_DIR` / relative target.
///
/// Probes plain `debug`/`release` plus every `target/<triple>/{debug,release}`
/// layout so tests find binaries under `cargo test --workspace --target <triple>`.
pub fn fixture_bin(name: &str) -> PathBuf {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
        let dir = PathBuf::from(dir);
        candidates.push(dir.join("debug").join(&exe));
        candidates.push(dir.join("release").join(&exe));
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                candidates.push(entry.path().join("debug").join(&exe));
                candidates.push(entry.path().join("release").join(&exe));
            }
        }
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_root = manifest_dir.join("../../target");
    candidates.push(target_root.join("debug").join(&exe));
    candidates.push(target_root.join("release").join(&exe));
    if let Ok(entries) = std::fs::read_dir(&target_root) {
        for entry in entries.flatten() {
            candidates.push(entry.path().join("debug").join(&exe));
            candidates.push(entry.path().join("release").join(&exe));
        }
    }
    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(&exe))
}

/// Run `cargo metadata` and assert fixture-support does not depend on host/conformance.
pub fn assert_fixture_dependency_boundary() {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(workspace_root())
        .output()
        .expect("cargo metadata");
    assert!(output.status.success(), "cargo metadata failed");
    let meta: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    let packages = meta["packages"].as_array().expect("packages");
    for pkg in packages {
        let name = pkg["name"].as_str().unwrap_or("");
        let is_plugin_side = name == "agentmesh-fixture-support"
            || name.starts_with("agentmesh-fixture-")
            || name.starts_with("agentmesh-multica-")
            || name.starts_with("agentmesh-markdown-");
        if is_plugin_side {
            let deps = pkg["dependencies"].as_array().cloned().unwrap_or_default();
            for dep in deps {
                let dep_name = dep["name"].as_str().unwrap_or("");
                assert_ne!(
                    dep_name, "agentmesh-host",
                    "{name} must not depend on agentmesh-host"
                );
                assert_ne!(
                    dep_name, "agentmesh-conformance",
                    "{name} must not depend on agentmesh-conformance"
                );
            }
        }
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentmesh_host::execute_run_with;
    use agentmesh_host::lifecycle::RunConfig;
    use agentmesh_host::sidecar::VecCompactSink;
    use agentmesh_host::{CancellationToken, FsAuditStore};
    use agentmesh_proto::failure::FailureCode;
    use agentmesh_proto::{CompactOutcome, Limits};
    use std::time::Duration;

    #[test]
    fn fixture_crates_do_not_depend_on_host_or_conformance() {
        assert_fixture_dependency_boundary();
    }

    fn abs_fixture(name: &str) -> PathBuf {
        let p = fixture_bin(name);
        std::fs::canonicalize(&p).unwrap_or(p)
    }

    #[tokio::test]
    async fn echo_roundtrip_success() {
        let plugin = abs_fixture("agentmesh-fixture-echo");
        if !plugin.exists() {
            eprintln!("skip: fixture not built at {}", plugin.display());
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let mut limits = Limits::default();
        limits.run_timeout_ms = 5_000;
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input: br#"{"hello":"world"}"#.to_vec(),
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits,
                run_id: Some("test-echo-1".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(
            outcome.exit_code,
            0,
            "stdout={}",
            String::from_utf8_lossy(&sink.bytes)
        );
        assert_eq!(outcome.envelope.outcome, CompactOutcome::Ok);
        assert!(outcome.sidecar_path.is_some());
    }

    #[tokio::test]
    async fn bad_json_maps_to_invalid_json_or_schema() {
        let plugin = abs_fixture("agentmesh-fixture-bad-json");
        if !plugin.exists() {
            eprintln!("skip: fixture not built");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let mut limits = Limits::default();
        limits.initialize_timeout_ms = 3_000;
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input: br#"{}"#.to_vec(),
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits,
                run_id: Some("test-bad-json".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;
        assert_ne!(outcome.exit_code, 0);
        let code = outcome.envelope.diagnostics[0].code.unwrap();
        assert!(
            matches!(
                code,
                FailureCode::InvalidJson
                    | FailureCode::SchemaViolation
                    | FailureCode::RpcIdMismatch
            ),
            "got {code}"
        );
    }

    #[tokio::test]
    async fn sleep_fixture_times_out_while_draining_stderr() {
        let plugin = abs_fixture("agentmesh-fixture-sleep");
        if !plugin.exists() {
            eprintln!("skip: fixture not built");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let mut limits = Limits::default();
        limits.initialize_timeout_ms = 5_000;
        limits.run_timeout_ms = 1_000;
        limits.exit_grace_ms = 500;
        let started = std::time::Instant::now();
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input: br#"{}"#.to_vec(),
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits,
                run_id: Some("test-sleep".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "deadlock suspected"
        );
        assert_eq!(
            outcome.envelope.diagnostics[0].code,
            Some(FailureCode::RunTimeout)
        );
    }

    #[tokio::test]
    async fn stderr_success_does_not_fail_run() {
        let plugin = abs_fixture("agentmesh-fixture-stderr-success");
        if !plugin.exists() {
            eprintln!("skip: fixture not built");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input: br#"{"x":1}"#.to_vec(),
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits: Limits::default(),
                run_id: Some("test-stderr-ok".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(outcome.exit_code, 0);
        let text = String::from_utf8_lossy(&sink.bytes);
        assert!(!text.contains("SHOULD_NOT_LEAK_TO_TERMINAL"));
    }

    #[test]
    fn path_helper_smoke() {
        let _ = fixture_bin("agentmesh-fixture-echo");
    }

    #[tokio::test]
    async fn markdown_request_validator_valid_roundtrip() {
        use agentmesh_markdown_request_validator::validate_request_input;

        let plugin = abs_fixture("agentmesh-markdown-request-validator");
        if !plugin.exists() {
            eprintln!(
                "skip: markdown validator plugin not built at {}",
                plugin.display()
            );
            return;
        }
        let testdata = workspace_root().join("plugins/markdown-request-validator/testdata");
        let input = std::fs::read(testdata.join("valid_request_input.json")).unwrap();
        let expected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(testdata.join("expected_valid_compact_payload.json")).unwrap(),
        )
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let mut limits = Limits::default();
        limits.run_timeout_ms = 5_000;
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input,
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits,
                run_id: Some("test-markdown-validator-valid".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;

        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.envelope.outcome, CompactOutcome::Ok);
        assert_eq!(outcome.envelope.payload, expected);
        assert_eq!(
            validate_request_input(
                &serde_json::from_slice(
                    &std::fs::read(testdata.join("valid_request_input.json")).unwrap()
                )
                .unwrap()
            ),
            expected
        );
    }

    #[tokio::test]
    async fn markdown_request_validator_invalid_roundtrip() {
        let plugin = abs_fixture("agentmesh-markdown-request-validator");
        if !plugin.exists() {
            eprintln!(
                "skip: markdown validator plugin not built at {}",
                plugin.display()
            );
            return;
        }
        let testdata = workspace_root().join("plugins/markdown-request-validator/testdata");
        let input = std::fs::read(testdata.join("invalid_request_input.json")).unwrap();
        let expected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(testdata.join("expected_invalid_compact_payload.json")).unwrap(),
        )
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input,
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits: Limits::default(),
                run_id: Some("test-markdown-validator-invalid".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;

        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.envelope.outcome, CompactOutcome::Ok);
        assert_eq!(outcome.envelope.payload, expected);
    }

    #[tokio::test]
    async fn adapter_metadata_canonicalizer_matching_roundtrip() {
        use agentmesh_adapter_metadata_canonicalizer::canonicalize_metadata_input;

        let plugin = abs_fixture("agentmesh-adapter-metadata-canonicalizer");
        if !plugin.exists() {
            eprintln!(
                "skip: adapter metadata canonicalizer plugin not built at {}",
                plugin.display()
            );
            return;
        }
        let testdata = workspace_root().join("plugins/adapter-metadata-canonicalizer/testdata");
        let input = std::fs::read(testdata.join("matching_metadata_input.json")).unwrap();
        let expected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(testdata.join("expected_matching_compact_payload.json")).unwrap(),
        )
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let mut limits = Limits::default();
        limits.run_timeout_ms = 5_000;
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input: input.clone(),
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits,
                run_id: Some("test-adapter-metadata-canonicalizer".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;

        assert_eq!(
            outcome.exit_code,
            0,
            "stdout={}",
            String::from_utf8_lossy(&sink.bytes)
        );
        assert_eq!(outcome.envelope.outcome, CompactOutcome::Ok);
        assert_eq!(outcome.envelope.payload, expected);
        assert_eq!(
            canonicalize_metadata_input(&serde_json::from_slice(&input).unwrap()),
            expected
        );
    }

    #[tokio::test]
    async fn multica_selector_shadow_empty_backlog_roundtrip() {
        use agentmesh_multica_selector_shadow::compare_compact_shadow;

        let plugin = abs_fixture("agentmesh-multica-selector-shadow");
        if !plugin.exists() {
            eprintln!("skip: shadow plugin not built at {}", plugin.display());
            return;
        }
        let testdata = workspace_root().join("plugins/multica-selector-shadow/testdata");
        let input = std::fs::read(testdata.join("recorded_empty_backlog_input.json")).unwrap();
        let expected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(testdata.join("expected_empty_backlog_compact_payload.json")).unwrap(),
        )
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let mut limits = Limits::default();
        limits.run_timeout_ms = 5_000;
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input,
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits,
                run_id: Some("test-multica-shadow-empty".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;

        assert_eq!(
            outcome.exit_code,
            0,
            "stdout={}",
            String::from_utf8_lossy(&sink.bytes)
        );
        assert_eq!(outcome.envelope.outcome, CompactOutcome::Ok);
        let sidecar = outcome.sidecar_path.expect("sidecar path");
        assert!(sidecar.is_file(), "missing sidecar {}", sidecar.display());
        compare_compact_shadow(&outcome.envelope.payload, &expected)
            .expect("shadow compact payload mismatch");
    }

    #[tokio::test]
    async fn multica_selector_shadow_one_candidate_roundtrip() {
        use agentmesh_multica_selector_shadow::compare_compact_shadow;

        let plugin = abs_fixture("agentmesh-multica-selector-shadow");
        if !plugin.exists() {
            eprintln!("skip: shadow plugin not built at {}", plugin.display());
            return;
        }
        let testdata = workspace_root().join("plugins/multica-selector-shadow/testdata");
        let input = std::fs::read(testdata.join("recorded_one_candidate_input.json")).unwrap();
        let expected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(testdata.join("expected_one_candidate_compact_payload.json")).unwrap(),
        )
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let mut limits = Limits::default();
        limits.run_timeout_ms = 5_000;
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input,
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits,
                run_id: Some("test-multica-shadow-one".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;

        assert_eq!(
            outcome.exit_code,
            0,
            "stdout={}",
            String::from_utf8_lossy(&sink.bytes)
        );
        assert_eq!(outcome.envelope.outcome, CompactOutcome::Ok);
        assert!(outcome.sidecar_path.expect("sidecar").is_file());
        compare_compact_shadow(&outcome.envelope.payload, &expected)
            .expect("shadow compact payload mismatch");
    }
}

#[cfg(test)]
mod lane_run_ledger_tests {
    use super::*;
    use agentmesh_host::execute_run_with;
    use agentmesh_host::lifecycle::RunConfig;
    use agentmesh_host::sidecar::VecCompactSink;
    use agentmesh_host::{CancellationToken, FsAuditStore};
    use agentmesh_proto::{CompactOutcome, Limits};

    fn abs_plugin(name: &str) -> PathBuf {
        let plugin = fixture_bin(name);
        assert!(
            plugin.exists(),
            "required plugin binary missing: {} (build it before running conformance tests)",
            plugin.display()
        );
        std::fs::canonicalize(&plugin).unwrap_or(plugin)
    }

    #[tokio::test]
    async fn lane_run_ledger_record_roundtrip() {
        use agentmesh_lane_run_ledger::run_lane_ledger;

        let plugin = abs_plugin("agentmesh-lane-run-ledger");
        if !plugin.exists() {
            eprintln!(
                "skip: lane-run-ledger plugin not built at {}",
                plugin.display()
            );
            return;
        }
        let testdata = workspace_root().join("plugins/lane-run-ledger/testdata");
        let input = std::fs::read(testdata.join("valid_record_input.json")).unwrap();
        let expected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(testdata.join("expected_valid_record_payload.json")).unwrap(),
        )
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let mut limits = Limits::default();
        limits.run_timeout_ms = 5_000;
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input,
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits,
                run_id: Some("test-lane-run-ledger-record".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;

        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.envelope.outcome, CompactOutcome::Ok);
        assert_eq!(outcome.envelope.payload, expected);
        assert_eq!(
            run_lane_ledger(
                &serde_json::from_slice(
                    &std::fs::read(testdata.join("valid_record_input.json")).unwrap()
                )
                .unwrap()
            ),
            expected
        );
    }

    #[tokio::test]
    async fn lane_run_ledger_classify_roundtrip() {
        use agentmesh_lane_run_ledger::run_lane_ledger;

        let plugin = abs_plugin("agentmesh-lane-run-ledger");
        if !plugin.exists() {
            eprintln!(
                "skip: lane-run-ledger plugin not built at {}",
                plugin.display()
            );
            return;
        }
        let testdata = workspace_root().join("plugins/lane-run-ledger/testdata");
        for (input_name, expected_name) in [
            ("classify_input.json", "expected_classify_payload.json"),
            (
                "invalid_result_input.json",
                "expected_invalid_result_payload.json",
            ),
            ("malformed_input.json", "expected_malformed_payload.json"),
            (
                "unknown_field_input.json",
                "expected_unknown_field_payload.json",
            ),
        ] {
            let input = std::fs::read(testdata.join(input_name)).unwrap();
            let expected: serde_json::Value =
                serde_json::from_slice(&std::fs::read(testdata.join(expected_name)).unwrap())
                    .unwrap_or_else(|err| panic!("bad fixture {expected_name}: {err}"));

            let dir = tempfile::tempdir().unwrap();
            let mut sink = VecCompactSink::default();
            let outcome = execute_run_with(
                RunConfig {
                    plugin: plugin.clone(),
                    input,
                    sidecar_dir: dir.path().to_path_buf(),
                    plugin_env_keys: vec![],
                    redact_pointers: vec![],
                    capture_plugin_stderr: false,
                    limits: Limits::default(),
                    run_id: Some(format!("test-lane-run-ledger-{input_name}")),
                },
                &FsAuditStore,
                &mut sink,
                CancellationToken::new(),
            )
            .await;

            assert_eq!(outcome.exit_code, 0, "{input_name}");
            assert_eq!(outcome.envelope.payload, expected, "{input_name}");
            assert_eq!(
                run_lane_ledger(
                    &serde_json::from_slice(&std::fs::read(testdata.join(input_name)).unwrap())
                        .unwrap()
                ),
                expected,
                "{input_name}"
            );
        }
    }
}
