//! AgentMesh CLI entrypoint (Phase 0 host + App v0 validate/run).

mod docs;
mod request_parse;

use agentmesh_app::{
    default_toolchain_cache_root, install_toolchain_bundle, prepare_app_run, validate_app_bundle,
    write_run_marker, AppRunMode, AppRunRequest, ResolveMode,
};
use agentmesh_evidence::{
    compile as compile_evidence, evaluate as evaluate_evidence, health as evidence_health,
    load_contract as load_evidence_contract, secure_source_path, CommandSpec, CompileOptions,
    DecisionScope, EvidenceKind, EvidenceMode, EvidenceRequest,
};
use agentmesh_host::audit::FsAuditStore;
use agentmesh_host::lifecycle::{CancellationToken, RunConfig};
use agentmesh_host::sidecar::{CompactSink, CompactSinkError};
use agentmesh_host::{execute_run, execute_run_with};
use agentmesh_proto::{CompactDiagnostic, CompactEnvelope, FailureCode, Limits};
use clap::{Args, Parser, Subcommand};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const AGENT_DOCS_HINT: &str = "If you are a coding agent, run `agentmesh docs list` and\n`agentmesh docs show <name>` before answering AgentMesh questions\nor troubleshooting errors.";

#[derive(Debug, Default)]
struct BufferCompactSink {
    bytes: Vec<u8>,
}

impl CompactSink for BufferCompactSink {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), CompactSinkError> {
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "agentmesh",
    version,
    about = "AgentMesh one-shot host and App tooling",
    after_help = AGENT_DOCS_HINT
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
    /// AgentMesh App authoring / packaging commands.
    App {
        #[command(subcommand)]
        command: AppCommands,
    },
    /// Stable request parsing commands for adapter handoff.
    Request {
        #[command(subcommand)]
        command: RequestCommands,
    },
    /// Local toolchain cache install commands.
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommands,
    },
    /// Discover embedded AgentMesh documentation.
    Docs {
        #[command(subcommand)]
        command: DocsCommands,
    },
    /// Read-only source-linked Decision and AgentRun evidence compilation.
    Evidence {
        #[command(subcommand)]
        command: EvidenceCommands,
    },
}

#[derive(Debug, Subcommand)]
enum EvidenceCommands {
    /// Fuse keyword, semantic, and adaptive QMD candidates into an Evidence Packet.
    Compile(Box<EvidenceCompileArgs>),
    /// Validate contract and optional graph without running a query.
    Health(EvidenceHealthArgs),
    /// Run the canonical blind evaluation without persisting query text.
    Evaluate(Box<EvidenceEvaluateArgs>),
}

#[derive(Debug, Args)]
struct EvidenceCompileArgs {
    /// Canonical vault root.
    #[arg(long)]
    root: PathBuf,
    /// Vault-relative canonical Markdown contract.
    #[arg(long)]
    contract: PathBuf,
    /// Vault-relative UTF-8 query file. Query text is never emitted or persisted.
    #[arg(long = "query-file")]
    query_file: PathBuf,
    /// Packet kind: Decision or AgentRun.
    #[arg(long)]
    kind: String,
    /// Reviewed namespace key.
    #[arg(long)]
    namespace: String,
    /// Maximum source sensitivity.
    #[arg(long, default_value = "internal")]
    sensitivity_ceiling: String,
    /// Retrieval mode: qmd-only or hybrid.
    #[arg(long, default_value = "hybrid")]
    mode: String,
    /// Decision lifecycle scope: current, review, or historical.
    #[arg(
        long,
        default_value = "current",
        value_parser = ["current", "review", "historical"]
    )]
    decision_scope: String,
    /// Optional vault-relative serving-valid v2 graph.
    #[arg(long)]
    graph: Option<PathBuf>,
    /// QMD collection name.
    #[arg(long, default_value = "obsidian-note")]
    collection: String,
    /// qmd executable, npm shim, or Node entrypoint.
    #[arg(long, default_value = "qmd")]
    qmd_command: PathBuf,
    /// qmd-adaptive-search executable, npm shim, or Node entrypoint.
    #[arg(long, default_value = "qmd-adaptive-search")]
    adaptive_command: PathBuf,
    /// Disable adaptive discovery when a read-only-capable command is unavailable.
    #[arg(long)]
    no_adaptive: bool,
    /// Maximum emitted sources.
    #[arg(long, default_value_t = 6)]
    max_sources: usize,
    /// Shared hard timeout in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    timeout_ms: u64,
}

#[derive(Debug, Args)]
struct EvidenceHealthArgs {
    /// Canonical vault root.
    #[arg(long)]
    root: PathBuf,
    /// Vault-relative canonical Markdown contract.
    #[arg(long)]
    contract: PathBuf,
    /// Optional vault-relative serving-valid v2 graph.
    #[arg(long)]
    graph: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct EvidenceEvaluateArgs {
    /// Canonical vault root.
    #[arg(long)]
    root: PathBuf,
    /// Vault-relative canonical Markdown contract.
    #[arg(long)]
    contract: PathBuf,
    /// Optional vault-relative serving-valid v2 graph.
    #[arg(long)]
    graph: Option<PathBuf>,
    /// QMD collection name.
    #[arg(long, default_value = "obsidian-note")]
    collection: String,
    /// qmd executable, npm shim, or Node entrypoint.
    #[arg(long, default_value = "qmd")]
    qmd_command: PathBuf,
    /// qmd-adaptive-search executable, npm shim, or Node entrypoint.
    #[arg(long, default_value = "qmd-adaptive-search")]
    adaptive_command: PathBuf,
    /// Disable adaptive discovery.
    #[arg(long)]
    no_adaptive: bool,
    /// Repetitions per mode and fixture.
    #[arg(long, default_value_t = 3)]
    repeat: usize,
}

#[derive(Debug, Subcommand)]
enum AppCommands {
    /// Validate `agentmesh-app.toml` with a version-controlled toolchain pin.
    Validate {
        /// Path to `agentmesh-app.toml`.
        #[arg(long)]
        manifest: PathBuf,
        /// Path to toolchain pin TOML (tag/commit/target/manifest hash).
        #[arg(long = "toolchain-pin")]
        toolchain_pin: PathBuf,
        /// Emit machine-readable JSON report on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Run an App via pin→cache resolve (or development `--dev-plugin` override).
    Run {
        /// Path to `agentmesh-app.toml`.
        #[arg(long)]
        manifest: PathBuf,
        /// Path to toolchain pin TOML.
        #[arg(long = "toolchain-pin")]
        toolchain_pin: PathBuf,
        /// Path to input JSON.
        #[arg(long)]
        input: PathBuf,
        /// Parent directory for audit sidecars.
        #[arg(long)]
        sidecar_dir: PathBuf,
        /// Run mode: `production` (default) or `development`.
        #[arg(long, default_value = "production")]
        mode: String,
        /// Absolute path override for local plugin development (rejected in production).
        #[arg(long = "dev-plugin")]
        dev_plugin: Option<PathBuf>,
        /// Override toolchain cache root (default: `~/.agentmesh/toolchains` or env).
        #[arg(long = "toolchain-cache")]
        toolchain_cache: Option<PathBuf>,
        /// RFC 6901 JSON Pointers to redact in audit records.
        #[arg(long = "redact-pointer", value_name = "POINTER")]
        redact_pointer: Vec<String>,
        /// Run timeout override in milliseconds (1000..=3600000).
        #[arg(long)]
        run_timeout_ms: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
enum RequestCommands {
    /// Parse Markdown/JSON request input into the canonical AgentMesh request payload.
    Parse {
        /// Path to request parser input JSON.
        #[arg(long)]
        input: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum DocsCommands {
    /// List embedded AgentMesh documents available in this binary.
    List,
    /// Show one embedded AgentMesh document by exact catalog name.
    Show {
        /// Exact document name returned by `agentmesh docs list`.
        name: String,
    },
}

#[derive(Debug, Subcommand)]
enum ToolchainCommands {
    /// Atomically install a verified toolchain bundle into the local cache.
    Install {
        /// Path to an extracted toolchain bundle directory (`release-manifest.json` + `bin/`).
        #[arg(long)]
        bundle: PathBuf,
        /// Override toolchain cache root (default: `~/.agentmesh/toolchains` or env).
        #[arg(long = "toolchain-cache")]
        toolchain_cache: Option<PathBuf>,
        /// Emit machine-readable JSON report on stdout.
        #[arg(long)]
        json: bool,
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
        Commands::App { command } => match command {
            AppCommands::Validate {
                manifest,
                toolchain_pin,
                json,
            } => app_validate_command(manifest, toolchain_pin, json),
            AppCommands::Run {
                manifest,
                toolchain_pin,
                input,
                sidecar_dir,
                mode,
                dev_plugin,
                toolchain_cache,
                redact_pointer,
                run_timeout_ms,
            } => {
                match app_run_command(
                    manifest,
                    toolchain_pin,
                    input,
                    sidecar_dir,
                    mode,
                    dev_plugin,
                    toolchain_cache,
                    redact_pointer,
                    run_timeout_ms,
                )
                .await
                {
                    Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(70)),
                    Err(code) => ExitCode::from(u8::try_from(code).unwrap_or(70)),
                }
            }
        },
        Commands::Request { command } => match command {
            RequestCommands::Parse { input } => request_parse_command(input),
        },
        Commands::Toolchain { command } => match command {
            ToolchainCommands::Install {
                bundle,
                toolchain_cache,
                json,
            } => toolchain_install_command(bundle, toolchain_cache, json),
        },
        Commands::Docs { command } => match command {
            DocsCommands::List => docs::docs_list_command(),
            DocsCommands::Show { name } => docs::docs_show_command(&name),
        },
        Commands::Evidence { command } => evidence_command(command),
    }
}

fn evidence_command(command: EvidenceCommands) -> ExitCode {
    match command {
        EvidenceCommands::Compile(arguments) => {
            let EvidenceCompileArgs {
                root,
                contract,
                query_file,
                kind,
                namespace,
                sensitivity_ceiling,
                mode,
                decision_scope,
                graph,
                collection,
                qmd_command,
                adaptive_command,
                no_adaptive,
                max_sources,
                timeout_ms,
            } = *arguments;
            let result = (|| {
                let root = root
                    .canonicalize()
                    .map_err(|error| format!("resolve root: {error}"))?;
                let contract_path =
                    secure_source_path(&root, &contract).map_err(|error| error.to_string())?;
                let query_path =
                    secure_source_path(&root, &query_file).map_err(|error| error.to_string())?;
                let contract =
                    load_evidence_contract(&contract_path).map_err(|error| error.to_string())?;
                let query = read_bounded_query(&query_path, contract.max_query_bytes)?;
                let qmd = CommandSpec::resolve(&qmd_command).map_err(|error| error.to_string())?;
                let adaptive = if no_adaptive {
                    None
                } else {
                    Some(
                        CommandSpec::resolve(&adaptive_command)
                            .map_err(|error| error.to_string())?,
                    )
                };
                let request = EvidenceRequest {
                    kind: EvidenceKind::parse(&kind).map_err(|error| error.to_string())?,
                    query,
                    namespace,
                    sensitivity_ceiling,
                    max_sources,
                    timeout_ms,
                    decision_scope: DecisionScope::parse(&decision_scope)
                        .map_err(|error| format!("invalid request: {error}"))?,
                    mode: EvidenceMode::parse(&mode).map_err(|error| error.to_string())?,
                };
                compile_evidence(
                    &root,
                    &contract,
                    &request,
                    &CompileOptions {
                        collection,
                        qmd: Some(qmd),
                        adaptive,
                        graph_path: graph,
                    },
                )
                .map_err(|error| error.to_string())
            })();
            print_evidence_result(result)
        }
        EvidenceCommands::Health(EvidenceHealthArgs {
            root,
            contract,
            graph,
        }) => {
            let result = (|| {
                let root = root
                    .canonicalize()
                    .map_err(|error| format!("resolve root: {error}"))?;
                let contract_path =
                    secure_source_path(&root, &contract).map_err(|error| error.to_string())?;
                let contract =
                    load_evidence_contract(&contract_path).map_err(|error| error.to_string())?;
                Ok(evidence_health(&root, &contract, graph.as_deref()))
            })();
            print_evidence_result(result)
        }
        EvidenceCommands::Evaluate(arguments) => {
            let EvidenceEvaluateArgs {
                root,
                contract,
                graph,
                collection,
                qmd_command,
                adaptive_command,
                no_adaptive,
                repeat,
            } = *arguments;
            let result = (|| {
                let root = root
                    .canonicalize()
                    .map_err(|error| format!("resolve root: {error}"))?;
                let contract_path =
                    secure_source_path(&root, &contract).map_err(|error| error.to_string())?;
                let contract =
                    load_evidence_contract(&contract_path).map_err(|error| error.to_string())?;
                let qmd = CommandSpec::resolve(&qmd_command).map_err(|error| error.to_string())?;
                let adaptive = if no_adaptive {
                    None
                } else {
                    Some(
                        CommandSpec::resolve(&adaptive_command)
                            .map_err(|error| error.to_string())?,
                    )
                };
                evaluate_evidence(
                    &root,
                    &contract,
                    &CompileOptions {
                        collection,
                        qmd: Some(qmd),
                        adaptive,
                        graph_path: graph,
                    },
                    repeat,
                )
                .map_err(|error| error.to_string())
            })();
            print_evidence_result(result)
        }
    }
}

fn print_evidence_result(result: Result<serde_json::Value, String>) -> ExitCode {
    match result {
        Ok(payload) => match serde_json::to_string_pretty(&payload) {
            Ok(encoded) => {
                println!("{encoded}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "error": {"code": "serialization_error", "problem": error.to_string()}
                    })
                );
                ExitCode::from(2)
            }
        },
        Err(error) => {
            let code = classify_evidence_error(&error);
            eprintln!(
                "{}",
                serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": code,
                        "problem": error,
                        "cause": code,
                        "fix": "Run `agentmesh evidence health`, then verify QMD commands, contract, namespace, and graph paths. Coding agents should run `agentmesh docs list` and `agentmesh docs show <name>` for embedded guidance."
                    }
                })
            );
            ExitCode::from(2)
        }
    }
}

fn classify_evidence_error(problem: &str) -> &'static str {
    if problem.starts_with("invalid_request:") || problem.starts_with("invalid request:") {
        "invalid_request"
    } else if problem.starts_with("invalid contract:") {
        "invalid_contract"
    } else if problem.starts_with("invalid graph:") {
        "invalid_graph"
    } else if problem.starts_with("path rejected:") {
        "path_rejected"
    } else if problem.starts_with("command unavailable:") {
        "qmd_unavailable"
    } else if problem.contains("timed out") {
        "qmd_timeout"
    } else if problem.starts_with("command protocol error:") {
        "qmd_protocol_error"
    } else {
        "io_error"
    }
}

fn read_bounded_query(path: &std::path::Path, max_bytes: usize) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("open query file: {error}"))?;
    let mut bytes = Vec::new();
    file.take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read query file: {error}"))?;
    if bytes.len() > max_bytes {
        return Err(format!("invalid_request: query exceeds {max_bytes} bytes"));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| "invalid_request: query file must be UTF-8".to_owned())?;
    Ok(text.trim_start_matches('\u{feff}').to_owned())
}

fn request_parse_command(input: PathBuf) -> ExitCode {
    let bytes = match std::fs::read(&input) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("agentmesh request parse: input read failed: {err}");
            print_agent_docs_hint();
            return ExitCode::from(2);
        }
    };
    let (payload, valid) = request_parse::parse_request_input_bytes(&bytes);
    println!("{payload}");
    if valid {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}

fn init_tracing(verbose: bool) {
    let filter = if verbose { "info" } else { "warn" };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn toolchain_install_command(
    bundle: PathBuf,
    toolchain_cache: Option<PathBuf>,
    json: bool,
) -> ExitCode {
    let cache = match toolchain_cache {
        Some(path) => path,
        None => match default_toolchain_cache_root() {
            Ok(path) => path,
            Err(err) => {
                eprintln!("agentmesh toolchain install: {err}");
                print_agent_docs_hint();
                return ExitCode::from(2);
            }
        },
    };
    match install_toolchain_bundle(&bundle, &cache) {
        Ok(report) => {
            if json {
                let payload = serde_json::json!({
                    "ok": true,
                    "tag": report.tag,
                    "target": report.target,
                    "install_dir": report.install_dir,
                    "release_manifest_sha256": report.release_manifest_sha256,
                    "binary_count": report.binary_count,
                });
                println!("{payload}");
            } else {
                println!(
                    "ok installed tag={} target={} binaries={} release_manifest_sha256={}",
                    report.tag, report.target, report.binary_count, report.release_manifest_sha256
                );
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            if json {
                let payload = serde_json::json!({
                    "ok": false,
                    "error": err.to_string(),
                });
                println!("{payload}");
            } else {
                eprintln!("agentmesh toolchain install: {err}");
                print_agent_docs_hint();
            }
            ExitCode::from(2)
        }
    }
}

fn app_validate_command(manifest: PathBuf, toolchain_pin: PathBuf, json: bool) -> ExitCode {
    match validate_app_bundle(&manifest, &toolchain_pin) {
        Ok(report) => {
            if json {
                let payload = serde_json::json!({
                    "ok": true,
                    "app_name": report.app_name,
                    "plugin_logical_name": report.plugin_logical_name,
                    "pin_tag": report.pin_tag,
                    "pin_commit_sha": report.pin_commit_sha,
                    "pin_target": report.pin_target,
                    "protocol_version": report.protocol_version,
                });
                println!("{}", payload);
            } else {
                println!(
                    "ok app={} plugin={} pin={}@{} protocol={}",
                    report.app_name,
                    report.plugin_logical_name,
                    report.pin_tag,
                    report.pin_target,
                    report.protocol_version
                );
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            if json {
                let payload = serde_json::json!({
                    "ok": false,
                    "error": err.to_string(),
                });
                println!("{payload}");
            } else {
                eprintln!("agentmesh app validate: {err}");
                print_agent_docs_hint();
            }
            ExitCode::from(2)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn app_run_command(
    manifest: PathBuf,
    toolchain_pin: PathBuf,
    input: PathBuf,
    sidecar_dir: PathBuf,
    mode: String,
    dev_plugin: Option<PathBuf>,
    toolchain_cache: Option<PathBuf>,
    redact_pointer: Vec<String>,
    run_timeout_ms: Option<u64>,
) -> Result<i32, i32> {
    let mode = match AppRunMode::parse(&mode) {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("agentmesh app run: {msg}");
            print_agent_docs_hint();
            return Err(2);
        }
    };

    let prepared = match prepare_app_run(AppRunRequest {
        manifest_path: &manifest,
        pin_path: &toolchain_pin,
        mode,
        dev_plugin: dev_plugin.as_deref(),
        toolchain_cache: toolchain_cache.as_deref(),
    }) {
        Ok((_app, prepared)) => prepared,
        Err(err) => {
            eprintln!("agentmesh app run: {err}");
            print_agent_docs_hint();
            return Err(2);
        }
    };

    if let Err(err) = write_run_marker(&sidecar_dir, &prepared.run_marker) {
        eprintln!("agentmesh app run: {err}");
        print_agent_docs_hint();
        return Err(2);
    }

    let mut limits = prepared.limits;
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

    if input_bytes.len() > limits.input_max_bytes {
        emit_input_failure(
            "input-too-large",
            FailureCode::InputSchemaViolation,
            "input exceeds app/host input_max_bytes",
        );
        return Err(2);
    }

    let config = RunConfig {
        plugin: prepared.resolved.plugin_path.clone(),
        input: input_bytes,
        sidecar_dir,
        plugin_env_keys: prepared.plugin_env_keys,
        redact_pointers: redact_pointer,
        capture_plugin_stderr: prepared.capture_plugin_stderr,
        limits,
        run_id: None,
    };

    let mut sink = BufferCompactSink::default();
    let mut outcome =
        execute_run_with(config, &FsAuditStore, &mut sink, CancellationToken::new()).await;
    annotate_app_run_envelope(
        &mut outcome.envelope,
        &prepared.run_marker,
        prepared.resolved.mode,
    );
    let env_bytes = serde_json::to_vec(&outcome.envelope).unwrap_or_default();
    if let Err(e) = std::io::stdout().lock().write_all(&env_bytes) {
        eprintln!("agentmesh app run: stdout write failed: {e}");
        print_agent_docs_hint();
        return Err(70);
    }
    if outcome.exit_code != 0 {
        print_agent_docs_hint();
    }
    Ok(outcome.exit_code)
}

fn annotate_app_run_envelope(envelope: &mut CompactEnvelope, marker: &str, mode: ResolveMode) {
    envelope.diagnostics.push(CompactDiagnostic {
        category: None,
        code: None,
        message: marker.to_string(),
    });
    let _ = mode; // marker already encodes pinned vs unpinned
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
    if outcome.exit_code != 0 {
        print_agent_docs_hint();
    }
    Ok(outcome.exit_code)
}

fn emit_input_failure(run_id: &str, code: FailureCode, message: &str) {
    let env = CompactEnvelope::error(
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
    print_agent_docs_hint();
}

fn print_agent_docs_hint() {
    eprintln!("{AGENT_DOCS_HINT}");
}
