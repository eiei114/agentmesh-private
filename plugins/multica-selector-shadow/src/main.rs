//! Shadow-mode Multica backlog selector plugin.
//!
//! Accepts recorded Multica backlog listings as opaque `agentmesh.run` input and
//! returns a Python-selector-shaped compact payload as opaque `payload`.

use agentmesh_fixture_support::run_fixture;
use agentmesh_multica_selector_shadow::{parse_input, select_compact_payload};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        "0.1.0",
        &["compact_output", "sidecar_refs"],
        Box::new(|params| match parse_input(&params.input) {
            Ok(input) => Ok(RunResult {
                payload: select_compact_payload(&input),
            }),
            Err(err) => Err((-32_000, err.to_string(), None)),
        }),
    )
}
