//! Deterministic lane-run ledger App plugin.

use agentmesh_fixture_support::run_fixture;
use agentmesh_lane_run_ledger::{run_lane_ledger, LANE_RUN_LEDGER_VERSION};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        LANE_RUN_LEDGER_VERSION,
        &["lane_run_record", "orphan_classify"],
        Box::new(|params| {
            Ok(RunResult {
                payload: run_lane_ledger(&params.input),
            })
        }),
    )
}
