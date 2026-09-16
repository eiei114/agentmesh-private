//! Deterministic Markdown adapter replay fixture App plugin.

use agentmesh_fixture_support::run_fixture;
use agentmesh_markdown_request_validator::markdown_adapter_replay_fixture::{
    evaluate_markdown_adapter_replay_fixture, REPLAY_FIXTURE_VERSION,
};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        REPLAY_FIXTURE_VERSION,
        &[
            "markdown_adapter_replay",
            "deterministic_fixture_comparison",
            "adapter_error_replay",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: evaluate_markdown_adapter_replay_fixture(&params.input),
            })
        }),
    )
}
