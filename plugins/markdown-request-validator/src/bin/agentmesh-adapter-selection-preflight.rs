//! Adapter selection preflight App entry point.
use agentmesh_fixture_support::run_fixture;
use agentmesh_markdown_request_validator::adapter_selection_preflight::{select_adapter, VERSION};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        VERSION,
        &["adapter_selection_preflight"],
        Box::new(|params| {
            Ok(RunResult {
                payload: select_adapter(&params.input),
            })
        }),
    )
}
