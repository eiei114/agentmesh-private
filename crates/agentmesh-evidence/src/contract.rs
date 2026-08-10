use crate::{DecisionScope, DecisionStatus, EvidenceError, Sensitivity};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// One canonical blind-evaluation fixture.
#[derive(Debug, Clone, Deserialize)]
pub struct EvaluationFixture {
    /// Stable Q01..Q20 identifier.
    pub id: String,
    /// Reviewed fixture category.
    pub category: String,
    /// Ephemeral query loaded from canonical fixture Markdown.
    pub query_ja: String,
    /// Vault-relative source paths expected for complete coverage.
    pub expected_evidence_paths: Vec<String>,
    /// Recorded QMD candidates retained for historical comparison.
    #[serde(default)]
    pub qmd_top_results: Vec<String>,
}

/// Reviewed runtime policy extracted from the canonical Markdown contract.
#[derive(Debug, Clone)]
pub struct Contract {
    /// Request schema accepted by the compiler.
    pub request_schema: String,
    /// Packet schema emitted by the compiler.
    pub packet_schema: String,
    /// Graph schema accepted for serving.
    pub graph_schema: String,
    /// Namespace roots and default sensitivity.
    pub namespaces: BTreeMap<String, NamespacePolicy>,
    /// Case-insensitive path fragments that are always denied.
    pub restricted_path_fragments: Vec<String>,
    /// Maximum UTF-8 query bytes.
    pub max_query_bytes: usize,
    /// Maximum source bytes read per file.
    pub max_source_bytes: u64,
    /// Maximum serialized packet bytes.
    pub max_packet_bytes: usize,
    /// Maximum emitted evidence sources.
    pub max_sources: usize,
    /// Maximum explicit graph hops.
    pub max_graph_hops: usize,
    /// Maximum visited graph nodes.
    pub max_visited_nodes: usize,
    /// Hard request deadline.
    pub hard_timeout_ms: u64,
    /// Canonical evaluation fixture IDs.
    pub fixture_ids: Vec<String>,
    /// Fixtures evaluated as AgentRun packets.
    pub agent_run_fixture_ids: BTreeSet<String>,
    /// Canonical evaluation source contract path.
    pub evaluation_source: String,
    /// Reviewed default evaluation namespace.
    pub evaluation_namespace: String,
    /// Reviewed default evaluation sensitivity ceiling.
    pub evaluation_sensitivity_ceiling: String,
    /// Decision statuses served by each named retrieval scope.
    pub decision_scopes: BTreeMap<String, BTreeSet<DecisionStatus>>,
    /// Default Decision retrieval scope.
    pub default_decision_scope: DecisionScope,
}

/// One reviewed namespace boundary.
#[derive(Debug, Clone)]
pub struct NamespacePolicy {
    /// Allowed vault-relative roots.
    pub roots: Vec<String>,
    /// Sensitivity assigned to discovered paths without graph metadata.
    pub default_sensitivity: Sensitivity,
}

#[derive(Debug, Deserialize)]
struct Wrapper {
    evidence_compiler_contract: RawContract,
}

#[derive(Debug, Deserialize)]
struct RawContract {
    schema_version: String,
    request_schema: String,
    packet_schema: String,
    graph_schema: String,
    source_of_truth: String,
    raw_query_persistence: String,
    max_query_bytes: usize,
    max_source_bytes: u64,
    max_packet_bytes: usize,
    max_sources: usize,
    max_graph_hops: usize,
    max_visited_nodes: usize,
    hard_timeout_ms: u64,
    namespace_registry: Vec<RawNamespace>,
    sensitivity_order: Vec<String>,
    restricted_path_fragments: Vec<String>,
    #[serde(default)]
    decision_scopes: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    default_decision_scope: Option<String>,
    evaluation: RawEvaluation,
}

#[derive(Debug, Deserialize)]
struct RawNamespace {
    namespace: String,
    roots: Vec<String>,
    default_sensitivity: String,
}

#[derive(Debug, Deserialize)]
struct RawEvaluation {
    source_contract: String,
    expected_query_count: usize,
    fixture_ids: Vec<String>,
    agent_run_fixture_ids: Vec<String>,
    default_namespace: String,
    default_sensitivity_ceiling: String,
}

/// Parse and strictly validate the first YAML fence in the canonical Markdown contract.
pub fn load_contract(path: &Path) -> Result<Contract, EvidenceError> {
    let text = fs::read_to_string(path).map_err(|source| EvidenceError::Io {
        context: format!("read contract {}", path.display()),
        source,
    })?;
    let yaml = yaml_fence(&text).ok_or_else(|| {
        EvidenceError::InvalidContract("canonical Markdown contract lacks a YAML fence".into())
    })?;
    let wrapper: Wrapper = serde_yaml::from_str(yaml)
        .map_err(|error| EvidenceError::InvalidContract(format!("invalid YAML: {error}")))?;
    validate_raw(wrapper.evidence_compiler_contract)
}

/// Parse the canonical evaluation query fence and require exact reviewed IDs.
pub fn load_evaluation(
    path: &Path,
    contract: &Contract,
) -> Result<Vec<EvaluationFixture>, EvidenceError> {
    let text = fs::read_to_string(path).map_err(|source| EvidenceError::Io {
        context: format!("read evaluation source {}", path.display()),
        source,
    })?;
    let yaml = yaml_fences(&text)
        .find(|block| block.trim_start().starts_with("evaluation_queries:"))
        .ok_or_else(|| {
            EvidenceError::InvalidContract("evaluation_queries YAML fence missing".into())
        })?;
    #[derive(Deserialize)]
    struct Evaluation {
        evaluation_queries: Vec<EvaluationFixture>,
    }
    let parsed: Evaluation = serde_yaml::from_str(yaml).map_err(|error| {
        EvidenceError::InvalidContract(format!("invalid evaluation YAML: {error}"))
    })?;
    let ids: Vec<_> = parsed
        .evaluation_queries
        .iter()
        .map(|fixture| fixture.id.clone())
        .collect();
    if ids != contract.fixture_ids {
        return Err(EvidenceError::InvalidContract(
            "evaluation fixture IDs differ from reviewed Q01..Q20".into(),
        ));
    }
    if parsed.evaluation_queries.iter().any(|fixture| {
        fixture.category.trim().is_empty()
            || fixture.query_ja.trim().is_empty()
            || fixture.expected_evidence_paths.is_empty()
    }) {
        return Err(EvidenceError::InvalidContract(
            "evaluation query or expected path is empty".into(),
        ));
    }
    Ok(parsed.evaluation_queries)
}

fn yaml_fence(text: &str) -> Option<&str> {
    yaml_fences(text).next()
}

fn yaml_fences(text: &str) -> impl Iterator<Item = &str> {
    text.split("```yaml").skip(1).filter_map(|rest| {
        let end = rest.find("```")?;
        Some(rest[..end].trim())
    })
}

fn validate_raw(raw: RawContract) -> Result<Contract, EvidenceError> {
    if raw.schema_version != "okf-evidence-contract.v2"
        || raw.request_schema != "evidence-request.v1"
        || raw.packet_schema != "evidence-packet.v1"
        || raw.graph_schema != "okf-derived-graph.v2"
    {
        return Err(EvidenceError::InvalidContract(
            "contract/request/packet/graph schema versions do not match v2".into(),
        ));
    }
    if raw.source_of_truth != "obsidian-markdown" || raw.raw_query_persistence != "forbidden" {
        return Err(EvidenceError::InvalidContract(
            "source_of_truth must be obsidian-markdown and raw query persistence forbidden".into(),
        ));
    }
    if raw.sensitivity_order != ["public", "internal", "private", "restricted"] {
        return Err(EvidenceError::InvalidContract(
            "sensitivity order must be public, internal, private, restricted".into(),
        ));
    }
    let expected_ids: Vec<String> = (1..=20).map(|index| format!("Q{index:02}")).collect();
    if raw.evaluation.expected_query_count != 20 || raw.evaluation.fixture_ids != expected_ids {
        return Err(EvidenceError::InvalidContract(
            "evaluation fixtures must be exactly Q01..Q20".into(),
        ));
    }
    if raw.max_query_bytes == 0
        || raw.max_source_bytes == 0
        || raw.max_packet_bytes == 0
        || raw.max_sources == 0
        || raw.max_graph_hops != 2
        || raw.max_visited_nodes == 0
        || !(1_000..=30_000).contains(&raw.hard_timeout_ms)
    {
        return Err(EvidenceError::InvalidContract(
            "invalid compiler limits".into(),
        ));
    }
    let evaluation_namespace = raw.evaluation.default_namespace.clone();
    let evaluation_sensitivity_ceiling = raw.evaluation.default_sensitivity_ceiling.clone();
    let default_decision_scope =
        DecisionScope::parse(raw.default_decision_scope.as_deref().unwrap_or("current"))
            .map_err(|error| EvidenceError::InvalidContract(error.to_string()))?;
    let mut decision_scopes = BTreeMap::new();
    let raw_decision_scopes = if raw.decision_scopes.is_empty() {
        [
            DecisionScope::Current,
            DecisionScope::Review,
            DecisionScope::Historical,
        ]
        .into_iter()
        .map(|scope| {
            (
                scope.as_str().to_owned(),
                scope
                    .default_statuses()
                    .iter()
                    .map(|status| status.as_str().to_owned())
                    .collect(),
            )
        })
        .collect()
    } else {
        raw.decision_scopes
    };
    for (scope, statuses) in raw_decision_scopes {
        let scope = DecisionScope::parse(&scope)
            .map_err(|error| EvidenceError::InvalidContract(error.to_string()))?;
        let scope_name = scope.as_str().to_owned();
        if statuses.is_empty() {
            return Err(EvidenceError::InvalidContract(format!(
                "decision scope {scope_name} has no statuses"
            )));
        }
        let mut parsed = BTreeSet::new();
        for status in statuses {
            let status = DecisionStatus::parse(&status)
                .map_err(|error| EvidenceError::InvalidContract(error.to_string()))?;
            parsed.insert(status);
        }
        if decision_scopes.insert(scope_name, parsed).is_some() {
            return Err(EvidenceError::InvalidContract(
                "duplicate decision scope".into(),
            ));
        }
    }
    if !decision_scopes.contains_key(default_decision_scope.as_str()) {
        return Err(EvidenceError::InvalidContract(format!(
            "default decision scope {} is not registered",
            default_decision_scope.as_str()
        )));
    }
    let mut namespaces = BTreeMap::new();
    for entry in raw.namespace_registry {
        if entry.namespace.trim().is_empty() || entry.roots.is_empty() {
            return Err(EvidenceError::InvalidContract(
                "namespace or roots are empty".into(),
            ));
        }
        let sensitivity = Sensitivity::parse(&entry.default_sensitivity)?;
        if namespaces
            .insert(
                entry.namespace,
                NamespacePolicy {
                    roots: entry.roots,
                    default_sensitivity: sensitivity,
                },
            )
            .is_some()
        {
            return Err(EvidenceError::InvalidContract("duplicate namespace".into()));
        }
    }
    if namespaces.is_empty() {
        return Err(EvidenceError::InvalidContract(
            "namespace registry is empty".into(),
        ));
    }
    if !namespaces.contains_key(&evaluation_namespace)
        || Sensitivity::parse(&evaluation_sensitivity_ceiling)? == Sensitivity::Restricted
    {
        return Err(EvidenceError::InvalidContract(
            "invalid evaluation namespace or sensitivity ceiling".into(),
        ));
    }
    let agent_ids: BTreeSet<String> = raw.evaluation.agent_run_fixture_ids.into_iter().collect();
    if !agent_ids.iter().all(|id| expected_ids.contains(id)) {
        return Err(EvidenceError::InvalidContract(
            "AgentRun fixture IDs must be members of Q01..Q20".into(),
        ));
    }
    Ok(Contract {
        request_schema: raw.request_schema,
        packet_schema: raw.packet_schema,
        graph_schema: raw.graph_schema,
        namespaces,
        restricted_path_fragments: raw.restricted_path_fragments,
        max_query_bytes: raw.max_query_bytes,
        max_source_bytes: raw.max_source_bytes,
        max_packet_bytes: raw.max_packet_bytes,
        max_sources: raw.max_sources,
        max_graph_hops: raw.max_graph_hops,
        max_visited_nodes: raw.max_visited_nodes,
        hard_timeout_ms: raw.hard_timeout_ms,
        fixture_ids: expected_ids,
        agent_run_fixture_ids: agent_ids,
        evaluation_source: raw.evaluation.source_contract,
        evaluation_namespace,
        evaluation_sensitivity_ceiling,
        decision_scopes,
        default_decision_scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rejects_contract_without_exact_fixture_ids() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "```yaml\nevidence_compiler_contract:\n  schema_version: nope\n```"
        )
        .unwrap();
        assert!(matches!(
            load_contract(file.path()),
            Err(EvidenceError::InvalidContract(_))
        ));
    }

    #[test]
    fn loads_reviewed_decision_scope_registry() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let ids = (1..=20)
            .map(|index| format!("Q{index:02}"))
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!(
            r#"```yaml
evidence_compiler_contract:
  schema_version: okf-evidence-contract.v2
  request_schema: evidence-request.v1
  packet_schema: evidence-packet.v1
  graph_schema: okf-derived-graph.v2
  source_of_truth: obsidian-markdown
  raw_query_persistence: forbidden
  max_query_bytes: 32768
  max_source_bytes: 1048576
  max_packet_bytes: 65536
  max_sources: 12
  max_graph_hops: 2
  max_visited_nodes: 100
  hard_timeout_ms: 30000
  namespace_registry:
    - namespace: test
      roots: [docs]
      default_sensitivity: internal
  sensitivity_order: [public, internal, private, restricted]
  restricted_path_fragments: [secret]
  decision_scopes:
    current: [adopted]
    review: [candidate, deferred]
    historical: [candidate, adopted, rejected, deferred, superseded]
  default_decision_scope: current
  evaluation:
    source_contract: docs/eval.md
    expected_query_count: 20
    fixture_ids: [{ids}]
    agent_run_fixture_ids: [Q18, Q19, Q20]
    default_namespace: test
    default_sensitivity_ceiling: internal
```
"#
        );
        writeln!(file, "{body}").unwrap();

        let contract = load_contract(file.path()).unwrap();

        assert_eq!(contract.default_decision_scope, DecisionScope::Current);
        assert_eq!(
            contract.decision_scopes["review"],
            BTreeSet::from([DecisionStatus::Candidate, DecisionStatus::Deferred])
        );

        let mut invalid_file = tempfile::NamedTempFile::new().unwrap();
        let invalid_body = body.replace("current: [adopted]", "current: []");
        writeln!(invalid_file, "{invalid_body}").unwrap();
        assert!(matches!(
            load_contract(invalid_file.path()),
            Err(EvidenceError::InvalidContract(message))
                if message.contains("has no statuses")
        ));
    }
}
