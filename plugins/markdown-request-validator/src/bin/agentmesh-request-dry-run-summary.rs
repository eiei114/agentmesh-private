//! Deterministic request dry-run summary App plugin.

use agentmesh_fixture_support::run_fixture;
use agentmesh_markdown_request_validator::request_dry_run_summary::{
    summarize_request_dry_run, SUMMARY_VERSION,
};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        SUMMARY_VERSION,
        &[
            "request_dry_run_summary",
            "deterministic_markdown_preview",
            "normalized_json_evidence",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: summarize_request_dry_run(&params.input),
            })
        }),
    )
}
