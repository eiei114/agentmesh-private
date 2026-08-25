//! Deterministic adapter parity report App plugin.

use agentmesh_adapter_metadata_canonicalizer::{
    build_adapter_parity_report_input, ADAPTER_PARITY_REPORT_VERSION,
};
use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        ADAPTER_PARITY_REPORT_VERSION,
        &[
            "compact_output",
            "adapter_parity_report",
            "adapter_parity_diagnostics",
        ],
        Box::new(|params| {
            Ok(RunResult {
                payload: build_adapter_parity_report_input(&params.input),
            })
        }),
    )
}
