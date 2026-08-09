use crate::{normalize_candidate_path, EvidenceError};
use serde::Deserialize;
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

/// Candidate discovery stream retained in packet provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CandidateStream {
    /// `qmd search` lexical/BM25 output.
    Keyword,
    /// `qmd query` semantic/hybrid output.
    Semantic,
    /// `qmd-adaptive-search search --read-only` output.
    Adaptive,
}

impl CandidateStream {
    /// Stable machine name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keyword => "qmd_keyword",
            Self::Semantic => "qmd_semantic",
            Self::Adaptive => "qmd_adaptive",
        }
    }
}

/// One normalized discovery hit.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateHit {
    /// Vault-relative path.
    pub path: String,
    /// Zero-based rank inside the originating stream.
    pub rank: usize,
    /// Originating stream.
    pub stream: CandidateStream,
    /// Bounded, non-query-derived ranking explanations.
    pub reasons: Vec<String>,
}

/// Result and diagnostics for one stream.
#[derive(Debug, Clone)]
pub struct StreamResult {
    /// Stream identity.
    pub stream: CandidateStream,
    /// Parsed path-only hits.
    pub hits: Vec<CandidateHit>,
    /// Stable error code when the stream failed.
    pub error: Option<String>,
    /// Stream wall time.
    pub duration_ms: u128,
}

/// Executable plus fixed prefix arguments. No shell expansion is used.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    /// Native executable.
    pub program: PathBuf,
    /// Fixed arguments inserted before evidence-search arguments.
    pub prefix_args: Vec<OsString>,
}

impl CommandSpec {
    /// Resolve an executable, including safe Node resolution for npm `.cmd` shims.
    pub fn resolve(program: impl AsRef<OsStr>) -> Result<Self, EvidenceError> {
        let requested = PathBuf::from(program.as_ref());
        let resolved = if requested.components().count() > 1 || requested.is_absolute() {
            requested
        } else {
            which::which(&requested).map_err(|_| {
                EvidenceError::CommandUnavailable(requested.to_string_lossy().into_owned())
            })?
        };
        if resolved.extension().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("js")
                || ext.eq_ignore_ascii_case("mjs")
                || ext.eq_ignore_ascii_case("cjs")
        }) {
            let node = which::which("node")
                .map_err(|_| EvidenceError::CommandUnavailable("node".into()))?;
            return Ok(Self {
                program: node,
                prefix_args: vec![resolved.into_os_string()],
            });
        }
        if resolved
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
        {
            return resolve_node_shim(&resolved);
        }
        Ok(Self {
            program: resolved,
            prefix_args: Vec::new(),
        })
    }
}

fn resolve_node_shim(path: &Path) -> Result<CommandSpec, EvidenceError> {
    let metadata = fs::metadata(path).map_err(|source| EvidenceError::Io {
        context: format!("inspect command shim {}", path.display()),
        source,
    })?;
    if metadata.len() > 64 * 1024 {
        return Err(EvidenceError::CommandProtocol(
            "command shim exceeds 64 KiB".into(),
        ));
    }
    let text = fs::read_to_string(path).map_err(|source| EvidenceError::Io {
        context: format!("read command shim {}", path.display()),
        source,
    })?;
    let script = text
        .lines()
        .filter(|line| line.to_ascii_lowercase().contains("node"))
        .flat_map(quoted_values)
        .find(|value| {
            let lower = value.to_ascii_lowercase();
            lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs")
        })
        .ok_or_else(|| {
            EvidenceError::CommandProtocol(format!(
                "cannot safely resolve npm shim {}",
                path.display()
            ))
        })?;
    let node =
        which::which("node").map_err(|_| EvidenceError::CommandUnavailable("node".into()))?;
    Ok(CommandSpec {
        program: node,
        prefix_args: vec![OsString::from(script)],
    })
}

fn quoted_values(line: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('"') else { break };
        values.push(after[..end].to_owned());
        rest = &after[end + 1..];
    }
    values
}

/// Run all configured QMD streams concurrently under one shared deadline.
pub struct DiscoveryOptions<'a> {
    /// Canonical vault root.
    pub root: &'a Path,
    /// Per-stream result cap.
    pub limit: usize,
    /// QMD collection name.
    pub collection: &'a str,
    /// Direct QMD command.
    pub qmd: Option<&'a CommandSpec>,
    /// Adaptive command.
    pub adaptive: Option<&'a CommandSpec>,
    /// Whether to run `qmd query` in addition to `qmd search`.
    pub include_semantic: bool,
    /// Whether to run adaptive read-only discovery.
    pub include_adaptive: bool,
    /// Shared absolute deadline.
    pub deadline: Instant,
}

/// Run all configured QMD streams concurrently under one shared deadline.
pub fn discover_all(query: &str, options: DiscoveryOptions<'_>) -> Vec<StreamResult> {
    let DiscoveryOptions {
        root,
        limit,
        collection,
        qmd,
        adaptive,
        include_semantic,
        include_adaptive,
        deadline,
    } = options;
    let adaptive_preflight = if include_adaptive {
        adaptive.map(|command| verify_adaptive_capability(root, command, deadline))
    } else {
        None
    };
    thread::scope(|scope| {
        let mut handles = Vec::new();
        let mut immediate = Vec::new();
        if let Some(command) = qmd {
            let operations = if include_semantic {
                vec![
                    (CandidateStream::Keyword, "search"),
                    (CandidateStream::Semantic, "query"),
                ]
            } else {
                vec![(CandidateStream::Keyword, "search")]
            };
            for (stream, operation) in operations {
                let args = vec![
                    operation.to_owned(),
                    query.to_owned(),
                    "-n".into(),
                    limit.to_string(),
                    "-c".into(),
                    collection.to_owned(),
                    "--json".into(),
                ];
                let command = command.clone();
                handles
                    .push(scope.spawn(move || run_stream(root, stream, &command, &args, deadline)));
            }
        }
        if include_adaptive {
            if let Some(Ok(())) = adaptive_preflight.as_ref() {
                if let Some(command) = adaptive {
                    let args = vec![
                        "search".into(),
                        query.to_owned(),
                        "--mode".into(),
                        "precision".into(),
                        "--max".into(),
                        limit.to_string(),
                        "--read-only".into(),
                    ];
                    let command = command.clone();
                    handles.push(scope.spawn(move || {
                        run_stream(root, CandidateStream::Adaptive, &command, &args, deadline)
                    }));
                }
            } else if let Some(Err(error)) = adaptive_preflight.as_ref() {
                immediate.push(StreamResult {
                    stream: CandidateStream::Adaptive,
                    hits: Vec::new(),
                    error: Some(error.stable_code().to_owned()),
                    duration_ms: 0,
                });
            }
        }
        immediate.extend(
            handles
                .into_iter()
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| StreamResult {
                        stream: CandidateStream::Adaptive,
                        hits: Vec::new(),
                        error: Some("stream_thread_panicked".into()),
                        duration_ms: 0,
                    })
                })
                .collect::<Vec<_>>(),
        );
        immediate
    })
}

fn verify_adaptive_capability(
    root: &Path,
    command: &CommandSpec,
    deadline: Instant,
) -> Result<(), EvidenceError> {
    let output = run_bounded(root, command, &["--help".into()], deadline)?;
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| EvidenceError::CommandProtocol("adaptive help is not UTF-8".into()))?;
    let version = text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("qmd-adaptive-search "))
        .ok_or_else(|| {
            EvidenceError::CommandProtocol("adaptive capability/version header missing".into())
        })?;
    let mut parts = version
        .trim()
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok());
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    if (major, minor) < (1, 3) {
        return Err(EvidenceError::CommandProtocol(format!(
            "adaptive read-only capability requires >=1.3.0; found {version}"
        )));
    }
    Ok(())
}

fn run_stream(
    root: &Path,
    stream: CandidateStream,
    command: &CommandSpec,
    args: &[String],
    deadline: Instant,
) -> StreamResult {
    let started = Instant::now();
    let outcome = run_bounded(root, command, args, deadline)
        .and_then(|output| parse_output(stream, &output.stdout));
    match outcome {
        Ok(hits) => StreamResult {
            stream,
            hits,
            error: None,
            duration_ms: started.elapsed().as_millis(),
        },
        Err(error) => StreamResult {
            stream,
            hits: Vec::new(),
            error: Some(error.stable_code().to_owned()),
            duration_ms: started.elapsed().as_millis(),
        },
    }
}

struct Output {
    stdout: Vec<u8>,
}

fn run_bounded(
    root: &Path,
    spec: &CommandSpec,
    args: &[String],
    deadline: Instant,
) -> Result<Output, EvidenceError> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.prefix_args)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0000_0200); // CREATE_NEW_PROCESS_GROUP
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|source| EvidenceError::Io {
        context: format!("spawn {}", spec.program.display()),
        source,
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| EvidenceError::CommandProtocol("stdout pipe missing".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| EvidenceError::CommandProtocol("stderr pipe missing".into()))?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout));
    let stderr_reader = thread::spawn(move || read_bounded(stderr));
    let wait_result = wait_until(&mut child, deadline);
    let stdout = stdout_reader
        .join()
        .map_err(|_| EvidenceError::CommandProtocol("stdout reader panicked".into()))??;
    let _stderr = stderr_reader
        .join()
        .map_err(|_| EvidenceError::CommandProtocol("stderr reader panicked".into()))??;
    let status = wait_result?;
    if !status.success() {
        return Err(EvidenceError::CommandProtocol(format!(
            "command exited with {}",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(Output { stdout })
}

fn wait_until(child: &mut Child, deadline: Instant) -> Result<ExitStatus, EvidenceError> {
    loop {
        if let Some(status) = child.try_wait().map_err(|source| EvidenceError::Io {
            context: "poll evidence discovery command".into(),
            source,
        })? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            kill_tree(child);
            let _ = child.wait();
            return Err(EvidenceError::CommandTimeout);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

fn read_bounded(mut reader: impl Read) -> Result<Vec<u8>, EvidenceError> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut exceeded = false;
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|source| EvidenceError::Io {
                context: "read discovery command output".into(),
                source,
            })?;
        if count == 0 {
            break;
        }
        if output.len() < OUTPUT_LIMIT {
            let remaining = OUTPUT_LIMIT - output.len();
            output.extend_from_slice(&buffer[..count.min(remaining)]);
        }
        if output.len() == OUTPUT_LIMIT && count > 0 {
            exceeded = true;
        }
    }
    if exceeded {
        Err(EvidenceError::CommandProtocol(
            "command output exceeds 4 MiB".into(),
        ))
    } else {
        Ok(output)
    }
}

#[derive(Debug, Deserialize)]
struct QmdRow {
    file: String,
}

fn parse_output(stream: CandidateStream, bytes: &[u8]) -> Result<Vec<CandidateHit>, EvidenceError> {
    match stream {
        CandidateStream::Keyword | CandidateStream::Semantic => {
            let rows: Vec<QmdRow> = serde_json::from_slice(bytes).map_err(|error| {
                EvidenceError::CommandProtocol(format!("invalid qmd JSON: {error}"))
            })?;
            Ok(rows
                .into_iter()
                .enumerate()
                .filter_map(|(rank, row)| {
                    let path = normalize_candidate_path(&row.file);
                    (path.len() <= 1024).then(|| CandidateHit {
                        path,
                        rank,
                        stream,
                        reasons: Vec::new(),
                    })
                })
                .collect())
        }
        CandidateStream::Adaptive => parse_adaptive(bytes),
    }
}

fn parse_adaptive(bytes: &[u8]) -> Result<Vec<CandidateHit>, EvidenceError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| {
        EvidenceError::CommandProtocol(format!("invalid adaptive JSON: {error}"))
    })?;
    if value.get("readOnly").and_then(Value::as_bool) != Some(true) {
        return Err(EvidenceError::CommandProtocol(
            "adaptive command did not confirm readOnly=true".into(),
        ));
    }
    let rows = value
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| EvidenceError::CommandProtocol("adaptive results array missing".into()))?;
    rows.iter()
        .enumerate()
        .map(|(rank, row)| {
            let path = row.get("path").and_then(Value::as_str).ok_or_else(|| {
                EvidenceError::CommandProtocol("adaptive result path missing".into())
            })?;
            let path = normalize_candidate_path(path);
            if path.len() > 1024 {
                return Err(EvidenceError::CommandProtocol(
                    "adaptive result path exceeds 1024 bytes".into(),
                ));
            }
            let reasons = row
                .get("why")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .take(5)
                .map(|reason| reason.chars().take(160).collect())
                .collect();
            Ok(CandidateHit {
                path,
                rank,
                stream: CandidateStream::Adaptive,
                reasons,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_parser_requires_read_only_confirmation() {
        let error = parse_output(CandidateStream::Adaptive, br#"{"results":[]}"#).unwrap_err();
        assert!(matches!(error, EvidenceError::CommandProtocol(_)));
    }

    #[test]
    fn parses_all_qmd_rows_without_score_authority() {
        let hits = parse_output(
            CandidateStream::Keyword,
            br#"[{"file":"qmd://vault/4-Project/A.md"},{"file":"B.md"}]"#,
        )
        .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "4_Project/A.md");
        assert_eq!(hits[1].rank, 1);
    }

    #[test]
    fn read_bounded_accepts_small_output() {
        assert_eq!(read_bounded(std::io::Cursor::new(b"ok")).unwrap(), b"ok");
    }

    #[test]
    fn command_deadline_kills_long_running_process() {
        #[cfg(windows)]
        let spec = CommandSpec {
            program: PathBuf::from("cmd.exe"),
            prefix_args: vec!["/C".into(), "ping -n 6 127.0.0.1 >NUL".into()],
        };
        #[cfg(unix)]
        let spec = CommandSpec {
            program: PathBuf::from("/bin/sh"),
            prefix_args: vec!["-c".into(), "sleep 5".into()],
        };
        let started = Instant::now();
        let result = run_bounded(
            Path::new("."),
            &spec,
            &[],
            Instant::now() + Duration::from_millis(100),
        );
        assert!(matches!(result, Err(EvidenceError::CommandTimeout)));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
