//! AgentMesh Phase 0 CLI entrypoint.

use agentmesh_host::execute_run;
use agentmesh_host::lifecycle::RunConfig;
use agentmesh_proto::{FailureCode, Limits};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(
    name = "agentmesh",
    version,
    about = "AgentMesh Phase 0 contract spike CLI"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Increase host tracing on stderr (never prints plugin stderr).
    #[arg(long, global = true)]
    verbose: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Execute a one-shot plugin initialize/run/close lifecycle.
    Run {
        /// Absolute path to a native plugin executable.
        #[arg(long)]
        plugin: PathBuf,
        /// Path to input JSON (required).
        #[arg(long)]
        input: PathBuf,
        /// Parent directory for audit sidecars.
        #[arg(long)]
        sidecar_dir: PathBuf,
        /// Allowlisted environment variable NAMES (values from parent env).
        #[arg(long = "plugin-env", value_name = "KEY")]
        plugin_env: Vec<String>,
        /// RFC 6901 JSON Pointers to redact in audit records.
        #[arg(long = "redact-pointer", value_name = "POINTER")]
        redact_pointer: Vec<String>,
        /// Store bounded raw plugin stderr in the owner-only sidecar.
        #[arg(long)]
        capture_plugin_stderr: bool,
        /// Run timeout in milliseconds (1000..=3600000).
        #[arg(long)]
        run_timeout_ms: Option<u64>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Commands::Run {
            plugin,
            input,
            sidecar_dir,
            plugin_env,
            redact_pointer,
            capture_plugin_stderr,
            run_timeout_ms,
        } => match run_command(
            plugin,
            input,
            sidecar_dir,
            plugin_env,
            redact_pointer,
            capture_plugin_stderr,
            run_timeout_ms,
        )
        .await
        {
            Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(70)),
            Err(code) => ExitCode::from(u8::try_from(code).unwrap_or(70)),
        },
    }
}

fn init_tracing(verbose: bool) {
    let filter = if verbose { "info" } else { "warn" };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

async fn run_command(
    plugin: PathBuf,
    input: PathBuf,
    sidecar_dir: PathBuf,
    plugin_env: Vec<String>,
    redact_pointer: Vec<String>,
    capture_plugin_stderr: bool,
    run_timeout_ms: Option<u64>,
) -> Result<i32, i32> {
    let mut limits = Limits::default();
    if let Some(ms) = run_timeout_ms {
        match Limits::validate_run_timeout_ms(ms) {
            Ok(v) => limits.run_timeout_ms = v,
            Err(msg) => {
                emit_input_failure("invalid-timeout", FailureCode::InputSchemaViolation, msg);
                return Err(2);
            }
        }
    }

    let input_bytes = match std::fs::read(&input) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            emit_input_failure(
                "missing-input",
                FailureCode::InputMissing,
                "input path missing",
            );
            return Err(2);
        }
        Err(e) => {
            emit_input_failure(
                "input-read",
                FailureCode::InputReadFailed,
                &format!("input read failed: {e}"),
            );
            return Err(2);
        }
    };

    let config = RunConfig {
        plugin,
        input: input_bytes,
        sidecar_dir,
        plugin_env_keys: plugin_env,
        redact_pointers: redact_pointer,
        capture_plugin_stderr,
        limits,
        run_id: None,
    };

    let outcome = execute_run(config).await;
    // execute_run already wrote compact stdout (or stderr fallback).
    Ok(outcome.exit_code)
}

fn emit_input_failure(run_id: &str, code: FailureCode, message: &str) {
    let env = agentmesh_proto::CompactEnvelope::error(
        agentmesh_proto::PROTOCOL_VERSION,
        run_id,
        code.category(),
        code,
        message,
        vec![],
    );
    if let Ok(bytes) = serde_json::to_vec(&env) {
        let _ = std::io::Write::write_all(&mut std::io::stdout(), &bytes);
    }
    eprintln!(
        "agentmesh: run_id={run_id} code={} category={} message={message}",
        code,
        code.category()
    );
}
