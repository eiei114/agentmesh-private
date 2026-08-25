//! Deterministic request Markdown normalizer App plugin.

use agentmesh_fixture_support::run_fixture;
use agentmesh_markdown_request_validator::request_markdown_normalizer::{
    normalize_request_markdown, NORMALIZER_VERSION,
};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        NORMALIZER_VERSION,
        &[
            "request_markdown_normalization",
            "deterministic_projection",
            "stable_request_slug",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: normalize_request_markdown(&params.input),
            })
        }),
    )
}
