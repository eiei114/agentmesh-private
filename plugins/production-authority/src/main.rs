//! Production authority plugin binary.

use agentmesh_fixture_support::run_fixture;
use agentmesh_multica_cli_adapter::OsProcessRunner;
use agentmesh_production_authority::{run_production_authority, PRODUCTION_AUTHORITY_VERSION};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        PRODUCTION_AUTHORITY_VERSION,
        &["production_authority"],
        Box::new(|params| {
            Ok(RunResult {
                payload: run_production_authority(&params.input, &OsProcessRunner),
            })
        }),
    )
}
