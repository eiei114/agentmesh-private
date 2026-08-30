//! Pinned absolute Multica CLI adapter.
//!
//! Plugin-owned production boundary: invokes one configured Multica CLI executable
//! without shell expansion. AgentMesh never copies Multica auth tokens into its state.

use hex::encode;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Plugin/schema version exposed in compact output.
pub const MULTICA_CLI_ADAPTER_VERSION: &str = "multica-cli-adapter.v0";
const INPUT_SCHEMA_VERSION: &str = "multica-cli-adapter-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "multica-cli-adapter-output.v0";
const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_ARG_CHARS: usize = 256;
const MAX_ARGS: usize = 32;
const DEFAULT_TIMEOUT_MS: u64 = 60_000;
const MIN_TIMEOUT_MS: u64 = 1_000;
const MAX_TIMEOUT_MS: u64 = 3_600_000;

/// Fixed read-only query args for observer wiring.
pub const QUERY_OPERATION_ARGS: &[&str] = &["issues", "list", "--json"];

/// Allowed authority-scoped Multica CLI operation names.
pub const ALLOWED_MULTICA_OPERATIONS: &[&str] = &[
    "safe_writer_done_reconcile",
    "safe_writer_issue_create",
    "safe_writer_issue_import",
    "queue_backlog_promote",
    "todo_runner_assign_rerun",
];

const MAX_ISSUE_ID_CHARS: usize = 64;
const MAX_TITLE_CHARS: usize = 256;
const MAX_IMPORT_REF_CHARS: usize = 256;
const MAX_UUID_CHARS: usize = 64;

/// Build shell-free argv for one allowed authority operation.
pub fn build_allowed_operation_argv(
    multica_operation: &str,
    params: &Value,
) -> Result<Vec<String>, String> {
    if !ALLOWED_MULTICA_OPERATIONS.contains(&multica_operation) {
        return Err(format!(
            "multica_operation must be one of {ALLOWED_MULTICA_OPERATIONS:?}"
        ));
    }
    match multica_operation {
        "safe_writer_done_reconcile" => {
            let issue_id = require_param_str(params, "issue_id")?;
            validate_issue_id(&issue_id)?;
            Ok(vec![
                "issue".into(),
                "update".into(),
                issue_id,
                "--status".into(),
                "done".into(),
                "--no-start".into(),
                "--output".into(),
                "json".into(),
            ])
        }
        "safe_writer_issue_create" => {
            let title = require_param_str(params, "title")?;
            validate_bounded_text("title", &title, MAX_TITLE_CHARS)?;
            Ok(vec![
                "issue".into(),
                "create".into(),
                "--title".into(),
                title,
                "--no-start".into(),
                "--output".into(),
                "json".into(),
            ])
        }
        "safe_writer_issue_import" => {
            let import_ref = require_param_str(params, "import_ref")?;
            validate_bounded_text("import_ref", &import_ref, MAX_IMPORT_REF_CHARS)?;
            Ok(vec![
                "issue".into(),
                "import".into(),
                "--input".into(),
                import_ref,
                "--no-start".into(),
                "--output".into(),
                "json".into(),
            ])
        }
        "queue_backlog_promote" => {
            let issue_id = require_param_str(params, "issue_id")?;
            validate_issue_id(&issue_id)?;
            Ok(vec![
                "issue".into(),
                "update".into(),
                issue_id,
                "--status".into(),
                "todo".into(),
                "--no-start".into(),
                "--output".into(),
                "json".into(),
            ])
        }
        "todo_runner_assign_rerun" => {
            let issue_id = require_param_str(params, "issue_id")?;
            let assignee_uuid = require_param_str(params, "assignee_uuid")?;
            validate_issue_id(&issue_id)?;
            validate_uuid(&assignee_uuid)?;
            Ok(vec![
                "issue".into(),
                "update".into(),
                issue_id,
                "--assignee".into(),
                assignee_uuid,
                "--rerun".into(),
                "--no-start".into(),
                "--output".into(),
                "json".into(),
            ])
        }
        _ => unreachable!(),
    }
}

fn require_param_str(params: &Value, field: &str) -> Result<String, String> {
    params
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{field} is required"))
}

fn validate_bounded_text(field: &str, value: &str, max: usize) -> Result<(), String> {
    if value.is_empty() || value.chars().count() > max {
        return Err(format!("{field} must be 1..={max} chars"));
    }
    Ok(())
}

fn validate_issue_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.chars().count() > MAX_ISSUE_ID_CHARS
        || value.contains(' ')
        || value.starts_with('-')
    {
        return Err(format!(
            "issue_id must be 1..={MAX_ISSUE_ID_CHARS} chars without spaces or leading dash"
        ));
    }
    Ok(())
}

fn validate_uuid(value: &str) -> Result<(), String> {
    if value.is_empty() || value.chars().count() > MAX_UUID_CHARS {
        return Err(format!("assignee_uuid must be 1..={MAX_UUID_CHARS} chars"));
    }
    let lower = value.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('-').collect();
    if parts.len() == 5
        && parts
            .iter()
            .all(|part| part.len() == 4 || part.len() == 8 || part.len() == 12)
        && parts
            .iter()
            .all(|part| part.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return Ok(());
    }
    Err("assignee_uuid must be a UUID".into())
}

#[cfg(windows)]
const WINDOWS_ENV_ALLOWLIST: &[&str] = &[
    "SystemRoot",
    "windir",
    "SystemDrive",
    "COMSPEC",
    "PATHEXT",
    "USERPROFILE",
    "LOCALAPPDATA",
    "APPDATA",
    "PROGRAMDATA",
];

/// Errors validating a pinned CLI executable path.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CliPathError {
    #[error("cli path must be absolute")]
    NotAbsolute,
    #[error("cli path does not exist")]
    NotFound,
    #[error("cli path is not a file")]
    NotFile,
}

/// Validated absolute native Multica CLI executable path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedCliPath {
    path: PathBuf,
}

impl PinnedCliPath {
    /// Validate an absolute native executable path.
    pub fn resolve(path: impl AsRef<Path>) -> Result<Self, CliPathError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(CliPathError::NotAbsolute);
        }
        let meta = std::fs::metadata(path).map_err(|_| CliPathError::NotFound)?;
        if !meta.is_file() {
            return Err(CliPathError::NotFile);
        }
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    /// Borrow the absolute path.
    pub fn as_path(&self) -> &Path {
        &self.path
    }
}

/// Fixed executable plus prefix arguments. No shell expansion.
#[derive(Debug, Clone)]
pub struct CliCommandSpec {
    pub program: PathBuf,
    pub prefix_args: Vec<OsString>,
}

impl CliCommandSpec {
    /// Build a spec from a validated pinned path and bounded fixed prefix args.
    pub fn from_pinned(pinned: &PinnedCliPath, prefix_args: &[String]) -> Result<Self, String> {
        if prefix_args.len() > MAX_ARGS {
            return Err(format!("prefix_args exceeds {MAX_ARGS} items"));
        }
        for arg in prefix_args {
            if arg.is_empty() || arg.chars().count() > MAX_ARG_CHARS {
                return Err(format!("each prefix arg must be 1..={MAX_ARG_CHARS} chars"));
            }
        }
        Ok(Self {
            program: pinned.path.clone(),
            prefix_args: prefix_args.iter().map(OsString::from).collect(),
        })
    }

    /// Build a shell-free `Command` with cleared environment and minimal OS baseline.
    pub fn build_command(&self, operation_args: &[String]) -> Result<Command, String> {
        if operation_args.len() > MAX_ARGS {
            return Err(format!("operation_args exceeds {MAX_ARGS} items"));
        }
        for arg in operation_args {
            if arg.is_empty() || arg.chars().count() > MAX_ARG_CHARS {
                return Err(format!(
                    "each operation arg must be 1..={MAX_ARG_CHARS} chars"
                ));
            }
        }
        let mut cmd = Command::new(&self.program);
        cmd.env_clear();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
            for key in WINDOWS_ENV_ALLOWLIST {
                if let Ok(val) = std::env::var(key) {
                    cmd.env(key, val);
                }
            }
        }
        #[cfg(unix)]
        {
            for key in ["PATH", "HOME", "LANG", "LC_ALL", "TZ"] {
                if let Ok(val) = std::env::var(key) {
                    cmd.env(key, val);
                }
            }
        }
        cmd.args(&self.prefix_args);
        cmd.args(operation_args);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(cmd)
    }
}

/// Result of one CLI invocation (internal; raw stdout is never emitted in compact output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliInvokeResult {
    pub exit_code: i32,
    pub stdout_json: Option<Value>,
    pub stdout_sha256: String,
    pub stdout_byte_count: usize,
    pub stdout_truncated: bool,
    pub stderr_byte_count: usize,
    pub timed_out: bool,
}

fn sha256_prefixed(bytes: &[u8]) -> String {
    format!("sha256:{}", encode(Sha256::digest(bytes)))
}

fn json_top_level_kind(value: Option<&Value>) -> &'static str {
    match value {
        Some(Value::Object(_)) => "object",
        Some(Value::Array(_)) => "array",
        Some(Value::Null) => "null",
        Some(Value::String(_)) => "string",
        Some(Value::Number(_)) => "number",
        Some(Value::Bool(_)) => "bool",
        None => "none",
    }
}

/// Process runner abstraction for synthetic contract tests.
pub trait ProcessRunner {
    fn run(
        &self,
        spec: &CliCommandSpec,
        operation_args: &[String],
        timeout_ms: u64,
    ) -> Result<CliInvokeResult, String>;
}

/// Default OS process runner.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsProcessRunner;

impl ProcessRunner for OsProcessRunner {
    fn run(
        &self,
        spec: &CliCommandSpec,
        operation_args: &[String],
        timeout_ms: u64,
    ) -> Result<CliInvokeResult, String> {
        let mut child = spec
            .build_command(operation_args)
            .map_err(|e| format!("build_command: {e}"))?
            .spawn()
            .map_err(|e| format!("spawn_failed: {e}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "stdout_unavailable".to_string())?;
        let stderr = child.stderr.take();
        let stdout_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut reader = stdout;
            let _ = reader.read_to_end(&mut buf);
            buf
        });
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let status = loop {
            match child.try_wait().map_err(|e| format!("wait_failed: {e}"))? {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_thread.join();
                    return Ok(CliInvokeResult {
                        exit_code: -1,
                        stdout_json: None,
                        stdout_sha256: sha256_prefixed(&[]),
                        stdout_byte_count: 0,
                        stdout_truncated: false,
                        stderr_byte_count: 0,
                        timed_out: true,
                    });
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        let stdout_buf = stdout_thread
            .join()
            .map_err(|_| "stdout_join_failed".to_string())?;
        let stderr_byte_count = if let Some(mut err) = stderr {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            buf.len()
        } else {
            0
        };
        parse_invoke_output(status, stdout_buf, stderr_byte_count, false)
    }
}

fn parse_invoke_output(
    status: ExitStatus,
    stdout_buf: Vec<u8>,
    stderr_byte_count: usize,
    timed_out: bool,
) -> Result<CliInvokeResult, String> {
    let exit_code = status.code().unwrap_or(-1);
    let stdout_byte_count = stdout_buf.len();
    let stdout_truncated = stdout_byte_count > MAX_STDOUT_BYTES;
    let bounded = if stdout_truncated {
        &stdout_buf[..MAX_STDOUT_BYTES]
    } else {
        &stdout_buf
    };
    let stdout_sha256 = sha256_prefixed(bounded);
    let stdout_json = if bounded.is_empty() {
        None
    } else {
        serde_json::from_slice(bounded).ok()
    };
    Ok(CliInvokeResult {
        exit_code,
        stdout_json,
        stdout_sha256,
        stdout_byte_count,
        stdout_truncated,
        stderr_byte_count,
        timed_out,
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdapterInput {
    schema_version: String,
    operation: String,
    cli_path: String,
    #[serde(default)]
    prefix_args: Vec<String>,
    #[serde(default)]
    invoke_args: Vec<String>,
    #[serde(default)]
    multica_operation: Option<String>,
    #[serde(default)]
    operation_params: Option<Value>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

fn issue(code: &str, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn resolve_timeout_ms(raw: Option<u64>) -> Result<u64, String> {
    let timeout_ms = raw.unwrap_or(DEFAULT_TIMEOUT_MS);
    if (MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&timeout_ms) {
        Ok(timeout_ms)
    } else {
        Err(format!(
            "timeout_ms must be {MIN_TIMEOUT_MS}..={MAX_TIMEOUT_MS}"
        ))
    }
}

fn compact(
    operation: &str,
    valid: bool,
    exit_reason: &str,
    issues: Vec<Value>,
    invoke: Option<&CliInvokeResult>,
) -> Value {
    let mut payload = json!({
        "schema_version": OUTPUT_SCHEMA_VERSION,
        "app_version": MULTICA_CLI_ADAPTER_VERSION,
        "operation": operation,
        "valid": valid,
        "exit_reason": exit_reason,
        "issue_count": issues.len(),
        "issues": issues,
    });
    if let Some(result) = invoke {
        let obj = payload.as_object_mut().expect("compact object");
        obj.insert("exit_code".into(), json!(result.exit_code));
        obj.insert("stdout_sha256".into(), json!(result.stdout_sha256));
        obj.insert("stdout_byte_count".into(), json!(result.stdout_byte_count));
        obj.insert("stdout_truncated".into(), json!(result.stdout_truncated));
        obj.insert("stderr_byte_count".into(), json!(result.stderr_byte_count));
        obj.insert("json_parse_ok".into(), json!(result.stdout_json.is_some()));
        obj.insert(
            "json_top_level_kind".into(),
            json!(json_top_level_kind(result.stdout_json.as_ref())),
        );
        obj.insert("timed_out".into(), json!(result.timed_out));
    }
    payload
}

/// Validate input and invoke the pinned CLI through the supplied runner.
pub fn run_multica_cli_adapter(value: &Value, runner: &dyn ProcessRunner) -> Value {
    let Ok(input) = serde_json::from_value::<AdapterInput>(value.clone()) else {
        return compact(
            "probe",
            false,
            "input_invalid",
            vec![issue(
                "input_invalid",
                "input must match multica-cli-adapter-input.v0",
            )],
            None,
        );
    };

    let mut issues = Vec::new();
    if input.schema_version != INPUT_SCHEMA_VERSION {
        issues.push(issue(
            "input_invalid",
            format!("schema_version must be {INPUT_SCHEMA_VERSION}"),
        ));
    }
    let operation = input.operation.as_str();
    if operation != "probe" && operation != "invoke" && operation != "query" {
        issues.push(issue(
            "unknown_operation",
            "operation must be probe, query, or invoke",
        ));
    }
    let timeout_ms = match resolve_timeout_ms(input.timeout_ms) {
        Ok(ms) => ms,
        Err(message) => {
            issues.push(issue("timeout_ms_invalid", message));
            DEFAULT_TIMEOUT_MS
        }
    };
    if !issues.is_empty() {
        return compact(operation, false, "input_invalid", issues, None);
    }

    let pinned = match PinnedCliPath::resolve(&input.cli_path) {
        Ok(path) => path,
        Err(CliPathError::NotAbsolute) => {
            return compact(
                operation,
                false,
                "cli_path_not_absolute",
                vec![issue("cli_path_not_absolute", "cli_path must be absolute")],
                None,
            );
        }
        Err(CliPathError::NotFound) => {
            return compact(
                operation,
                false,
                "cli_path_not_found",
                vec![issue("cli_path_not_found", "cli_path does not exist")],
                None,
            );
        }
        Err(CliPathError::NotFile) => {
            return compact(
                operation,
                false,
                "cli_path_not_file",
                vec![issue("cli_path_not_file", "cli_path is not a file")],
                None,
            );
        }
    };

    let spec = match CliCommandSpec::from_pinned(&pinned, &input.prefix_args) {
        Ok(spec) => spec,
        Err(message) => {
            return compact(
                operation,
                false,
                "prefix_args_invalid",
                vec![issue("prefix_args_invalid", message)],
                None,
            );
        }
    };

    let invoke_args = match operation {
        "probe" => vec!["--help".to_string()],
        "query" => QUERY_OPERATION_ARGS
            .iter()
            .map(|arg| (*arg).to_string())
            .collect(),
        "invoke" => {
            if let Some(multica_operation) = input.multica_operation.as_deref() {
                if !input.invoke_args.is_empty() {
                    return compact(
                        operation,
                        false,
                        "invoke_args_forbidden",
                        vec![issue(
                            "invoke_args_forbidden",
                            "invoke with multica_operation must not include invoke_args",
                        )],
                        None,
                    );
                }
                match build_allowed_operation_argv(
                    multica_operation,
                    input.operation_params.as_ref().unwrap_or(&json!({})),
                ) {
                    Ok(args) => args,
                    Err(message) => {
                        return compact(
                            operation,
                            false,
                            "allowed_operation_invalid",
                            vec![issue("allowed_operation_invalid", message)],
                            None,
                        );
                    }
                }
            } else if input.invoke_args.is_empty() {
                return compact(
                    operation,
                    false,
                    "invoke_args_missing",
                    vec![issue(
                        "invoke_args_missing",
                        "invoke requires invoke_args or multica_operation",
                    )],
                    None,
                );
            } else {
                return compact(
                    operation,
                    false,
                    "arbitrary_argv_rejected",
                    vec![issue(
                        "arbitrary_argv_rejected",
                        "authority paths require multica_operation; arbitrary invoke_args rejected",
                    )],
                    None,
                );
            }
        }
        _ => unreachable!(),
    };

    match runner.run(&spec, &invoke_args, timeout_ms) {
        Ok(result) => {
            let exit_reason = if result.timed_out {
                "process_timeout"
            } else if result.exit_code == 0 {
                match operation {
                    "probe" => "probe_ok",
                    "query" => "query_ok",
                    _ => "invoke_ok",
                }
            } else if result.stdout_truncated {
                "stdout_truncated"
            } else if result.stdout_json.is_none() && operation != "probe" {
                "stdout_not_json"
            } else {
                "cli_nonzero_exit"
            };
            let valid = matches!(exit_reason, "probe_ok" | "query_ok" | "invoke_ok");
            compact(operation, valid, exit_reason, Vec::new(), Some(&result))
        }
        Err(message) => compact(
            operation,
            false,
            "spawn_failed",
            vec![issue("spawn_failed", message)],
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct FakeRunner {
        exit_code: i32,
        stdout: Vec<u8>,
        stderr_len: usize,
        timed_out: bool,
    }

    impl ProcessRunner for FakeRunner {
        fn run(
            &self,
            _spec: &CliCommandSpec,
            _operation_args: &[String],
            _timeout_ms: u64,
        ) -> Result<CliInvokeResult, String> {
            let bounded = if self.stdout.len() > MAX_STDOUT_BYTES {
                &self.stdout[..MAX_STDOUT_BYTES]
            } else {
                &self.stdout
            };
            Ok(CliInvokeResult {
                exit_code: self.exit_code,
                stdout_json: serde_json::from_slice(bounded).ok(),
                stdout_sha256: sha256_prefixed(bounded),
                stdout_byte_count: self.stdout.len(),
                stdout_truncated: self.stdout.len() > MAX_STDOUT_BYTES,
                stderr_byte_count: self.stderr_len,
                timed_out: self.timed_out,
            })
        }
    }

    fn base_input(operation: &str) -> Value {
        json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": operation,
            "cli_path": "C:/fake/multica.exe",
            "prefix_args": [],
            "invoke_args": []
        })
    }

    #[test]
    fn rejects_relative_cli_path() {
        let mut input = base_input("probe");
        input["cli_path"] = json!("relative/multica.exe");
        let output = run_multica_cli_adapter(&input, &OsProcessRunner);
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["exit_reason"], json!("cli_path_not_absolute"));
    }

    #[test]
    fn probe_redacts_stdout_to_hash() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("multica.exe");
        fs::write(&file, b"x").unwrap();
        let mut input = base_input("probe");
        input["cli_path"] = json!(file.canonicalize().unwrap().to_string_lossy());
        let runner = FakeRunner {
            exit_code: 0,
            stdout: br#"{"ok":true,"secret":"token"}"#.to_vec(),
            stderr_len: 0,
            timed_out: false,
        };
        let output = run_multica_cli_adapter(&input, &runner);
        assert_eq!(output["valid"], json!(true));
        assert_eq!(output["exit_reason"], json!("probe_ok"));
        assert!(output.get("stdout_json").is_none());
        assert!(output["stdout_sha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(output["stdout_byte_count"], json!(28));
        assert_eq!(output["json_parse_ok"], json!(true));
        assert_eq!(output["json_top_level_kind"], json!("object"));
    }

    #[test]
    fn query_uses_fixed_read_only_args() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("multica.exe");
        fs::write(&file, b"x").unwrap();
        let mut input = base_input("query");
        input["cli_path"] = json!(file.canonicalize().unwrap().to_string_lossy());

        struct ArgCapturingRunner;
        impl ProcessRunner for ArgCapturingRunner {
            fn run(
                &self,
                _spec: &CliCommandSpec,
                operation_args: &[String],
                _timeout_ms: u64,
            ) -> Result<CliInvokeResult, String> {
                assert_eq!(
                    operation_args,
                    &[
                        "issues".to_string(),
                        "list".to_string(),
                        "--json".to_string()
                    ]
                );
                Ok(CliInvokeResult {
                    exit_code: 0,
                    stdout_json: Some(json!({"issues": []})),
                    stdout_sha256: sha256_prefixed(b"{}"),
                    stdout_byte_count: 2,
                    stdout_truncated: false,
                    stderr_byte_count: 0,
                    timed_out: false,
                })
            }
        }

        let output = run_multica_cli_adapter(&input, &ArgCapturingRunner);
        assert_eq!(output["exit_reason"], json!("query_ok"));
    }

    #[test]
    fn invoke_requires_multica_operation() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("multica.exe");
        fs::write(&file, b"x").unwrap();
        let mut input = base_input("invoke");
        input["cli_path"] = json!(file.canonicalize().unwrap().to_string_lossy());
        let output = run_multica_cli_adapter(
            &input,
            &FakeRunner {
                exit_code: 0,
                stdout: br#"{}"#.to_vec(),
                stderr_len: 0,
                timed_out: false,
            },
        );
        assert_eq!(output["exit_reason"], json!("invoke_args_missing"));
    }

    #[test]
    fn rejects_arbitrary_invoke_args() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("multica.exe");
        fs::write(&file, b"x").unwrap();
        let mut input = base_input("invoke");
        input["cli_path"] = json!(file.canonicalize().unwrap().to_string_lossy());
        input["invoke_args"] = json!(["issues", "list", "--json"]);
        let output = run_multica_cli_adapter(
            &input,
            &FakeRunner {
                exit_code: 0,
                stdout: br#"{}"#.to_vec(),
                stderr_len: 0,
                timed_out: false,
            },
        );
        assert_eq!(output["exit_reason"], json!("arbitrary_argv_rejected"));
    }

    #[test]
    fn allowed_operation_builds_fixed_argv() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("multica.exe");
        fs::write(&file, b"x").unwrap();
        let mut input = base_input("invoke");
        input["cli_path"] = json!(file.canonicalize().unwrap().to_string_lossy());
        input["multica_operation"] = json!("queue_backlog_promote");
        input["operation_params"] = json!({"issue_id": "AM-1"});

        struct ArgCapturingRunner;
        impl ProcessRunner for ArgCapturingRunner {
            fn run(
                &self,
                _spec: &CliCommandSpec,
                operation_args: &[String],
                _timeout_ms: u64,
            ) -> Result<CliInvokeResult, String> {
                assert_eq!(
                    operation_args,
                    &[
                        "issue".to_string(),
                        "update".to_string(),
                        "AM-1".to_string(),
                        "--status".to_string(),
                        "todo".to_string(),
                        "--no-start".to_string(),
                        "--output".to_string(),
                        "json".to_string(),
                    ]
                );
                Ok(CliInvokeResult {
                    exit_code: 0,
                    stdout_json: Some(json!({"ok": true})),
                    stdout_sha256: sha256_prefixed(b"{}"),
                    stdout_byte_count: 2,
                    stdout_truncated: false,
                    stderr_byte_count: 0,
                    timed_out: false,
                })
            }
        }

        let output = run_multica_cli_adapter(&input, &ArgCapturingRunner);
        assert_eq!(output["exit_reason"], json!("invoke_ok"));
    }

    #[test]
    fn synthetic_contract_records_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("multica.exe");
        fs::write(&file, b"x").unwrap();
        let mut input = base_input("invoke");
        input["cli_path"] = json!(file.canonicalize().unwrap().to_string_lossy());
        input["multica_operation"] = json!("safe_writer_done_reconcile");
        input["operation_params"] = json!({"issue_id": "AM-1"});
        let output = run_multica_cli_adapter(
            &input,
            &FakeRunner {
                exit_code: 10,
                stdout: br#"{"error":"auth"}"#.to_vec(),
                stderr_len: 12,
                timed_out: false,
            },
        );
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["exit_reason"], json!("cli_nonzero_exit"));
        assert_eq!(output["exit_code"], json!(10));
        assert!(output.get("stdout_json").is_none());
    }

    #[test]
    fn process_timeout_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("multica.exe");
        fs::write(&file, b"x").unwrap();
        let mut input = base_input("query");
        input["cli_path"] = json!(file.canonicalize().unwrap().to_string_lossy());
        let output = run_multica_cli_adapter(
            &input,
            &FakeRunner {
                exit_code: -1,
                stdout: vec![],
                stderr_len: 0,
                timed_out: true,
            },
        );
        assert_eq!(output["exit_reason"], json!("process_timeout"));
        assert_eq!(output["timed_out"], json!(true));
    }

    #[test]
    fn rejects_invalid_timeout_ms() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("multica.exe");
        fs::write(&file, b"x").unwrap();
        let mut input = base_input("probe");
        input["cli_path"] = json!(file.canonicalize().unwrap().to_string_lossy());
        input["timeout_ms"] = json!(50);
        let output = run_multica_cli_adapter(&input, &OsProcessRunner);
        assert_eq!(output["exit_reason"], json!("input_invalid"));
    }

    #[test]
    fn pinned_path_rejects_missing_file() {
        let err = PinnedCliPath::resolve("C:/nonexistent/multica-cli-000000.exe").unwrap_err();
        assert_eq!(err, CliPathError::NotFound);
    }
}
