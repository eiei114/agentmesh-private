//! Deterministic adapter selection decision record App plugin.

use agentmesh_adapter_metadata_canonicalizer::{
    build_adapter_selection_decision_record, ADAPTER_SELECTION_DECISION_RECORD_VERSION,
};
use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        ADAPTER_SELECTION_DECISION_RECORD_VERSION,
        &["compact_output", "adapter_selection_decision_record"],
        Box::new(|params| {
            Ok(RunResult {
                payload: build_adapter_selection_decision_record(&params.input),
            })
        }),
    )
}
