//! Production evaluation report plugin binary.

use agentmesh_fixture_support::run_fixture;
use agentmesh_production_evaluation_report::{
    run_production_evaluation_report, PRODUCTION_EVALUATION_REPORT_VERSION,
};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        PRODUCTION_EVALUATION_REPORT_VERSION,
        &["production_evaluation_report"],
        Box::new(|params| {
            Ok(RunResult {
                payload: run_production_evaluation_report(&params.input),
            })
        }),
    )
}
