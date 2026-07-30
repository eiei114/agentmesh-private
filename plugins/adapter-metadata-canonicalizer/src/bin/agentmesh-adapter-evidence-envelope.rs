//! Deterministic adapter evidence envelope App plugin.

use agentmesh_adapter_metadata_canonicalizer::{
    build_adapter_evidence_envelope_input, ADAPTER_EVIDENCE_ENVELOPE_VERSION,
};
use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        ADAPTER_EVIDENCE_ENVELOPE_VERSION,
        &[
            "compact_output",
            "adapter_evidence_envelope",
            "adapter_parity_evidence",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: build_adapter_evidence_envelope_input(&params.input),
            })
        }),
    )
}
