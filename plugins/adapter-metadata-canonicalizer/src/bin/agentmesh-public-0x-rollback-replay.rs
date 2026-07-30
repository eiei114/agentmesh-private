//! Deterministic public 0.x rollback replay evidence App plugin.

use agentmesh_adapter_metadata_canonicalizer::{
    evaluate_public_0x_rollback_replay_input, PUBLIC_0X_ROLLBACK_REPLAY_VERSION,
};
use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        PUBLIC_0X_ROLLBACK_REPLAY_VERSION,
        &[
            "compact_output",
            "rollback_replay_evidence",
            "adapter_digest_parity",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: evaluate_public_0x_rollback_replay_input(&params.input),
            })
        }),
    )
}
