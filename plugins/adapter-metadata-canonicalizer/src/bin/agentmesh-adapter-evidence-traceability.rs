//! Deterministic adapter evidence traceability App plugin.

use agentmesh_adapter_metadata_canonicalizer::{
    build_adapter_evidence_traceability_input, ADAPTER_EVIDENCE_TRACEABILITY_VERSION,
};
use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        ADAPTER_EVIDENCE_TRACEABILITY_VERSION,
        &[
            "compact_output",
            "adapter_evidence_traceability",
            "traceability_graph",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: build_adapter_evidence_traceability_input(&params.input),
            })
        }),
    )
}
