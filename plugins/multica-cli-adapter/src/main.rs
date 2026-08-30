//! Pinned Multica CLI adapter plugin binary.

use agentmesh_fixture_support::run_fixture;
use agentmesh_multica_cli_adapter::{
    run_multica_cli_adapter, OsProcessRunner, MULTICA_CLI_ADAPTER_VERSION,
};
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        MULTICA_CLI_ADAPTER_VERSION,
        &["multica_cli_probe", "multica_cli_invoke"],
        Box::new(|params| {
            Ok(RunResult {
                payload: run_multica_cli_adapter(&params.input, &OsProcessRunner),
            })
        }),
    )
}
