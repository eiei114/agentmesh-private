//! Deterministic non-Multica request adapter App plugin.

use agentmesh_fixture_support::run_fixture;
use agentmesh_non_multica_request_adapter::{adapt_request_input, ADAPTER_VERSION};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        ADAPTER_VERSION,
        &["compact_output", "non_multica_request_adapter"],
        Box::new(|params| {
            Ok(RunResult {
                payload: adapt_request_input(&params.input),
            })
        }),
    )
}
