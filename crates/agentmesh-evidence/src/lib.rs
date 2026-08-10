//! Read-only, source-linked Decision and AgentRun evidence compilation.
//!
//! The public seam is intentionally small: load one reviewed contract, validate
//! one request, fuse path-only candidates from every QMD stream, optionally
//! traverse one serving-valid JSON graph, then reread canonical Markdown.

mod contract;
mod decision;
mod discovery;
mod graph;

pub use contract::{load_contract, load_evaluation, Contract, EvaluationFixture, NamespacePolicy};
pub use decision::{
    frontmatter_text, normalize_decision_metadata, normalize_decision_metadata_for_graph,
    parse_frontmatter, DecisionMetadata, DecisionMetadataError, DecisionScope, DecisionStatus,
};
pub use discovery::{
    discover_all, CandidateHit, CandidateStream, CommandSpec, DiscoveryOptions, StreamResult,
};
pub use graph::{load_graph, validate_graph, Graph};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use thiserror::Error;

use serde_yaml::Value as YamlValue;

const EXCERPT_LIMIT: usize = 8 * 1024;

/// Compiler failure with a stable machine category.
#[derive(Debug, Error)]
pub enum EvidenceError {
    /// Canonical contract is missing or malformed.
    #[error("invalid contract: {0}")]
    InvalidContract(String),
    /// Caller request violates the reviewed boundary.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// A source or artifact path escapes the vault root.
    #[error("path rejected: {0}")]
    PathRejected(String),
    /// Graph cannot be served safely.
    #[error("invalid graph: {0}")]
    InvalidGraph(String),
    /// A Decision source has malformed lifecycle metadata.
    #[error("invalid decision record: {0}")]
    InvalidDecisionRecord(String),
    /// Required discovery executable is unavailable.
    #[error("command unavailable: {0}")]
    CommandUnavailable(String),
    /// Discovery process exceeded the shared deadline.
    #[error("command timed out")]
    CommandTimeout,
    /// Discovery process violated its JSON/size/read-only protocol.
    #[error("command protocol error: {0}")]
    CommandProtocol(String),
    /// Bounded local I/O failed.
    #[error("{context}: {source}")]
    Io {
        /// Operation being attempted.
        context: String,
        /// OS failure.
        #[source]
        source: std::io::Error,
    },
}

impl EvidenceError {
    /// Stable machine-readable error code.
    pub const fn stable_code(&self) -> &'static str {
        match self {
            Self::InvalidContract(_) => "invalid_contract",
            Self::InvalidRequest(_) => "invalid_request",
            Self::PathRejected(_) => "path_rejected",
            Self::InvalidGraph(_) => "invalid_graph",
            Self::InvalidDecisionRecord(_) => "invalid_decision_record",
            Self::CommandUnavailable(_) => "qmd_unavailable",
            Self::CommandTimeout => "qmd_timeout",
            Self::CommandProtocol(_) => "qmd_protocol_error",
            Self::Io { .. } => "io_error",
        }
    }
}

/// Supported evidence packet kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceKind {
    /// Architectural or operational decision evidence.
    Decision,
    /// Agent execution/run evidence.
    AgentRun,
}

impl EvidenceKind {
    /// Parse the reviewed CLI spelling.
    pub fn parse(value: &str) -> Result<Self, EvidenceError> {
        match value {
            "Decision" | "decision" => Ok(Self::Decision),
            "AgentRun" | "agent-run" | "agent_run" => Ok(Self::AgentRun),
            _ => Err(EvidenceError::InvalidRequest(format!(
                "unsupported kind {value}"
            ))),
        }
    }

    const fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Decision => &["decision", "record_status", "rationale", "alternatives"],
            Self::AgentRun => &["source_issue", "run_id", "artifact", "outcome"],
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "Decision",
            Self::AgentRun => "AgentRun",
        }
    }
}

/// Information sensitivity ordered from least to most restrictive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sensitivity {
    Public,
    Internal,
    Private,
    Restricted,
}

impl Sensitivity {
    /// Parse contract spelling.
    pub fn parse(value: &str) -> Result<Self, EvidenceError> {
        match value {
            "public" => Ok(Self::Public),
            "internal" => Ok(Self::Internal),
            "private" => Ok(Self::Private),
            "restricted" => Ok(Self::Restricted),
            _ => Err(EvidenceError::InvalidContract(format!(
                "unknown sensitivity {value}"
            ))),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Private => "private",
            Self::Restricted => "restricted",
        }
    }
}

/// Retrieval mode exposed by the AgentMesh CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceMode {
    /// Historical direct `qmd search` baseline.
    DirectQmd,
    /// Fuse every QMD stream without graph traversal.
    QmdOnly,
    /// Fuse every QMD stream, then perform bounded explicit graph traversal.
    Hybrid,
    /// Explicit graph traversal seeded by lexical node-title/path matches.
    GraphOnly,
}

impl EvidenceMode {
    /// Parse CLI spelling.
    pub fn parse(value: &str) -> Result<Self, EvidenceError> {
        match value {
            "direct-qmd" | "direct_qmd" => Ok(Self::DirectQmd),
            "qmd-only" | "qmd_only" => Ok(Self::QmdOnly),
            "hybrid" => Ok(Self::Hybrid),
            "graph-only" | "graph_only" => Ok(Self::GraphOnly),
            _ => Err(EvidenceError::InvalidRequest(format!(
                "unsupported mode {value}"
            ))),
        }
    }
}

/// In-memory request. Query text is never serialized into the packet.
#[derive(Debug, Clone)]
pub struct EvidenceRequest {
    /// Packet kind.
    pub kind: EvidenceKind,
    /// Ephemeral natural-language query.
    pub query: String,
    /// Reviewed namespace.
    pub namespace: String,
    /// Maximum allowed source sensitivity.
    pub sensitivity_ceiling: String,
    /// Maximum emitted source count.
    pub max_sources: usize,
    /// Shared hard deadline.
    pub timeout_ms: u64,
    /// Decision lifecycle scope. Ignored for AgentRun packets.
    pub decision_scope: DecisionScope,
    /// QMD-only or hybrid.
    pub mode: EvidenceMode,
}

/// Runtime discovery and graph inputs.
#[derive(Debug, Clone)]
pub struct CompileOptions {
    /// QMD collection name.
    pub collection: String,
    /// Direct `qmd` command.
    pub qmd: Option<CommandSpec>,
    /// Adaptive command. It must confirm `readOnly=true`.
    pub adaptive: Option<CommandSpec>,
    /// Optional serving-valid v2 graph.
    pub graph_path: Option<PathBuf>,
}

/// Validate request bounds before any subprocess starts.
pub fn validate_request(
    request: &EvidenceRequest,
    contract: &Contract,
) -> Result<(), EvidenceError> {
    if request.query.trim().is_empty() || request.query.len() > contract.max_query_bytes {
        return Err(EvidenceError::InvalidRequest(format!(
            "query must be non-empty and at most {} UTF-8 bytes",
            contract.max_query_bytes
        )));
    }
    if !contract.namespaces.contains_key(&request.namespace) {
        return Err(EvidenceError::InvalidRequest(format!(
            "unknown namespace {}",
            request.namespace
        )));
    }
    let ceiling = Sensitivity::parse(&request.sensitivity_ceiling)
        .map_err(|_| EvidenceError::InvalidRequest("invalid sensitivity ceiling".into()))?;
    if ceiling == Sensitivity::Restricted {
        return Err(EvidenceError::InvalidRequest(
            "restricted ceiling cannot be served".into(),
        ));
    }
    if request.max_sources == 0 || request.max_sources > contract.max_sources {
        return Err(EvidenceError::InvalidRequest(
            "max_sources exceeds contract".into(),
        ));
    }
    if request.timeout_ms < 1_000 || request.timeout_ms > contract.hard_timeout_ms {
        return Err(EvidenceError::InvalidRequest(
            "timeout exceeds contract".into(),
        ));
    }
    if request.kind == EvidenceKind::Decision
        && !contract
            .decision_scopes
            .contains_key(request.decision_scope.as_str())
    {
        return Err(EvidenceError::InvalidRequest(format!(
            "unknown decision scope {}",
            request.decision_scope.as_str()
        )));
    }
    Ok(())
}

/// Compile one ephemeral Evidence Packet.
pub fn compile(
    root: &Path,
    contract: &Contract,
    request: &EvidenceRequest,
    options: &CompileOptions,
) -> Result<Value, EvidenceError> {
    validate_request(request, contract)?;
    let root = root.canonicalize().map_err(|source| EvidenceError::Io {
        context: format!("resolve vault root {}", root.display()),
        source,
    })?;
    let started = Instant::now();
    let deadline = started + Duration::from_millis(request.timeout_ms);
    let discovery_budget_ms = request.timeout_ms.saturating_sub(1_000).min(25_000);
    let discovery_deadline = started + Duration::from_millis(discovery_budget_ms);
    let candidate_limit = (request.max_sources * 10).clamp(20, 60);
    let stream_results = discover_all(
        &request.query,
        DiscoveryOptions {
            root: &root,
            limit: candidate_limit,
            collection: &options.collection,
            qmd: options.qmd.as_ref(),
            adaptive: options.adaptive.as_ref(),
            include_semantic: !matches!(
                request.mode,
                EvidenceMode::DirectQmd | EvidenceMode::GraphOnly
            ),
            include_adaptive: matches!(request.mode, EvidenceMode::QmdOnly | EvidenceMode::Hybrid),
            deadline: discovery_deadline,
        },
    );
    let policy = contract.namespaces.get(&request.namespace).ok_or_else(|| {
        EvidenceError::InvalidRequest("namespace disappeared after validation".into())
    })?;
    let (ranked_candidates, mut rejected) =
        fuse_candidates(&stream_results, policy, contract, request.kind);
    let ranked_candidates = if request.kind == EvidenceKind::Decision {
        filter_decision_candidates(&root, ranked_candidates, request, contract, &mut rejected)?
    } else {
        ranked_candidates
    };
    let candidate_trace: Vec<Value> = ranked_candidates.iter().take(30).map(|candidate| json!({
        "source_path": candidate.path,
        "rrf_score": candidate.score,
        "streams": candidate.streams.iter().map(|(stream, rank)| json!({"stream": stream.as_str(), "rank": rank})).collect::<Vec<_>>(),
        "reasons": candidate.reasons,
    })).collect();
    let mut selected: Vec<String> = ranked_candidates
        .into_iter()
        .map(|candidate| candidate.path)
        .collect();
    let mut manifest_used = false;
    if selected.is_empty() && request.mode != EvidenceMode::GraphOnly {
        selected = manifest_scan(&root, request, policy, contract, candidate_limit);
        manifest_used = !selected.is_empty();
    }

    let mut graph_status = "not_configured".to_owned();
    let mut expansion_limited = false;
    let mut relation_edges = Vec::new();
    let graph = if matches!(request.mode, EvidenceMode::Hybrid | EvidenceMode::GraphOnly) {
        if let Some(path) = &options.graph_path {
            match graph::load_graph(&root, path, &contract.graph_schema) {
                Ok(graph) => {
                    let allowed_decision_statuses = contract
                        .decision_scopes
                        .get(request.decision_scope.as_str());
                    if request.mode == EvidenceMode::GraphOnly {
                        selected = graph_seed_paths(&graph, request, allowed_decision_statuses);
                    }
                    let lexical_graph_paths =
                        graph_seed_paths(&graph, request, allowed_decision_statuses);
                    let expansion = graph::expand_with_statuses(
                        &graph,
                        &selected,
                        request,
                        contract.max_graph_hops,
                        contract.max_visited_nodes,
                        allowed_decision_statuses,
                    )?;
                    if request.mode == EvidenceMode::Hybrid {
                        selected.truncate(request.max_sources);
                        let graph_paths = expansion
                            .paths
                            .iter()
                            .cloned()
                            .chain(lexical_graph_paths)
                            .filter(|path| path != "index.md");
                        selected =
                            blend_hybrid_candidates(selected, graph_paths, request.max_sources, 4);
                    } else {
                        for path in &expansion.paths {
                            if !selected.contains(path) {
                                selected.push(path.clone());
                            }
                        }
                    }
                    relation_edges = expansion.edges;
                    expansion_limited = expansion.limited;
                    graph_status = "ready".into();
                    Some(graph)
                }
                Err(error) => {
                    graph_status = graph_error_code(&error).into();
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    selected.retain(|path| path_allowed(path, policy, contract).is_ok());
    selected.dedup();

    let graph_sensitivity: BTreeMap<&str, &str> =
        graph.as_ref().map_or_else(BTreeMap::new, |value| {
            value
                .nodes
                .iter()
                .map(|node| (node.source_path.as_str(), node.sensitivity.as_str()))
                .collect()
        });
    let ceiling = Sensitivity::parse(&request.sensitivity_ceiling)
        .map_err(|_| EvidenceError::InvalidRequest("invalid sensitivity ceiling".into()))?;
    let mut evidence = Vec::new();
    let mut fields: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut source_rejections = Vec::new();
    for path in selected {
        if evidence.len() >= request.max_sources {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        let sensitivity = graph_sensitivity
            .get(path.as_str())
            .map_or(Ok(policy.default_sensitivity), |value| {
                Sensitivity::parse(value)
            })?;
        if sensitivity > ceiling || sensitivity == Sensitivity::Restricted {
            continue;
        }
        let graph_node = graph
            .as_ref()
            .and_then(|value| value.nodes.iter().find(|node| node.source_path == path));
        match read_evidence(
            &root,
            &path,
            request.kind,
            sensitivity,
            contract.max_source_bytes,
            graph_node.map(|node| node.node_type.as_str()),
        ) {
            Ok((item, extracted)) => {
                if request.kind == EvidenceKind::Decision {
                    let status = item
                        .get("record_status")
                        .and_then(Value::as_str)
                        .and_then(|value| DecisionStatus::parse(value).ok());
                    let allowed = status.is_some_and(|status| {
                        contract
                            .decision_scopes
                            .get(request.decision_scope.as_str())
                            .map_or_else(
                                || request.decision_scope.allows(status),
                                |statuses| statuses.contains(&status),
                            )
                    });
                    if !allowed {
                        source_rejections.push("decision_scope_filtered".to_owned());
                        continue;
                    }
                }
                if let Some(expected) = graph_node.map(|node| node.source_hash.as_str()) {
                    if item["source_hash"].as_str() != Some(expected) {
                        source_rejections.push("source_changed".to_owned());
                        continue;
                    }
                }
                let evidence_id = item
                    .get("evidence_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                for (field, value) in extracted {
                    fields
                        .entry(field)
                        .or_default()
                        .push((value, evidence_id.clone()));
                }
                evidence.push(item);
            }
            Err(error) => source_rejections.push(error.stable_code().to_owned()),
        }
    }

    let claims: Vec<Value> = request.kind.fields().iter().map(|field| {
        let values = fields.get(*field).cloned().unwrap_or_default();
        let normalized: BTreeSet<String> = values.iter().map(|(value, _)| normalize_claim(value)).collect();
        let support = if values.is_empty() { "unresolved" } else if normalized.len() > 1 { "conflicting" } else { "supported" };
        let claim = values.first().map_or_else(|| format!("{field}: not recorded"), |(value, _)| {
            format!("{field}: {}", truncate_chars(value, 500))
        });
        json!({
            "claim_id": field,
            "claim": claim,
            "support_status": support,
            "evidence_refs": values.iter().map(|(_, reference)| reference).collect::<BTreeSet<_>>(),
        })
    }).collect();
    let supported = claims
        .iter()
        .filter(|claim| claim["support_status"] == "supported")
        .count();
    let status = if evidence.is_empty() {
        "no_evidence"
    } else if supported == request.kind.fields().len() {
        "complete"
    } else {
        "partial"
    };
    let evidence_by_source: BTreeMap<&str, &str> = evidence
        .iter()
        .filter_map(|item| {
            Some((
                item.get("source_path")?.as_str()?,
                item.get("evidence_id")?.as_str()?,
            ))
        })
        .collect();
    let relations: Vec<Value> = relation_edges
        .iter()
        .filter_map(|edge| {
            let graph = graph.as_ref()?;
            let from = graph.nodes.iter().find(|node| node.id == edge.from_id)?;
            let to = graph.nodes.iter().find(|node| node.id == edge.to_id)?;
            let from_evidence = evidence_by_source.get(from.source_path.as_str())?;
            let to_evidence = evidence_by_source.get(to.source_path.as_str())?;
            Some(json!({
                "edge_id": edge.edge_id,
                "from_evidence_id": from_evidence,
                "to_evidence_id": to_evidence,
                "relation_type": edge.relation_type,
                "origin": edge.origin,
                "review_status": edge.review_status,
                "source_path": edge.source_path,
                "source_hash": edge.source_hash,
            }))
        })
        .collect();
    let stream_trace: Vec<Value> = stream_results
        .iter()
        .map(|result| {
            json!({
                "stream": result.stream.as_str(),
                "output_count": result.hits.len(),
                "duration_ms": result.duration_ms,
                "error": result.error,
            })
        })
        .collect();
    let stream_error = stream_results.iter().find_map(|result| {
        result
            .error
            .as_ref()
            .map(|error| format!("{}:{error}", result.stream.as_str()))
    });
    let graph_fallback = (matches!(request.mode, EvidenceMode::Hybrid | EvidenceMode::GraphOnly)
        && graph.is_none())
    .then(|| graph_status.clone());
    let fallback_reason = if manifest_used {
        Some("manifest_scan".to_owned())
    } else {
        graph_fallback.or(stream_error)
    };
    let status = if fallback_reason.is_some() && !evidence.is_empty() {
        "fallback"
    } else if request.mode == EvidenceMode::GraphOnly && graph.is_none() {
        "blocked"
    } else {
        status
    };
    let discover_duration = stream_results
        .iter()
        .map(|result| result.duration_ms)
        .max()
        .unwrap_or(0);
    let mut filter_reasons = rejected.clone();
    filter_reasons.extend(source_rejections);
    filter_reasons.sort();
    filter_reasons.dedup();
    let mut packet = json!({
        "schema_version": contract.packet_schema,
        "packet_kind": request.kind.as_str(),
        "status": status,
        "namespace": request.namespace,
        "decision_scope": (request.kind == EvidenceKind::Decision)
            .then_some(request.decision_scope.as_str()),
        "generated_at": Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        "duration_ms": started.elapsed().as_millis(),
        "mode_used": match request.mode {
            EvidenceMode::DirectQmd => "qmd_only",
            EvidenceMode::QmdOnly => "qmd_only",
            EvidenceMode::Hybrid if graph.is_some() => "hybrid",
            EvidenceMode::Hybrid => "qmd_only",
            EvidenceMode::GraphOnly => "graph_only",
        },
        "fallback_reason": fallback_reason,
        "summary": format!("{} evidence: {supported}/{} required fields supported", request.kind.as_str(), request.kind.fields().len()),
        "claims": claims,
        "evidence": evidence,
        "relations": relations,
        "trace": {
            "stages": [
                {"stage": "discover", "input_count": 1, "output_count": candidate_trace.len(), "duration_ms": discover_duration, "reason_codes": stream_results.iter().filter_map(|result| result.error.as_ref().map(|error| format!("{}:{error}", result.stream.as_str()))).collect::<Vec<_>>()},
                {"stage": "expand", "input_count": candidate_trace.len(), "output_count": evidence_by_source.len(), "duration_ms": 0, "reason_codes": if expansion_limited { vec!["expansion_limited"] } else { Vec::<&str>::new() }},
                {"stage": "filter", "input_count": candidate_trace.len(), "output_count": evidence_by_source.len(), "duration_ms": 0, "reason_codes": filter_reasons},
            ],
            "streams": stream_trace,
            "fusion": "rrf.v1",
            "candidates": candidate_trace,
            "candidate_count": stream_results.iter().map(|result| result.hits.len()).sum::<usize>(),
            "accepted_count": evidence_by_source.len(),
            "rejected_reason_codes": rejected,
            "graph_status": graph_status,
            "expansion_limited": expansion_limited,
        },
        "health": {
            "source_of_truth": "obsidian-markdown",
            "read_only": true,
            "writes_enabled": false,
            "raw_query_persisted": false,
            "graph_fresh": graph.is_some(),
            "cache_fresh": false,
            "qmd_available": stream_results.iter().any(|result| result.error.is_none()),
            "provenance_complete": evidence.iter().all(|item| item.get("source_hash").and_then(Value::as_str).is_some()),
        },
    });
    bound_packet(&mut packet, contract.max_packet_bytes);
    Ok(packet)
}

/// Run the canonical Q01..Q20 blind evaluation without persisting query text.
pub fn evaluate(
    root: &Path,
    contract: &Contract,
    options: &CompileOptions,
    repeat: usize,
) -> Result<Value, EvidenceError> {
    if !(1..=10).contains(&repeat) {
        return Err(EvidenceError::InvalidRequest("repeat must be 1..10".into()));
    }
    let evaluation_path = secure_source_path(root, Path::new(&contract.evaluation_source))?;
    let fixtures = load_evaluation(&evaluation_path, contract)?;
    let namespace = contract.evaluation_namespace.clone();
    let mut rows = Vec::new();
    for fixture in fixtures {
        let expected: BTreeSet<String> = fixture
            .expected_evidence_paths
            .iter()
            .map(|path| normalize_candidate_path(path))
            .collect();
        let mut runs = Vec::new();
        for (mode_name, mode) in [
            ("direct_qmd", EvidenceMode::DirectQmd),
            ("qmd_fused", EvidenceMode::QmdOnly),
            ("hybrid", EvidenceMode::Hybrid),
        ] {
            for attempt in 1..=repeat {
                let packet = compile(
                    root,
                    contract,
                    &EvidenceRequest {
                        kind: if contract.agent_run_fixture_ids.contains(&fixture.id) {
                            EvidenceKind::AgentRun
                        } else {
                            EvidenceKind::Decision
                        },
                        query: fixture.query_ja.clone(),
                        namespace: namespace.clone(),
                        sensitivity_ceiling: contract.evaluation_sensitivity_ceiling.clone(),
                        max_sources: contract.max_sources.min(12),
                        timeout_ms: contract.hard_timeout_ms,
                        decision_scope: contract.default_decision_scope,
                        mode,
                    },
                    options,
                )?;
                let ordered_paths: Vec<String> = packet["evidence"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|item| item["source_path"].as_str().map(str::to_owned))
                    .collect();
                let paths: BTreeSet<String> = ordered_paths.iter().cloned().collect();
                let hits: Vec<String> = expected.intersection(&paths).cloned().collect();
                runs.push(json!({
                    "mode": mode_name,
                    "attempt": attempt,
                    "status": packet["status"],
                    "duration_ms": packet["duration_ms"],
                    "hits": hits,
                    "complete": expected.is_subset(&paths),
                    "source_paths": ordered_paths,
                    "graph_fresh": packet["health"]["graph_fresh"],
                    "fallback_reason": packet["fallback_reason"],
                    "provenance_complete": packet["health"]["provenance_complete"],
                    "restricted_leak": packet["evidence"].as_array().into_iter().flatten().any(|item| matches!(item["sensitivity"].as_str(), Some("private" | "restricted"))),
                    "unsupported_claims": packet["claims"].as_array().into_iter().flatten().filter(|claim| claim["support_status"] != "supported").count(),
                }));
            }
        }
        rows.push(json!({"fixture_id": fixture.id, "runs": runs}));
    }
    let direct = aggregate_evaluation(&rows, "direct_qmd");
    let qmd = aggregate_evaluation(&rows, "qmd_fused");
    let hybrid = aggregate_evaluation(&rows, "hybrid");
    let incremental = hybrid["complete_queries"].as_u64().unwrap_or(0) as i64
        - qmd["complete_queries"].as_u64().unwrap_or(0) as i64;
    let direct_hit = direct["expected_hit_queries"].as_u64().unwrap_or(0);
    let direct_complete = direct["complete_queries"].as_u64().unwrap_or(0);
    let qmd_hit = qmd["expected_hit_queries"].as_u64().unwrap_or(0);
    let qmd_complete = qmd["complete_queries"].as_u64().unwrap_or(0);
    let qmd_no_regression = qmd["expected_hit_queries"].as_u64().unwrap_or(0) >= direct_hit
        && qmd["complete_queries"].as_u64().unwrap_or(0) >= direct_complete;
    let qmd_baseline = qmd_hit >= 17 && qmd_complete >= 6;
    let graph_ready = evaluation_runs(&rows)
        .filter(|run| run["mode"] == "hybrid")
        .all(|run| run["graph_fresh"] == true);
    let deterministic = evaluation_deterministic(&rows);
    let provenance_complete = evaluation_runs(&rows).all(|run| run["provenance_complete"] == true);
    let no_restricted_leak = evaluation_runs(&rows).all(|run| run["restricted_leak"] == false);
    let hard_timeout = evaluation_runs(&rows).all(|run| {
        run["duration_ms"]
            .as_u64()
            .is_some_and(|duration| duration <= contract.hard_timeout_ms)
    });
    let graph_no_regression = hybrid["expected_hit_queries"].as_u64().unwrap_or(0)
        >= qmd["expected_hit_queries"].as_u64().unwrap_or(0);
    let intrusion_ok = unrelated_count(&rows, "hybrid") <= unrelated_count(&rows, "qmd_fused") + 1;
    let promotion = qmd_baseline
        && graph_ready
        && qmd_no_regression
        && deterministic
        && provenance_complete
        && no_restricted_leak
        && hard_timeout
        && graph_no_regression
        && intrusion_ok
        && hybrid["expected_hit_queries"].as_u64().unwrap_or(0) >= 18
        && hybrid["complete_queries"].as_u64().unwrap_or(0) >= 12
        && incremental >= 4;
    Ok(json!({
        "schema_version": "okf-evidence-evaluation.v2",
        "fixture_count": rows.len(),
        "repeat": repeat,
        "query_text_persisted": false,
        "direct_qmd": direct,
        "qmd_only": qmd.clone(),
        "qmd_fused": qmd,
        "hybrid": hybrid,
        "incremental_complete": incremental,
        "gates": {"qmd_baseline": qmd_baseline, "qmd_no_regression": qmd_no_regression, "graph_ready": graph_ready, "deterministic_source_order": deterministic, "privacy": true, "provenance_complete": provenance_complete, "no_restricted_leak": no_restricted_leak, "hard_timeout": hard_timeout, "graph_no_regression": graph_no_regression, "unrelated_intrusion": intrusion_ok},
        "promotion": {"pass": promotion, "graph_default_enabled": promotion},
        "fixtures": rows,
    }))
}

fn evaluation_runs(rows: &[Value]) -> impl Iterator<Item = &Value> {
    rows.iter()
        .flat_map(|row| row["runs"].as_array().into_iter().flatten())
}

fn unrelated_count(rows: &[Value], mode: &str) -> usize {
    rows.iter()
        .filter_map(|row| {
            row["runs"]
                .as_array()?
                .iter()
                .find(|run| run["mode"] == mode)
        })
        .map(|run| {
            let source_count = run["source_paths"].as_array().map_or(0, Vec::len);
            let hit_count = run["hits"].as_array().map_or(0, Vec::len);
            source_count.saturating_sub(hit_count)
        })
        .sum()
}

fn evaluation_deterministic(rows: &[Value]) -> bool {
    rows.iter().all(|row| {
        ["direct_qmd", "qmd_fused", "hybrid"].iter().all(|mode| {
            let orders: Vec<&Value> = row["runs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|run| run["mode"] == **mode)
                .map(|run| &run["source_paths"])
                .collect();
            orders.windows(2).all(|pair| pair[0] == pair[1])
        })
    })
}

fn aggregate_evaluation(rows: &[Value], mode: &str) -> Value {
    let first: Vec<&Value> = rows
        .iter()
        .filter_map(|row| {
            row["runs"]
                .as_array()?
                .iter()
                .find(|run| run["mode"] == mode)
        })
        .collect();
    let mut durations: Vec<u64> = first
        .iter()
        .filter_map(|run| run["duration_ms"].as_u64())
        .collect();
    durations.sort_unstable();
    let p95_index = durations
        .len()
        .saturating_mul(95)
        .div_ceil(100)
        .saturating_sub(1);
    json!({
        "expected_hit_queries": first.iter().filter(|run| run["hits"].as_array().is_some_and(|hits| !hits.is_empty())).count(),
        "complete_queries": first.iter().filter(|run| run["complete"] == true).count(),
        "p95_ms": durations.get(p95_index).copied().unwrap_or(0),
    })
}

/// Return read-only health without running discovery.
pub fn health(root: &Path, contract: &Contract, graph_path: Option<&Path>) -> Value {
    let graph = graph_path.map_or_else(
        || json!({"ok": false, "reason": "graph_not_configured"}),
        |path| match graph::load_graph(root, path, &contract.graph_schema) {
            Ok(graph) => json!({"ok": true, "nodes": graph.nodes.len(), "edges": graph.edges.len(), "hash": graph.normalized_graph_hash}),
            Err(error) => json!({"ok": false, "reason": graph_error_code(&error)}),
        },
    );
    json!({
        "ok": true,
        "status": if graph["ok"] == true { "ready" } else { "fallback" },
        "source_of_truth": "obsidian-markdown",
        "read_only": true,
        "writes_enabled": false,
        "raw_query_persistence": false,
        "qmd_fallback": true,
        "graph": graph,
    })
}

#[derive(Debug)]
struct RankedCandidate {
    path: String,
    score: f64,
    streams: BTreeMap<CandidateStream, usize>,
    reasons: BTreeSet<String>,
}

fn fuse_candidates(
    streams: &[StreamResult],
    policy: &NamespacePolicy,
    contract: &Contract,
    kind: EvidenceKind,
) -> (Vec<RankedCandidate>, Vec<String>) {
    let mut scores: BTreeMap<String, RankedCandidate> = BTreeMap::new();
    let mut rejected = Vec::new();
    for result in streams {
        for hit in &result.hits {
            match path_allowed(&hit.path, policy, contract) {
                Ok(()) => {
                    let entry = scores
                        .entry(hit.path.clone())
                        .or_insert_with(|| RankedCandidate {
                            path: hit.path.clone(),
                            score: 0.0,
                            streams: BTreeMap::new(),
                            reasons: BTreeSet::new(),
                        });
                    entry.score += 1.0 / (60.0 + hit.rank as f64 + 1.0);
                    entry.streams.insert(hit.stream, hit.rank);
                    entry.reasons.extend(hit.reasons.iter().cloned());
                }
                Err(code) => rejected.push(code),
            }
        }
    }
    for candidate in scores.values_mut() {
        candidate.score += authority_prior(kind, &candidate.path);
    }
    let mut rows: Vec<_> = scores.into_values().collect();
    rows.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.streams.len().cmp(&left.streams.len()))
            .then_with(|| left.path.cmp(&right.path))
    });
    rejected.sort();
    rejected.dedup();
    (rows, rejected)
}

/// Remove non-scope Decision candidates before bounded ranking output.
fn filter_decision_candidates(
    root: &Path,
    candidates: Vec<RankedCandidate>,
    request: &EvidenceRequest,
    contract: &Contract,
    rejected: &mut Vec<String>,
) -> Result<Vec<RankedCandidate>, EvidenceError> {
    let Some(allowed_statuses) = contract
        .decision_scopes
        .get(request.decision_scope.as_str())
    else {
        return Ok(candidates);
    };
    let mut accepted = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        match inspect_decision_status(root, &candidate.path, contract.max_source_bytes) {
            Ok(Some(status)) if allowed_statuses.contains(&status) => accepted.push(candidate),
            Ok(Some(_)) => rejected.push("decision_scope_filtered".into()),
            Ok(None) => accepted.push(candidate),
            Err(error) => rejected.push(error.stable_code().into()),
        }
    }
    Ok(accepted)
}

/// Read just enough canonical source to classify one Decision candidate.
fn inspect_decision_status(
    root: &Path,
    path: &str,
    max_source_bytes: u64,
) -> Result<Option<DecisionStatus>, EvidenceError> {
    let relative = Path::new(path);
    if path.is_empty()
        || relative.is_absolute()
        || relative.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(EvidenceError::PathRejected(path.into()));
    }
    let candidate = root.join(relative);
    if !candidate.exists() {
        return Ok(None);
    }
    let absolute = secure_source_path(root, relative)?;
    let metadata = fs::metadata(&absolute).map_err(|source| EvidenceError::Io {
        context: format!("inspect decision source {path}"),
        source,
    })?;
    if metadata.len() > max_source_bytes {
        return Err(EvidenceError::PathRejected(format!(
            "source_too_large:{path}"
        )));
    }
    let raw = fs::read(&absolute).map_err(|source| EvidenceError::Io {
        context: format!("read decision source {path}"),
        source,
    })?;
    let text = std::str::from_utf8(raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&raw)).map_err(
        |_| EvidenceError::InvalidDecisionRecord(format!("{path}: source is not UTF-8")),
    )?;
    let frontmatter = parse_frontmatter(text)
        .map_err(|error| EvidenceError::InvalidDecisionRecord(format!("{path}: {error}")))?;
    let metadata = normalize_decision_metadata(&frontmatter)
        .map_err(|error| EvidenceError::InvalidDecisionRecord(format!("{path}: {error}")))?;
    Ok(Some(metadata.decision_status))
}

fn authority_prior(kind: EvidenceKind, path: &str) -> f64 {
    let lower = path.to_ascii_lowercase();
    match kind {
        EvidenceKind::Decision
            if lower.contains("/design/")
                || lower.contains("/docs/adr/")
                || lower.ends_with("context.md") =>
        {
            0.02
        }
        EvidenceKind::Decision if lower.contains("/research/") => 0.015,
        EvidenceKind::Decision if lower.contains("/issues/") || lower.contains("/progress/") => {
            0.008
        }
        EvidenceKind::AgentRun
            if lower.contains("/progress/")
                || lower.contains("/issues/")
                || lower.contains("/feedback-events/") =>
        {
            0.02
        }
        EvidenceKind::AgentRun if lower.ends_with("context.md") || lower.contains("/design/") => {
            0.008
        }
        _ => 0.0,
    }
}

fn graph_seed_paths(
    graph: &Graph,
    request: &EvidenceRequest,
    allowed_statuses: Option<&BTreeSet<DecisionStatus>>,
) -> Vec<String> {
    let terms = query_terms(&request.query);
    let ceiling = Sensitivity::parse(&request.sensitivity_ceiling).unwrap_or(Sensitivity::Internal);
    let mut rows: Vec<(usize, String)> = graph
        .nodes
        .iter()
        .filter_map(|node| {
            if node.namespace != request.namespace
                || Sensitivity::parse(&node.sensitivity).ok()? > ceiling
                || node.sensitivity == "restricted"
            {
                return None;
            }
            if node.node_type == "Decision" {
                let status = graph::decision_node_status(node)
                    .ok()
                    .flatten()
                    .unwrap_or(DecisionStatus::Adopted);
                let allowed = allowed_statuses.map_or_else(
                    || request.decision_scope.allows(status),
                    |set| set.contains(&status),
                );
                if !allowed {
                    return None;
                }
            }
            let haystack = format!("{} {}", node.title, node.source_path).to_lowercase();
            let score = terms
                .iter()
                .filter(|term| haystack.contains(term.as_str()))
                .count();
            (score > 0).then(|| (score, node.source_path.clone()))
        })
        .collect();
    rows.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    rows.into_iter().map(|(_, path)| path).collect()
}

fn blend_hybrid_candidates(
    qmd_paths: Vec<String>,
    graph_paths: impl IntoIterator<Item = String>,
    max_sources: usize,
    graph_quota: usize,
) -> Vec<String> {
    let target_count = qmd_paths.len().min(max_sources);
    if target_count == 0 {
        return graph_paths.into_iter().take(max_sources).collect();
    }
    let qmd_set: BTreeSet<&str> = qmd_paths.iter().map(String::as_str).collect();
    let mut graph_paths: Vec<String> = graph_paths
        .into_iter()
        .filter(|path| !path.is_empty() && !qmd_set.contains(path.as_str()))
        .collect();
    graph_paths.dedup();
    let reserve = graph_quota.min(graph_paths.len()).min(target_count);
    let mut blended: Vec<String> = qmd_paths
        .iter()
        .take(target_count - reserve)
        .cloned()
        .collect();
    for path in graph_paths {
        if blended.len() >= target_count {
            break;
        }
        if !blended.contains(&path) {
            blended.push(path);
        }
    }
    for path in qmd_paths {
        if blended.len() >= target_count {
            break;
        }
        if !blended.contains(&path) {
            blended.push(path);
        }
    }
    blended
}

fn query_terms(query: &str) -> Vec<String> {
    let mut values = BTreeSet::new();
    for token in query
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
    {
        if token.chars().count() >= 2 {
            values.insert(token.to_owned());
        }
    }
    values.into_iter().take(16).collect()
}

fn manifest_scan(
    root: &Path,
    request: &EvidenceRequest,
    policy: &NamespacePolicy,
    contract: &Contract,
    limit: usize,
) -> Vec<String> {
    let terms = query_terms(&request.query);
    if terms.is_empty() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for policy_root in &policy.roots {
        let absolute = root.join(policy_root);
        let Ok(metadata) = fs::symlink_metadata(&absolute) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_file() {
            score_manifest_file(root, &absolute, &terms, policy, contract, &mut candidates);
        } else if metadata.is_dir() {
            walk_manifest(root, &absolute, &terms, policy, contract, &mut candidates);
        }
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    candidates
        .into_iter()
        .take(limit)
        .map(|(_, path)| path)
        .collect()
}

fn walk_manifest(
    root: &Path,
    directory: &Path,
    terms: &[String],
    policy: &NamespacePolicy,
    contract: &Contract,
    output: &mut Vec<(usize, String)>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !matches!(name.as_str(), ".git" | "node_modules" | "target") {
                walk_manifest(root, &path, terms, policy, contract, output);
            }
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            score_manifest_file(root, &path, terms, policy, contract, output);
        }
    }
}

fn score_manifest_file(
    root: &Path,
    path: &Path,
    terms: &[String],
    policy: &NamespacePolicy,
    contract: &Contract,
    output: &mut Vec<(usize, String)>,
) {
    let Ok(relative) = path.strip_prefix(root) else {
        return;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    if path_allowed(&relative, policy, contract).is_err() {
        return;
    }
    let Ok(secure) = secure_source_path(root, Path::new(&relative)) else {
        return;
    };
    let Ok(file) = fs::File::open(secure) else {
        return;
    };
    let mut bytes = Vec::new();
    if file.take(32 * 1024).read_to_end(&mut bytes).is_err() {
        return;
    }
    let haystack = format!("{} {}", relative, String::from_utf8_lossy(&bytes)).to_lowercase();
    let score = terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count();
    if score > 0 {
        output.push((score, relative));
    }
}

fn path_allowed(path: &str, policy: &NamespacePolicy, contract: &Contract) -> Result<(), String> {
    let normalized = path.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    if normalized.is_empty()
        || Path::new(&normalized).is_absolute()
        || Path::new(&normalized).components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("path_escape".into());
    }
    if lower.contains("/data/okf-") || lower.contains("/data/okf_") {
        return Err("derived_artifact".into());
    }
    if contract
        .restricted_path_fragments
        .iter()
        .any(|fragment| lower.contains(&fragment.to_ascii_lowercase()))
    {
        return Err("restricted_path".into());
    }
    if !policy.roots.iter().any(|root| {
        normalized == *root || normalized.starts_with(&format!("{}/", root.trim_end_matches('/')))
    }) {
        return Err("outside_namespace".into());
    }
    Ok(())
}

/// Normalize qmd URIs and known vault top-level aliases.
pub fn normalize_candidate_path(value: &str) -> String {
    let mut raw = percent_decode(value);
    if let Some(rest) = raw.strip_prefix("qmd://") {
        raw = rest.split_once('/').map_or("", |(_, path)| path).to_owned();
    }
    raw = raw.replace('\\', "/").trim_start_matches('/').to_owned();
    for (alias, canonical) in [
        ("1-Fleeting/", "1_Fleeting/"),
        ("2-Literature/", "2_Literature/"),
        ("3-Permanent/", "3_Permanent/"),
        ("4-Project/", "4_Project/"),
        ("5-Structure/", "5_Structure/"),
    ] {
        if let Some(rest) = raw.strip_prefix(alias) {
            raw = format!("{canonical}{rest}");
            break;
        }
    }
    let without_line = raw
        .rsplit_once(':')
        .filter(|(_, suffix)| suffix.chars().all(|ch| ch.is_ascii_digit()))
        .map_or(raw.as_str(), |(path, _)| path);
    without_line.to_owned()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_digit(bytes[index + 1]), hex_digit(bytes[index + 2]))
            {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

const fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

/// Resolve a regular file beneath root, rejecting traversal and symlink escapes.
pub fn secure_source_path(root: &Path, value: &Path) -> Result<PathBuf, EvidenceError> {
    if value.is_absolute()
        || value.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(EvidenceError::PathRejected(value.display().to_string()));
    }
    let canonical_root = root.canonicalize().map_err(|source| EvidenceError::Io {
        context: format!("resolve root {}", root.display()),
        source,
    })?;
    let candidate = canonical_root
        .join(value)
        .canonicalize()
        .map_err(|source| EvidenceError::Io {
            context: format!("resolve source {}", value.display()),
            source,
        })?;
    if !candidate.starts_with(&canonical_root) || !candidate.is_file() {
        return Err(EvidenceError::PathRejected(value.display().to_string()));
    }
    Ok(candidate)
}

/// SHA-256 with the shared `sha256:` prefix.
pub fn sha256_prefixed(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn read_evidence(
    root: &Path,
    path: &str,
    kind: EvidenceKind,
    sensitivity: Sensitivity,
    max_source_bytes: u64,
    graph_source_type: Option<&str>,
) -> Result<(Value, BTreeMap<String, String>), EvidenceError> {
    let absolute = secure_source_path(root, Path::new(path))?;
    let metadata = fs::metadata(&absolute).map_err(|source| EvidenceError::Io {
        context: format!("inspect source {path}"),
        source,
    })?;
    if metadata.len() > max_source_bytes {
        return Err(EvidenceError::PathRejected(format!(
            "source_too_large:{path}"
        )));
    }
    let raw = fs::read(&absolute).map_err(|source| EvidenceError::Io {
        context: format!("read source {path}"),
        source,
    })?;
    let text = std::str::from_utf8(raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&raw))
        .map_err(|_| EvidenceError::PathRejected(format!("source_not_utf8:{path}")))?;
    let decision_frontmatter =
        if kind == EvidenceKind::Decision {
            Some(parse_frontmatter(text).map_err(|error| {
                EvidenceError::InvalidDecisionRecord(format!("{path}: {error}"))
            })?)
        } else {
            None
        };
    let decision_metadata = decision_frontmatter
        .as_ref()
        .map(|frontmatter| {
            normalize_decision_metadata(frontmatter)
                .map_err(|error| EvidenceError::InvalidDecisionRecord(format!("{path}: {error}")))
        })
        .transpose()?;
    let fields = extract_fields(
        kind,
        text,
        decision_frontmatter.as_ref(),
        decision_metadata.as_ref(),
    );
    let title = text
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .unwrap_or_else(|| {
            absolute
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("source")
        });
    let source_hash = sha256_prefixed(&raw);
    let content_hash = sha256_prefixed(text.replace("\r\n", "\n").as_bytes());
    let id_seed = format!("{path}{source_hash}");
    let evidence_id = format!(
        "ev-{}",
        &hex::encode(Sha256::digest(id_seed.as_bytes()))[..12]
    );
    let excerpt = fields.values().next().map_or_else(
        || title.to_owned(),
        |value| truncate_chars(value, EXCERPT_LIMIT),
    );
    let freshness = source_freshness(
        kind,
        text,
        decision_frontmatter.as_ref(),
        decision_metadata.as_ref(),
    );
    let mut item = json!({
        "evidence_id": evidence_id,
        "source_path": path,
        "source_hash": source_hash,
        "content_hash": content_hash,
        "heading": Value::Null,
        "anchor": Value::Null,
        "excerpt": excerpt,
        "source_type": graph_source_type.unwrap_or_else(|| kind.as_str()),
        "freshness": freshness,
        "sensitivity": sensitivity.as_str(),
    });
    if let Some(metadata) = &decision_metadata {
        item["record_status"] = Value::String(metadata.decision_status.as_str().into());
        item["decision_status"] = Value::String(metadata.decision_status.as_str().into());
        item["decision_kind"] = metadata
            .decision_kind
            .as_ref()
            .map_or(Value::Null, |value| Value::String(value.clone()));
        item["recorded_by"] = Value::String(metadata.recorded_by.clone());
        item["review_status"] = Value::String(metadata.review_status.clone());
        item["adoption_mode"] = Value::String(metadata.adoption_mode.clone());
        item["impact"] = Value::String(metadata.impact.clone());
        item["source_refs"] = serde_json::to_value(&metadata.source_refs).unwrap_or(Value::Null);
        item["supersedes"] = serde_json::to_value(&metadata.supersedes).unwrap_or(Value::Null);
    }
    Ok((item, fields))
}

fn source_freshness(
    kind: EvidenceKind,
    text: &str,
    decision_frontmatter: Option<&BTreeMap<String, YamlValue>>,
    decision_metadata: Option<&DecisionMetadata>,
) -> &'static str {
    let (legacy_frontmatter, _) = split_frontmatter(text);
    let stale_after = decision_frontmatter
        .and_then(|frontmatter| frontmatter_text(frontmatter, "stale_after"))
        .or_else(|| legacy_frontmatter.get("stale_after").cloned());
    if let Some(value) = stale_after {
        if let Ok(date) = chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d") {
            return if date < Utc::now().date_naive() {
                "stale"
            } else {
                "fresh"
            };
        }
    }
    if kind == EvidenceKind::Decision
        && decision_metadata
            .is_some_and(|metadata| metadata.decision_status == DecisionStatus::Adopted)
    {
        "timeless"
    } else {
        "fresh"
    }
}

fn extract_fields(
    kind: EvidenceKind,
    text: &str,
    decision_frontmatter: Option<&BTreeMap<String, YamlValue>>,
    decision_metadata: Option<&DecisionMetadata>,
) -> BTreeMap<String, String> {
    let (legacy_frontmatter, body) = split_frontmatter(text);
    let headings = heading_sections(body);
    let aliases: &[(&str, &[&str])] = match kind {
        EvidenceKind::Decision => &[
            ("decision", &["decision", "決定"]),
            (
                "record_status",
                &["record_status", "decision_status", "status"],
            ),
            (
                "rationale",
                &["rationale", "why", "理由", "背景", "context"],
            ),
            (
                "alternatives",
                &["alternatives", "alternatives considered", "代替案"],
            ),
        ],
        EvidenceKind::AgentRun => &[
            (
                "source_issue",
                &["source_issue", "issue_key", "issue", "source issue"],
            ),
            ("run_id", &["run_id", "attempt", "run", "run id"]),
            (
                "artifact",
                &["artifact", "evidence", "artifacts", "handoff"],
            ),
            ("outcome", &["outcome", "result", "status"]),
        ],
    };
    let mut output = BTreeMap::new();
    for (field, names) in aliases {
        let value = names.iter().find_map(|name| {
            decision_frontmatter
                .and_then(|frontmatter| frontmatter_text(frontmatter, name))
                .or_else(|| legacy_frontmatter.get(*name).cloned())
        });
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            output.insert((*field).to_owned(), value.clone());
            continue;
        }
        if let Some((_, value)) = headings
            .iter()
            .find(|(heading, value)| names.contains(&heading.as_str()) && !value.is_empty())
        {
            output.insert((*field).to_owned(), truncate_chars(value, EXCERPT_LIMIT));
        }
    }
    if let Some(metadata) = decision_metadata {
        output.insert(
            "record_status".to_owned(),
            metadata.decision_status.as_str().to_owned(),
        );
    }
    output
}

fn split_frontmatter(text: &str) -> (BTreeMap<String, String>, &str) {
    let normalized = text.strip_prefix("\u{feff}").unwrap_or(text);
    let open_len = if normalized.starts_with("---\r\n") {
        5
    } else if normalized.starts_with("---\n") {
        4
    } else {
        return (BTreeMap::new(), normalized);
    };
    let rest = &normalized[open_len..];
    if let Some(close_newline) = rest.find("\n---") {
        let close_start = close_newline + 1;
        let after_marker = &rest[close_start + 3..];
        let body_start = if after_marker.is_empty() {
            close_start + 3
        } else if after_marker.starts_with("\r\n") {
            close_start + 5
        } else if after_marker.starts_with('\n') {
            close_start + 4
        } else {
            return (BTreeMap::new(), normalized);
        };
        let front = &rest[..close_newline];
        let values = front
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once(':')?;
                let value = value.trim().trim_matches(['\'', '"']);
                (!value.is_empty()).then(|| (key.trim().to_ascii_lowercase(), value.to_owned()))
            })
            .collect();
        return (values, &normalized[open_len + body_start..]);
    }
    (BTreeMap::new(), normalized)
}

fn heading_sections(body: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = body.lines().collect();
    let mut output = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
        if hashes == 0 || hashes > 6 || line.as_bytes().get(hashes) != Some(&b' ') {
            continue;
        }
        let title = line[hashes + 1..].trim().to_lowercase();
        let mut content = Vec::new();
        for later in &lines[index + 1..] {
            let later_hashes = later.bytes().take_while(|byte| *byte == b'#').count();
            if later_hashes > 0
                && later_hashes <= hashes
                && later.as_bytes().get(later_hashes) == Some(&b' ')
            {
                break;
            }
            content.push(*later);
        }
        output.push((title, content.join("\n").trim().to_owned()));
    }
    output
}

fn normalize_claim(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn bound_packet(packet: &mut Value, max_bytes: usize) {
    loop {
        let Ok(encoded) = serde_json::to_vec(packet) else {
            return;
        };
        if encoded.len() <= max_bytes {
            return;
        }
        let Some(evidence) = packet.get_mut("evidence").and_then(Value::as_array_mut) else {
            return;
        };
        let mut changed = false;
        for item in evidence {
            if let Some(excerpt) = item.get_mut("excerpt") {
                if let Some(text) = excerpt.as_str() {
                    if !text.is_empty() {
                        *excerpt = Value::String(truncate_chars(text, text.chars().count() / 2));
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            if let Some(candidates) = packet
                .get_mut("trace")
                .and_then(|trace| trace.get_mut("candidates"))
                .and_then(Value::as_array_mut)
            {
                changed = candidates.pop().is_some();
            }
        }
        if !changed {
            return;
        }
        if packet.get("fallback_reason").is_none_or(Value::is_null) {
            packet["fallback_reason"] = Value::String("excerpt_truncated".into());
        }
    }
}

fn graph_error_code(error: &EvidenceError) -> &'static str {
    match error {
        EvidenceError::InvalidGraph(_) => "invalid_graph",
        EvidenceError::PathRejected(_) => "path_rejected",
        EvidenceError::Io { .. } => "graph_io_error",
        _ => "graph_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_qmd_uri_and_line_suffix() {
        assert_eq!(
            normalize_candidate_path("qmd://vault/4-Project/A%20B.md:17"),
            "4_Project/A B.md"
        );
    }

    #[test]
    fn hybrid_blend_reserves_graph_slots_without_growing_source_count() {
        let qmd = (1..=6).map(|index| format!("qmd/{index}.md")).collect();
        let graph = vec![
            "graph/relevant-a.md".to_owned(),
            "qmd/2.md".to_owned(),
            "graph/relevant-b.md".to_owned(),
        ];

        let blended = blend_hybrid_candidates(qmd, graph, 6, 2);

        assert_eq!(blended.len(), 6);
        assert_eq!(
            &blended[..4],
            ["qmd/1.md", "qmd/2.md", "qmd/3.md", "qmd/4.md"]
        );
        assert!(blended.contains(&"graph/relevant-a.md".to_owned()));
        assert!(blended.contains(&"graph/relevant-b.md".to_owned()));
    }

    #[test]
    fn hybrid_blend_keeps_qmd_bound_when_graph_candidates_duplicate() {
        let qmd = vec!["qmd/a.md".to_owned(), "qmd/b.md".to_owned()];
        let graph = vec!["qmd/a.md".to_owned(), "qmd/b.md".to_owned()];

        assert_eq!(blend_hybrid_candidates(qmd.clone(), graph, 2, 2), qmd);
    }

    #[test]
    fn hybrid_blend_replaces_slots_without_growing_an_underfilled_qmd_set() {
        let qmd = vec!["qmd/a.md".to_owned(), "qmd/b.md".to_owned()];
        let graph = vec![
            "graph/a.md".to_owned(),
            "graph/b.md".to_owned(),
            "graph/c.md".to_owned(),
        ];

        let blended = blend_hybrid_candidates(qmd, graph, 12, 4);

        assert_eq!(blended, ["graph/a.md", "graph/b.md"]);
    }

    #[test]
    fn fuses_every_stream_with_stable_rrf() {
        let policy = NamespacePolicy {
            roots: vec!["docs".into()],
            default_sensitivity: Sensitivity::Internal,
        };
        let contract = Contract {
            request_schema: "evidence-request.v1".into(),
            packet_schema: "evidence-packet.v1".into(),
            graph_schema: "okf-derived-graph.v2".into(),
            namespaces: BTreeMap::new(),
            restricted_path_fragments: vec!["secret".into()],
            max_query_bytes: 10,
            max_source_bytes: 10,
            max_packet_bytes: 10,
            max_sources: 10,
            max_graph_hops: 2,
            max_visited_nodes: 100,
            hard_timeout_ms: 30_000,
            fixture_ids: vec![],
            agent_run_fixture_ids: BTreeSet::new(),
            evaluation_source: String::new(),
            evaluation_namespace: "test".into(),
            evaluation_sensitivity_ceiling: "internal".into(),
            decision_scopes: BTreeMap::new(),
            default_decision_scope: DecisionScope::Current,
        };
        let streams = vec![
            StreamResult {
                stream: CandidateStream::Keyword,
                hits: vec![CandidateHit {
                    path: "docs/A.md".into(),
                    rank: 0,
                    stream: CandidateStream::Keyword,
                    reasons: vec![],
                }],
                error: None,
                duration_ms: 1,
            },
            StreamResult {
                stream: CandidateStream::Semantic,
                hits: vec![CandidateHit {
                    path: "docs/B.md".into(),
                    rank: 0,
                    stream: CandidateStream::Semantic,
                    reasons: vec![],
                }],
                error: None,
                duration_ms: 1,
            },
            StreamResult {
                stream: CandidateStream::Adaptive,
                hits: vec![CandidateHit {
                    path: "docs/A.md".into(),
                    rank: 1,
                    stream: CandidateStream::Adaptive,
                    reasons: vec![],
                }],
                error: None,
                duration_ms: 1,
            },
        ];
        let paths: Vec<_> = fuse_candidates(&streams, &policy, &contract, EvidenceKind::Decision)
            .0
            .into_iter()
            .map(|candidate| candidate.path)
            .collect();
        assert_eq!(paths, ["docs/A.md", "docs/B.md"]);
    }

    #[test]
    fn extracts_decision_fields_without_summary_invention() {
        let text = "---\nstatus: adopted\n---\n# Choice\n## Decision\nUse JSON.\n## Rationale\nPortable.\n## Alternatives\nSQLite.";
        let fields = extract_fields(EvidenceKind::Decision, text, None, None);
        assert_eq!(fields["decision"], "Use JSON.");
        assert_eq!(fields["record_status"], "adopted");
        assert_eq!(fields["rationale"], "Portable.");
        assert_eq!(fields["alternatives"], "SQLite.");
    }

    #[test]
    fn rejects_outside_namespace_and_sensitive_path_fragments() {
        let policy = NamespacePolicy {
            roots: vec!["docs".into()],
            default_sensitivity: Sensitivity::Internal,
        };
        let mut contract = Contract {
            request_schema: String::new(),
            packet_schema: String::new(),
            graph_schema: String::new(),
            namespaces: BTreeMap::new(),
            restricted_path_fragments: vec!["secret".into()],
            max_query_bytes: 1,
            max_source_bytes: 1,
            max_packet_bytes: 1,
            max_sources: 1,
            max_graph_hops: 2,
            max_visited_nodes: 100,
            hard_timeout_ms: 1_000,
            fixture_ids: vec![],
            agent_run_fixture_ids: BTreeSet::new(),
            evaluation_source: String::new(),
            evaluation_namespace: "test".into(),
            evaluation_sensitivity_ceiling: "internal".into(),
            decision_scopes: BTreeMap::new(),
            default_decision_scope: DecisionScope::Current,
        };
        assert!(path_allowed("docs/Decision.md", &policy, &contract).is_ok());
        assert_eq!(
            path_allowed("other/Decision.md", &policy, &contract),
            Err("outside_namespace".into())
        );
        assert_eq!(
            path_allowed("docs/secret.md", &policy, &contract),
            Err("restricted_path".into())
        );
        contract.restricted_path_fragments.clear();
        assert_eq!(
            path_allowed("../escape.md", &policy, &contract),
            Err("path_escape".into())
        );
    }

    #[test]
    fn request_validation_happens_before_discovery() {
        let contract = Contract {
            request_schema: String::new(),
            packet_schema: String::new(),
            graph_schema: String::new(),
            namespaces: BTreeMap::new(),
            restricted_path_fragments: vec![],
            max_query_bytes: 8,
            max_source_bytes: 1,
            max_packet_bytes: 1,
            max_sources: 2,
            max_graph_hops: 2,
            max_visited_nodes: 100,
            hard_timeout_ms: 30_000,
            fixture_ids: vec![],
            agent_run_fixture_ids: BTreeSet::new(),
            evaluation_source: String::new(),
            evaluation_namespace: "test".into(),
            evaluation_sensitivity_ceiling: "internal".into(),
            decision_scopes: BTreeMap::new(),
            default_decision_scope: DecisionScope::Current,
        };
        let request = EvidenceRequest {
            kind: EvidenceKind::Decision,
            query: "too long query".into(),
            namespace: "missing".into(),
            sensitivity_ceiling: "internal".into(),
            max_sources: 1,
            timeout_ms: 1_000,
            decision_scope: DecisionScope::Current,
            mode: EvidenceMode::QmdOnly,
        };
        assert!(matches!(
            validate_request(&request, &contract),
            Err(EvidenceError::InvalidRequest(_))
        ));
    }

    #[test]
    fn packet_bound_drops_optional_candidate_trace_after_excerpts() {
        let mut packet = json!({
            "evidence": [{"excerpt": "x".repeat(200)}],
            "trace": {"candidates": (0..40).map(|index| json!({"source_path": format!("docs/{index}.md"), "reasons": ["y".repeat(160)]})).collect::<Vec<_>>()},
            "fallback_reason": Value::Null,
        });
        bound_packet(&mut packet, 1024);
        assert!(serde_json::to_vec(&packet).unwrap().len() <= 1024);
    }

    #[test]
    fn manifest_fallback_does_not_follow_symlink_outside_root() {
        let vault = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(vault.path().join("docs")).unwrap();
        fs::write(outside.path().join("Leak.md"), "# leak sentinel").unwrap();
        let link = vault.path().join("docs/Leak.md");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(outside.path().join("Leak.md"), &link).is_err() {
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("Leak.md"), &link).unwrap();
        let policy = NamespacePolicy {
            roots: vec!["docs".into()],
            default_sensitivity: Sensitivity::Internal,
        };
        let contract = Contract {
            request_schema: String::new(),
            packet_schema: String::new(),
            graph_schema: String::new(),
            namespaces: BTreeMap::new(),
            restricted_path_fragments: vec![],
            max_query_bytes: 100,
            max_source_bytes: 100,
            max_packet_bytes: 100,
            max_sources: 5,
            max_graph_hops: 2,
            max_visited_nodes: 100,
            hard_timeout_ms: 1_000,
            fixture_ids: vec![],
            agent_run_fixture_ids: BTreeSet::new(),
            evaluation_source: String::new(),
            evaluation_namespace: "test".into(),
            evaluation_sensitivity_ceiling: "internal".into(),
            decision_scopes: BTreeMap::new(),
            default_decision_scope: DecisionScope::Current,
        };
        let request = EvidenceRequest {
            kind: EvidenceKind::Decision,
            query: "leak sentinel".into(),
            namespace: "test".into(),
            sensitivity_ceiling: "internal".into(),
            max_sources: 5,
            timeout_ms: 1_000,
            decision_scope: DecisionScope::Current,
            mode: EvidenceMode::QmdOnly,
        };
        assert!(manifest_scan(vault.path(), &request, &policy, &contract, 10).is_empty());
    }
}
