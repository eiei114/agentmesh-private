//! Deterministic post-dogfood public 0.x readiness report App plugin.

use agentmesh_adapter_metadata_canonicalizer::{
    evaluate_public_0x_readiness_report_input, PUBLIC_0X_READINESS_REPORT_VERSION,
};
use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        PUBLIC_0X_READINESS_REPORT_VERSION,
        &[
            "compact_output",
            "public_0x_readiness_report",
            "adapter_envelope_consistency",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: evaluate_public_0x_readiness_report_input(&params.input),
            })
        }),
    )
}
