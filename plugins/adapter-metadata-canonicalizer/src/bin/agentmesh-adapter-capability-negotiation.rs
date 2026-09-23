//! Deterministic adapter capability negotiation App plugin.

use agentmesh_adapter_metadata_canonicalizer::{
    negotiate_adapter_capabilities, ADAPTER_CAPABILITY_NEGOTIATION_VERSION,
};
use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        ADAPTER_CAPABILITY_NEGOTIATION_VERSION,
        &["compact_output", "adapter_capability_negotiation"],
        Box::new(|params| {
            Ok(RunResult {
                payload: negotiate_adapter_capabilities(&params.input),
            })
        }),
    )
}
