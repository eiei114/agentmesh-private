//! Deterministic Markdown request validator App plugin.

use agentmesh_fixture_support::run_fixture;
use agentmesh_markdown_request_validator::{validate_request_input, VALIDATOR_VERSION};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        VALIDATOR_VERSION,
        &["compact_output", "markdown_request_validation"],
        Box::new(|params| {
            Ok(RunResult {
                payload: validate_request_input(&params.input),
            })
        }),
    )
}
