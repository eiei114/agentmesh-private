//! Observer-mode production controller plugin binary.

use agentmesh_fixture_support::run_fixture;
use agentmesh_multica_cli_adapter::OsProcessRunner;
use agentmesh_production_controller_observer::{
    run_production_controller_observer, PRODUCTION_CONTROLLER_OBSERVER_VERSION,
};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        PRODUCTION_CONTROLLER_OBSERVER_VERSION,
        &["production_controller_observer"],
        Box::new(|params| {
            Ok(RunResult {
                payload: run_production_controller_observer(&params.input, &OsProcessRunner),
            })
        }),
    )
}
