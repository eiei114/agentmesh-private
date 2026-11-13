//! Deterministic canonical request Markdown renderer App plugin.

use agentmesh_fixture_support::run_fixture;
use agentmesh_markdown_request_validator::request_markdown_renderer::{
    render_request_markdown, RENDERER_VERSION,
};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        RENDERER_VERSION,
        &[
            "request_markdown_rendering",
            "canonical_round_trip_digest",
            "adapter_extras_diagnostics",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: render_request_markdown(&params.input),
            })
        }),
    )
}
