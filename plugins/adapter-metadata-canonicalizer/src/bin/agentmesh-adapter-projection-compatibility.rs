//! Deterministic adapter projection compatibility App plugin.

use agentmesh_adapter_metadata_canonicalizer::{
    build_adapter_projection_compatibility, ADAPTER_PROJECTION_COMPATIBILITY_VERSION,
};
use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        ADAPTER_PROJECTION_COMPATIBILITY_VERSION,
        &["compact_output", "adapter_projection_compatibility"],
        Box::new(|params| {
            Ok(RunResult {
                payload: build_adapter_projection_compatibility(&params.input),
            })
        }),
    )
}
