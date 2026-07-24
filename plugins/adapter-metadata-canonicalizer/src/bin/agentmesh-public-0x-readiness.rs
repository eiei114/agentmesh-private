//! Deterministic public 0.x readiness evidence gate App plugin.

use agentmesh_adapter_metadata_canonicalizer::{
    evaluate_public_0x_readiness_input, PUBLIC_0X_READINESS_VERSION,
};
use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        PUBLIC_0X_READINESS_VERSION,
        &[
            "compact_output",
            "public_0x_readiness_gate",
            "adapter_parity_evidence",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: evaluate_public_0x_readiness_input(&params.input),
            })
        }),
    )
}
