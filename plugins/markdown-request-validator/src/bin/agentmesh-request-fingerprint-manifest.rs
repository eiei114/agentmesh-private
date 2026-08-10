//! Deterministic request fingerprint manifest App plugin.

use agentmesh_fixture_support::run_fixture;
use agentmesh_markdown_request_validator::request_fingerprint_manifest::{
    fingerprint_request_manifest, FINGERPRINT_MANIFEST_VERSION,
};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        FINGERPRINT_MANIFEST_VERSION,
        &[
            "request_fingerprint_manifest",
            "deterministic_hashes",
            "json_markdown_hybrid_manifest",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: fingerprint_request_manifest(&params.input),
            })
        }),
    )
}
