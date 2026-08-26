//! Deterministic request materialization and dedupe audit App plugin.

use agentmesh_fixture_support::run_fixture;
use agentmesh_markdown_request_validator::request_materialization_audit::{
    audit_request_materialization, MATERIALIZATION_AUDIT_VERSION,
};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        MATERIALIZATION_AUDIT_VERSION,
        &[
            "request_materialization_audit",
            "same_scope_dedupe",
            "adapter_specific_partition",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: audit_request_materialization(&params.input),
            })
        }),
    )
}
