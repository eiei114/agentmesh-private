//! Deterministic local tracker adapter App plugin.

use agentmesh_fixture_support::run_fixture;
use agentmesh_local_tracker_adapter::{adapt_request_input, ADAPTER_VERSION};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        ADAPTER_VERSION,
        &["compact_output", "local_tracker_adapter"],
        Box::new(|params| {
            Ok(RunResult {
                payload: adapt_request_input(&params.input),
            })
        }),
    )
}
