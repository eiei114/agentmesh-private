//! App-local SQLite control ledger plugin binary.

use agentmesh_fixture_support::run_fixture;
use agentmesh_local_control_ledger::{run_local_control_ledger, LOCAL_CONTROL_LEDGER_VERSION};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        LOCAL_CONTROL_LEDGER_VERSION,
        &["control_ledger"],
        Box::new(|params| {
            Ok(RunResult {
                payload: run_local_control_ledger(&params.input),
            })
        }),
    )
}
