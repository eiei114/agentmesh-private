//! Deterministic adapter metadata comparison and canonicalization App plugin.

use agentmesh_adapter_metadata_canonicalizer::{canonicalize_metadata_input, APP_VERSION};
use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        APP_VERSION,
        &["compact_output", "adapter_metadata_canonicalizer"],
        Box::new(|params| {
            Ok(RunResult {
                payload: canonicalize_metadata_input(&params.input),
            })
        }),
    )
}
