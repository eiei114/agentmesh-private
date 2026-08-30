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
#[cfg(unix)]
use std::process::Child;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Plugin/schema version exposed in compact output.
pub const MULTICA_CLI_ADAPTER_VERSION: &str = "multica-cli-adapter.v0";
const INPUT_SCHEMA_VERSION: &str = "multica-cli-adapter-input.v0";
const OUTPUT_SCHEMA_VERSION: &str = "multica-cli-adapter-output.v0";
const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;
#[cfg(unix)]
const CAPTURE_DRAIN_GRACE_MS: u64 = 2_000;
#[cfg(windows)]
const WINDOWS_JOB_DRAIN_GRACE_MS: u64 = 5_000;
const CHILD_REAP_GRACE_MS: u64 = 2_000;
const MAX_ARG_CHARS: usize = 256;
const MAX_ARGS: usize = 32;
/// Default subprocess timeout, leaving host-lifecycle cleanup headroom.
pub const DEFAULT_CLI_TIMEOUT_MS: u64 = 60_000;
/// Minimum supported subprocess timeout.
pub const MIN_CLI_TIMEOUT_MS: u64 = 1_000;
/// Maximum subprocess timeout. App host limit is 120s, reserving 50s for
/// process-tree cleanup, ledger writes, and host finalization.
pub const MAX_CLI_TIMEOUT_MS: u64 = 70_000;

/// Fixed read-only query args for observer wiring.
pub const QUERY_OPERATION_ARGS: &[&str] = &["issue", "list", "--output", "json"];

/// Allowed authority-scoped Multica CLI operation names.
pub const ALLOWED_MULTICA_OPERATIONS: &[&str] = &[
    "safe_writer_done_reconcile",
    "safe_writer_issue_create",
    "safe_writer_issue_import",
    "queue_backlog_promote",
    "todo_runner_assign",
    "todo_runner_rerun",
    "cursor_recovery_rerun",
];

const MAX_ISSUE_ID_CHARS: usize = 64;
const MAX_TITLE_CHARS: usize = 256;
const MAX_UUID_CHARS: usize = 64;

/// Validate a relative import path stays inside an allowlisted root directory.
pub fn validate_import_description_file(
    import_root: &Path,
    description_file: &str,
) -> Result<String, String> {
    if description_file.is_empty() {
        return Err("description_file is required".into());
    }
    if Path::new(description_file).is_absolute() {
        return Err("description_file must be relative".into());
    }
    if description_file.contains('\\') {
        return Err("description_file must use forward slashes".into());
    }
    let rel = Path::new(description_file);
    for component in rel.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err("description_file must not contain ..".into());
        }
    }
    let root = import_root
        .canonicalize()
        .map_err(|_| "import_root does not exist".to_string())?;
    let joined = root.join(rel);
    let canonical = joined
        .canonicalize()
        .map_err(|_| "description_file does not exist under import_root".to_string())?;
    if !canonical.starts_with(&root) {
        return Err("description_file escapes import_root".into());
    }
    Ok(description_file.to_string())
}

/// Shell-free argv plus optional process working directory for one allowed operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedOperationSpec {
    pub argv: Vec<String>,
    pub working_directory: Option<PathBuf>,
}

/// Build shell-free argv for one allowed authority operation.
pub fn build_allowed_operation_argv(
    multica_operation: &str,
    params: &Value,
) -> Result<Vec<String>, String> {
    build_allowed_operation_spec(multica_operation, params).map(|spec| spec.argv)
}

/// Build argv and optional working directory for one allowed authority operation.
pub fn build_allowed_operation_spec(
    multica_operation: &str,
    params: &Value,
) -> Result<AllowedOperationSpec, String> {
    if !ALLOWED_MULTICA_OPERATIONS.contains(&multica_operation) {
        return Err(format!(
            "multica_operation must be one of {ALLOWED_MULTICA_OPERATIONS:?}"
        ));
    }
    match multica_operation {
        "safe_writer_done_reconcile" => {
            let issue_id = require_param_str(params, "issue_id")?;
            validate_issue_id(&issue_id)?;
            Ok(AllowedOperationSpec {
                argv: vec![
                    "issue".into(),
                    "update".into(),
                    issue_id,
                    "--status".into(),
                    "done".into(),
                    "--no-start".into(),
                    "--output".into(),
                    "json".into(),
                ],
                working_directory: None,
            })
        }
        "safe_writer_issue_create" => {
            let title = require_param_str(params, "title")?;
            validate_bounded_text("title", &title, MAX_TITLE_CHARS)?;
            Ok(AllowedOperationSpec {
                argv: vec![
                    "issue".into(),
                    "create".into(),
                    "--title".into(),
                    title,
                    "--no-start".into(),
                    "--output".into(),
                    "json".into(),
                ],
                working_directory: None,
            })
        }
        "safe_writer_issue_import" => {
            let title = require_param_str(params, "title")?;
            let description_file = require_param_str(params, "description_file")?;
            let project_id = require_param_str(params, "project_id")?;
            let import_root = require_param_str(params, "import_root")?;
            validate_bounded_text("title", &title, MAX_TITLE_CHARS)?;
            validate_bounded_text("project_id", &project_id, MAX_ISSUE_ID_CHARS)?;
            let rel = validate_import_description_file(Path::new(&import_root), &description_file)?;
            let canonical_root = Path::new(&import_root)
                .canonicalize()
                .map_err(|_| "import_root does not exist".to_string())?;
            Ok(AllowedOperationSpec {
                argv: vec![
                    "issue".into(),
                    "create".into(),
                    "--title".into(),
                    title,
                    "--description-file".into(),
                    rel,
                    "--project".into(),
                    project_id,
                    "--status".into(),
                    "todo".into(),
                    "--output".into(),
                    "json".into(),
                ],
                working_directory: Some(canonical_root),
            })
        }
        "queue_backlog_promote" => {
            let issue_id = require_param_str(params, "issue_id")?;
            validate_issue_id(&issue_id)?;
            Ok(AllowedOperationSpec {
                argv: vec![
                    "issue".into(),
                    "update".into(),
                    issue_id,
                    "--status".into(),
                    "todo".into(),
                    "--no-start".into(),
                    "--output".into(),
                    "json".into(),
                ],
                working_directory: None,
            })
        }
        "todo_runner_assign" => {
            let issue_id = require_param_str(params, "issue_id")?;
            let assignee_uuid = require_param_str(params, "assignee_uuid")?;
            validate_issue_id(&issue_id)?;
            validate_uuid(&assignee_uuid)?;
            Ok(AllowedOperationSpec {
                argv: vec![
                    "issue".into(),
                    "assign".into(),
                    issue_id,
                    "--to-id".into(),
                    assignee_uuid,
                    "--output".into(),
                    "json".into(),
                ],
                working_directory: None,
            })
        }
        "todo_runner_rerun" | "cursor_recovery_rerun" => {
            let issue_id = require_param_str(params, "issue_id")?;
            validate_issue_id(&issue_id)?;
            Ok(AllowedOperationSpec {
                argv: vec![
                    "issue".into(),
                    "rerun".into(),
                    issue_id,
                    "--output".into(),
                    "json".into(),
                ],
                working_directory: None,
            })
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
    pub working_directory: Option<PathBuf>,
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
            working_directory: None,
        })
    }

    /// Build a shell-free `Command` with cleared environment and minimal OS baseline.
    pub fn build_command(&self, operation_args: &[String]) -> Result<Command, String> {
        validate_operation_args(operation_args)?;
        let mut cmd = Command::new(&self.program);
        cmd.env_clear();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
            for key in WINDOWS_ENV_ALLOWLIST {
                if let Ok(val) = std::env::var(key) {
                    cmd.env(key, val);
                }
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
            for key in ["PATH", "HOME", "LANG", "LC_ALL", "TZ"] {
                if let Ok(val) = std::env::var(key) {
                    cmd.env(key, val);
                }
            }
        }
        cmd.args(&self.prefix_args);
        cmd.args(operation_args);
        if let Some(cwd) = &self.working_directory {
            cmd.current_dir(cwd);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(cmd)
    }

    #[cfg(windows)]
    fn build_windows_command(
        &self,
        operation_args: &[String],
    ) -> Result<windows_spawn::Command, String> {
        validate_operation_args(operation_args)?;
        let mut cmd = windows_spawn::Command::new(&self.program);
        cmd.env_clear();
        for key in WINDOWS_ENV_ALLOWLIST {
            if let Ok(value) = std::env::var(key) {
                cmd.env(key, value);
            }
        }
        cmd.args(&self.prefix_args);
        cmd.args(operation_args);
        if let Some(cwd) = &self.working_directory {
            cmd.current_dir(cwd);
        }
        cmd.stdin(windows_spawn::Stdio::null())
            .stdout(windows_spawn::Stdio::piped())
            .stderr(windows_spawn::Stdio::piped());
        Ok(cmd)
    }
}

fn validate_operation_args(operation_args: &[String]) -> Result<(), String> {
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
    Ok(())
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

#[derive(Debug)]
struct BoundedCapture {
    prefix: Vec<u8>,
    byte_count: usize,
    truncated: bool,
}

fn drain_bounded(mut reader: impl Read, retain_limit: usize) -> std::io::Result<BoundedCapture> {
    let mut prefix = Vec::with_capacity(retain_limit.min(64 * 1024));
    let mut byte_count = 0usize;
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        byte_count = byte_count.saturating_add(read);
        let retain = retain_limit.saturating_sub(prefix.len()).min(read);
        prefix.extend_from_slice(&chunk[..retain]);
    }
    Ok(BoundedCapture {
        prefix,
        byte_count,
        truncated: byte_count > retain_limit,
    })
}

fn join_capture(
    handle: std::thread::JoinHandle<std::io::Result<BoundedCapture>>,
    label: &str,
) -> Result<BoundedCapture, String> {
    handle
        .join()
        .map_err(|_| format!("{label}_join_failed"))?
        .map_err(|e| format!("{label}_read_failed: {e}"))
}

#[cfg(windows)]
fn join_captures_after_job_close(
    stdout: std::thread::JoinHandle<std::io::Result<BoundedCapture>>,
    stderr: std::thread::JoinHandle<std::io::Result<BoundedCapture>>,
) -> Result<(BoundedCapture, BoundedCapture), String> {
    let deadline = Instant::now() + Duration::from_millis(WINDOWS_JOB_DRAIN_GRACE_MS);
    while !(stdout.is_finished() && stderr.is_finished()) {
        if Instant::now() >= deadline {
            return Err("capture_drain_timeout: windows_job_closed".to_string());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok((
        join_capture(stdout, "stdout")?,
        join_capture(stderr, "stderr")?,
    ))
}

#[cfg(windows)]
fn wait_windows_child_bounded(
    child: &mut windows_spawn::Child,
    timeout_ms: u64,
) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match child.try_wait().map_err(|e| format!("wait_failed: {e}"))? {
            Some(status) => return Ok(status),
            None if Instant::now() >= deadline => {
                return Err("child_reap_timeout".to_string());
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(unix)]
fn join_captures_bounded(
    stdout: std::thread::JoinHandle<std::io::Result<BoundedCapture>>,
    stderr: std::thread::JoinHandle<std::io::Result<BoundedCapture>>,
    root_pid: u32,
) -> Result<(BoundedCapture, BoundedCapture), String> {
    let deadline = Instant::now() + Duration::from_millis(CAPTURE_DRAIN_GRACE_MS);
    while !(stdout.is_finished() && stderr.is_finished()) {
        if Instant::now() >= deadline {
            let cleanup = terminate_descendants(root_pid);
            let cleanup_deadline = Instant::now() + Duration::from_millis(CAPTURE_DRAIN_GRACE_MS);
            while !(stdout.is_finished() && stderr.is_finished()) {
                if Instant::now() >= cleanup_deadline {
                    return Err(format!(
                        "capture_drain_timeout: process_tree_cleanup={}",
                        cleanup
                            .err()
                            .unwrap_or_else(|| "completed_but_pipes_open".to_string())
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok((
        join_capture(stdout, "stdout")?,
        join_capture(stderr, "stderr")?,
    ))
}

#[cfg(unix)]
fn wait_child_bounded(child: &mut Child, timeout_ms: u64) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match child.try_wait().map_err(|e| format!("wait_failed: {e}"))? {
            Some(status) => return Ok(status),
            None if Instant::now() >= deadline => {
                return Err("child_reap_timeout".to_string());
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(unix)]
fn terminate_process_tree(root_pid: u32) -> Result<(), String> {
    use nix::errno::Errno;
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    let process_group = i32::try_from(root_pid)
        .map(Pid::from_raw)
        .map_err(|_| "process_tree_kill_invalid_pid".to_string())?;
    match killpg(process_group, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(format!("process_tree_kill_failed: {error}")),
    }
}

#[cfg(unix)]
fn terminate_descendants(root_pid: u32) -> Result<(), String> {
    terminate_process_tree(root_pid)
}

#[cfg(unix)]
fn cleanup_timed_out_child(
    child: &mut Child,
    root_pid: u32,
) -> (Option<ExitStatus>, Option<String>) {
    let tree_error = terminate_process_tree(root_pid).err();
    let status = match child.try_wait() {
        Ok(Some(status)) => Some(status),
        Ok(None) => {
            // A cleanup command can lose a race with a root process exiting. Always
            // try the direct handle before the bounded reap so that this path never
            // falls back to an unbounded Child::wait.
            let direct_kill_error = child.kill().err().map(|error| error.to_string());
            match wait_child_bounded(child, CHILD_REAP_GRACE_MS) {
                Ok(status) => Some(status),
                Err(error) => {
                    return (
                        None,
                        Some(format!(
                            "timeout_child_reap_failed: {error}; tree={}; direct_kill={}",
                            tree_error.as_deref().unwrap_or("ok"),
                            direct_kill_error.unwrap_or_else(|| "none".to_string())
                        )),
                    );
                }
            }
        }
        Err(error) => {
            return (None, Some(format!("timeout_try_wait_failed: {error}")));
        }
    };

    if let Some(tree_error) = tree_error {
        if let Err(descendant_error) = terminate_descendants(root_pid) {
            return (
                status,
                Some(format!(
                    "timeout_tree_cleanup_failed: tree={tree_error}; descendants={descendant_error}"
                )),
            );
        }
    }
    (status, None)
}

fn captured_invoke_result(
    exit_code: i32,
    stdout: BoundedCapture,
    stderr_byte_count: usize,
    timed_out: bool,
) -> CliInvokeResult {
    let stdout_json = if timed_out || stdout.prefix.is_empty() {
        None
    } else {
        serde_json::from_slice(&stdout.prefix).ok()
    };
    CliInvokeResult {
        exit_code,
        stdout_json,
        stdout_sha256: sha256_prefixed(&stdout.prefix),
        stdout_byte_count: stdout.byte_count,
        stdout_truncated: stdout.truncated,
        stderr_byte_count,
        timed_out,
    }
}

#[cfg(unix)]
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
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "stderr_unavailable".to_string())?;
        let stdout_thread = std::thread::spawn(move || drain_bounded(stdout, MAX_STDOUT_BYTES));
        let stderr_thread = std::thread::spawn(move || drain_bounded(stderr, 0));
        let root_pid = child.id();
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut timed_out = false;
        let mut cleanup_error = None;
        let status = loop {
            match child.try_wait().map_err(|e| format!("wait_failed: {e}"))? {
                Some(status) => break Some(status),
                None if Instant::now() >= deadline => {
                    timed_out = true;
                    let (status, error) = cleanup_timed_out_child(&mut child, root_pid);
                    cleanup_error = error;
                    break status;
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        let (stdout, stderr) = join_captures_bounded(stdout_thread, stderr_thread, root_pid)?;
        if let Some(error) = cleanup_error {
            return Err(error);
        }
        Ok(captured_invoke_result(
            if timed_out {
                -1
            } else {
                status.and_then(|value| value.code()).unwrap_or(-1)
            },
            stdout,
            stderr.byte_count,
            timed_out,
        ))
    }
}

#[cfg(windows)]
impl ProcessRunner for OsProcessRunner {
    fn run(
        &self,
        spec: &CliCommandSpec,
        operation_args: &[String],
        timeout_ms: u64,
    ) -> Result<CliInvokeResult, String> {
        use windows_spawn::{CreationFlags, DropPolicy, SpawnOptions};

        let mut command = spec
            .build_windows_command(operation_args)
            .map_err(|e| format!("build_command: {e}"))?;
        let options = SpawnOptions::new()
            .creation_flags(CreationFlags::NO_WINDOW | CreationFlags::NEW_PROCESS_GROUP)
            .drop_policy(DropPolicy::KillTree);
        let mut child = command
            .spawn_with(options)
            .map_err(|e| format!("spawn_failed: {e}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "stdout_unavailable".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "stderr_unavailable".to_string())?;
        let stdout_thread = std::thread::spawn(move || drain_bounded(stdout, MAX_STDOUT_BYTES));
        let stderr_thread = std::thread::spawn(move || drain_bounded(stderr, 0));
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut timed_out = false;
        let mut cleanup_error = None;
        let status = loop {
            match child.try_wait().map_err(|e| format!("wait_failed: {e}"))? {
                Some(status) => break Some(status),
                None if Instant::now() >= deadline => {
                    timed_out = true;
                    let direct_kill_error = child.kill().err().map(|error| error.to_string());
                    match wait_windows_child_bounded(&mut child, CHILD_REAP_GRACE_MS) {
                        Ok(status) => break Some(status),
                        Err(error) => {
                            cleanup_error = Some(format!(
                                "timeout_child_reap_failed: {error}; direct_kill={}",
                                direct_kill_error.unwrap_or_else(|| "none".to_string())
                            ));
                            break None;
                        }
                    }
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        };

        // KillTree is attached during the suspended CreateProcessW transaction.
        // Dropping the child closes that Job before capture handles are joined,
        // terminating every descendant without PID discovery or shell helpers.
        drop(child);
        let (stdout, stderr) = join_captures_after_job_close(stdout_thread, stderr_thread)?;
        if let Some(error) = cleanup_error {
            return Err(error);
        }
        Ok(captured_invoke_result(
            if timed_out {
                -1
            } else {
                status.and_then(|value| value.code()).unwrap_or(-1)
            },
            stdout,
            stderr.byte_count,
            timed_out,
        ))
    }
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

/// Resolve and validate a CLI subprocess timeout shared by composed Apps.
pub fn resolve_cli_timeout_ms(raw: Option<u64>) -> Result<u64, String> {
    let timeout_ms = raw.unwrap_or(DEFAULT_CLI_TIMEOUT_MS);
    if (MIN_CLI_TIMEOUT_MS..=MAX_CLI_TIMEOUT_MS).contains(&timeout_ms) {
        Ok(timeout_ms)
    } else {
        Err(format!(
            "timeout_ms must be {MIN_CLI_TIMEOUT_MS}..={MAX_CLI_TIMEOUT_MS}"
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
    let timeout_ms = match resolve_cli_timeout_ms(input.timeout_ms) {
        Ok(ms) => ms,
        Err(message) => {
            issues.push(issue("timeout_ms_invalid", message));
            DEFAULT_CLI_TIMEOUT_MS
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

    let mut spec = match CliCommandSpec::from_pinned(&pinned, &input.prefix_args) {
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
                match build_allowed_operation_spec(
                    multica_operation,
                    input.operation_params.as_ref().unwrap_or(&json!({})),
                ) {
                    Ok(op) => {
                        spec.working_directory = op.working_directory;
                        op.argv
                    }
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
            } else if result.stdout_truncated {
                "stdout_truncated"
            } else if result.exit_code != 0 {
                "cli_nonzero_exit"
            } else if result.stdout_json.is_none() && operation != "probe" {
                "stdout_not_json"
            } else {
                match operation {
                    "probe" => "probe_ok",
                    "query" => "query_ok",
                    _ => "invoke_ok",
                }
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
    use std::io::Write as _;

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
                        "issue".to_string(),
                        "list".to_string(),
                        "--output".to_string(),
                        "json".to_string()
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
    fn query_rejects_exit_zero_non_json_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("multica.exe");
        fs::write(&file, b"x").unwrap();
        let mut input = base_input("query");
        input["cli_path"] = json!(file.canonicalize().unwrap().to_string_lossy());
        let output = run_multica_cli_adapter(
            &input,
            &FakeRunner {
                exit_code: 0,
                stdout: b"not-json".to_vec(),
                stderr_len: 0,
                timed_out: false,
            },
        );
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["exit_reason"], json!("stdout_not_json"));
    }

    #[test]
    fn query_rejects_truncated_stdout_before_success() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("multica.exe");
        fs::write(&file, b"x").unwrap();
        let mut input = base_input("query");
        input["cli_path"] = json!(file.canonicalize().unwrap().to_string_lossy());
        let output = run_multica_cli_adapter(
            &input,
            &FakeRunner {
                exit_code: 0,
                stdout: vec![b' '; MAX_STDOUT_BYTES + 1],
                stderr_len: 0,
                timed_out: false,
            },
        );
        assert_eq!(output["valid"], json!(false));
        assert_eq!(output["exit_reason"], json!("stdout_truncated"));
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

    fn process_fixture_spec(name: &str) -> CliCommandSpec {
        let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
        let pinned = PinnedCliPath::resolve(executable).unwrap();
        let prefix_args = vec![
            format!("tests::{name}"),
            "--exact".to_string(),
            "--ignored".to_string(),
            "--nocapture".to_string(),
        ];
        CliCommandSpec::from_pinned(&pinned, &prefix_args).unwrap()
    }

    fn run_process_fixture(name: &str) -> CliInvokeResult {
        OsProcessRunner
            .run(&process_fixture_spec(name), &[], 20_000)
            .unwrap()
    }

    fn spawn_process_fixture(name: &str) {
        let executable = std::env::current_exe().unwrap();
        let child = Command::new(executable)
            .args([
                format!("tests::{name}"),
                "--exact".to_string(),
                "--ignored".to_string(),
                "--nocapture".to_string(),
            ])
            .spawn()
            .unwrap();
        drop(child);
    }

    fn descendant_heartbeat_path(suffix: &str) -> PathBuf {
        std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .join(format!("agentmesh-adapter-descendant-heartbeat-{suffix}"))
    }

    #[test]
    fn os_runner_bounds_large_stdout_without_timeout() {
        let output = run_process_fixture("process_fixture_large_stdout");
        assert_eq!(output.exit_code, 0);
        assert!(!output.timed_out);
        assert!(output.stdout_truncated);
        assert!(output.stdout_byte_count > MAX_STDOUT_BYTES);
    }

    #[test]
    fn os_runner_drains_large_stderr_concurrently() {
        let output = run_process_fixture("process_fixture_large_stderr");
        assert_eq!(output.exit_code, 0);
        assert!(!output.timed_out);
        assert!(output.stderr_byte_count >= 512 * 1024);
    }

    #[test]
    fn os_runner_timeout_terminates_process_tree() {
        let started = Instant::now();
        let output = OsProcessRunner
            .run(
                &process_fixture_spec("process_fixture_timeout_tree"),
                &[],
                1_000,
            )
            .unwrap();
        assert!(output.timed_out);
        assert_eq!(output.exit_code, -1);
        assert!(started.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn os_runner_bounds_post_exit_inherited_pipe_wait() {
        let heartbeat = descendant_heartbeat_path("post-exit");
        let _ = fs::remove_file(&heartbeat);
        let started = Instant::now();
        let result = OsProcessRunner.run(
            &process_fixture_spec("process_fixture_descendant_holds_pipes"),
            &[],
            10_000,
        );
        let result_summary = format!("{result:?}");
        assert!(
            result.is_ok()
                || result
                    .as_ref()
                    .unwrap_err()
                    .starts_with("capture_drain_timeout")
        );
        assert!(started.elapsed() < Duration::from_secs(8));
        let first_len = fs::metadata(&heartbeat).ok().map(|meta| meta.len());
        std::thread::sleep(Duration::from_millis(500));
        let second_len = fs::metadata(&heartbeat).ok().map(|meta| meta.len());
        assert_eq!(
            first_len, second_len,
            "descendant heartbeat still running; runner={result_summary}"
        );
        let _ = fs::remove_file(heartbeat);
    }

    #[test]
    fn os_runner_bounds_timeout_boundary_parent_exit_race() {
        let heartbeat = descendant_heartbeat_path("timeout-boundary");
        let _ = fs::remove_file(&heartbeat);
        let started = Instant::now();
        let result = OsProcessRunner.run(
            &process_fixture_spec("process_fixture_timeout_boundary_descendant"),
            &[],
            1_000,
        );
        let result_summary = format!("{result:?}");
        assert!(
            result.is_ok(),
            "timeout boundary cleanup failed: {result_summary}"
        );
        assert!(started.elapsed() < Duration::from_secs(8));
        let first_len = fs::metadata(&heartbeat).ok().map(|meta| meta.len());
        assert!(
            first_len.is_some(),
            "boundary descendant never started; runner={result_summary}"
        );
        std::thread::sleep(Duration::from_millis(500));
        let second_len = fs::metadata(&heartbeat).ok().map(|meta| meta.len());
        assert_eq!(
            first_len, second_len,
            "boundary descendant heartbeat still running; runner={result_summary}"
        );
        let _ = fs::remove_file(heartbeat);
    }

    #[test]
    #[ignore = "subprocess fixture invoked by os_runner_bounds_large_stdout_without_timeout"]
    fn process_fixture_large_stdout() {
        let bytes = vec![b'x'; MAX_STDOUT_BYTES + 64 * 1024];
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(&bytes).unwrap();
        stdout.flush().unwrap();
    }

    #[test]
    #[ignore = "subprocess fixture invoked by os_runner_drains_large_stderr_concurrently"]
    fn process_fixture_large_stderr() {
        let bytes = vec![b'e'; 512 * 1024];
        let mut stderr = std::io::stderr().lock();
        stderr.write_all(&bytes).unwrap();
        stderr.flush().unwrap();
    }

    #[test]
    #[ignore = "subprocess fixture invoked by os_runner_timeout_terminates_process_tree"]
    fn process_fixture_timeout_tree() {
        spawn_process_fixture("process_fixture_sleep_long");
        std::thread::sleep(Duration::from_secs(30));
    }

    #[test]
    #[ignore = "subprocess fixture invoked by os_runner_bounds_post_exit_inherited_pipe_wait"]
    fn process_fixture_descendant_holds_pipes() {
        spawn_process_fixture("process_fixture_heartbeat_post_exit");
    }

    #[test]
    #[ignore = "subprocess fixture invoked by os_runner_bounds_timeout_boundary_parent_exit_race"]
    fn process_fixture_timeout_boundary_descendant() {
        spawn_process_fixture("process_fixture_heartbeat_timeout_boundary");
        std::thread::sleep(Duration::from_millis(950));
    }

    #[test]
    #[ignore = "descendant process fixture"]
    fn process_fixture_sleep_long() {
        std::thread::sleep(Duration::from_secs(30));
    }

    #[test]
    #[ignore = "descendant process fixture"]
    fn process_fixture_heartbeat_post_exit() {
        run_heartbeat_fixture("post-exit");
    }

    #[test]
    #[ignore = "descendant process fixture"]
    fn process_fixture_heartbeat_timeout_boundary() {
        run_heartbeat_fixture("timeout-boundary");
    }

    fn run_heartbeat_fixture(suffix: &str) {
        let heartbeat = descendant_heartbeat_path(suffix);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(heartbeat)
            .unwrap();
        for _ in 0..300 {
            file.write_all(b".").unwrap();
            file.flush().unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
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
    fn todo_runner_assign_builds_fixed_argv() {
        let args = build_allowed_operation_argv(
            "todo_runner_assign",
            &json!({"issue_id": "AM-2", "assignee_uuid": "550e8400-e29b-41d4-a716-446655440000"}),
        )
        .unwrap();
        assert_eq!(
            args,
            vec![
                "issue",
                "assign",
                "AM-2",
                "--to-id",
                "550e8400-e29b-41d4-a716-446655440000",
                "--output",
                "json"
            ]
        );
    }

    #[test]
    fn todo_runner_rerun_has_no_no_start() {
        let args = build_allowed_operation_argv("todo_runner_rerun", &json!({"issue_id": "AM-3"}))
            .unwrap();
        assert_eq!(args, vec!["issue", "rerun", "AM-3", "--output", "json"]);
        assert!(!args.contains(&"--no-start".to_string()));
    }

    #[test]
    fn import_rejects_path_escape() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("imports");
        std::fs::create_dir_all(&root).unwrap();
        let err = validate_import_description_file(&root, "../outside.md").unwrap_err();
        assert!(err.contains(".."));
    }

    #[test]
    fn import_accepts_canonical_relative_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("imports");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("issue.md"), b"# issue").unwrap();
        let rel = validate_import_description_file(&root, "issue.md").unwrap();
        assert_eq!(rel, "issue.md");
    }

    #[test]
    fn issue_import_builds_create_argv() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("imports");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("issue.md"), b"# issue").unwrap();
        let spec = build_allowed_operation_spec(
            "safe_writer_issue_import",
            &json!({
                "title": "Imported",
                "description_file": "issue.md",
                "project_id": "agentmesh-private",
                "import_root": root.to_string_lossy(),
            }),
        )
        .unwrap();
        assert_eq!(
            spec.argv,
            vec![
                "issue",
                "create",
                "--title",
                "Imported",
                "--description-file",
                "issue.md",
                "--project",
                "agentmesh-private",
                "--status",
                "todo",
                "--output",
                "json"
            ]
        );
        let cwd = spec.working_directory.as_ref().unwrap();
        let desc_arg = spec
            .argv
            .iter()
            .skip_while(|arg| arg.as_str() != "--description-file")
            .nth(1)
            .expect("--description-file value");
        let resolved = cwd.join(desc_arg).canonicalize().unwrap();
        assert_eq!(resolved, root.join("issue.md").canonicalize().unwrap());
    }

    #[test]
    fn issue_import_invoke_sets_cwd_and_resolves_description_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("multica.exe");
        fs::write(&file, b"x").unwrap();
        let root = dir.path().join("imports");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("issue.md"), b"# issue").unwrap();

        struct ImportCwdRunner {
            expected_root: PathBuf,
        }

        impl ProcessRunner for ImportCwdRunner {
            fn run(
                &self,
                spec: &CliCommandSpec,
                operation_args: &[String],
                _timeout_ms: u64,
            ) -> Result<CliInvokeResult, String> {
                let cwd = spec
                    .working_directory
                    .as_ref()
                    .expect("working_directory required for issue import");
                assert_eq!(cwd, &self.expected_root);
                let rel = operation_args
                    .iter()
                    .skip_while(|arg| arg.as_str() != "--description-file")
                    .nth(1)
                    .expect("--description-file value");
                let resolved = cwd.join(rel).canonicalize().unwrap();
                assert_eq!(resolved, self.expected_root.join("issue.md"));
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

        let canonical_root = root.canonicalize().unwrap();
        let input = json!({
            "schema_version": INPUT_SCHEMA_VERSION,
            "operation": "invoke",
            "cli_path": file.canonicalize().unwrap().to_string_lossy(),
            "multica_operation": "safe_writer_issue_import",
            "operation_params": {
                "title": "Imported",
                "description_file": "issue.md",
                "project_id": "agentmesh-private",
                "import_root": root.to_string_lossy(),
            },
        });
        let output = run_multica_cli_adapter(
            &input,
            &ImportCwdRunner {
                expected_root: canonical_root,
            },
        );
        assert_eq!(output["valid"], json!(true));
        assert_eq!(output["exit_reason"], json!("invoke_ok"));
    }

    #[test]
    fn pinned_path_rejects_missing_file() {
        let missing = std::env::temp_dir().join(format!(
            "agentmesh-missing-multica-cli-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);
        let err = PinnedCliPath::resolve(missing.to_str().unwrap()).unwrap_err();
        assert_eq!(err, CliPathError::NotFound);
    }
}
