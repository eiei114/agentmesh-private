//! Deterministic normalized adapter error contract App plugin.

use agentmesh_fixture_support::run_fixture;
use agentmesh_markdown_request_validator::adapter_error_contract::{
    normalize_adapter_errors, ERROR_CONTRACT_VERSION,
};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        ERROR_CONTRACT_VERSION,
        &[
            "adapter_error_normalization",
            "markdown_request_validation",
            "external_failure_mapping",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: normalize_adapter_errors(&params.input),
            })
        }),
    )
}
