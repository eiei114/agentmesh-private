use crate::{
    normalize_decision_metadata_for_graph, secure_source_path, sha256_prefixed, DecisionMetadata,
    DecisionScope, DecisionStatus, EvidenceError, EvidenceRequest, Sensitivity,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::Path;

/// Serving-valid v2 graph snapshot.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Graph {
    /// Graph schema version.
    pub schema_version: String,
    /// Declared node count.
    pub node_count: usize,
    /// Declared edge count.
    pub edge_count: usize,
    /// Declared warning count.
    pub warning_count: usize,
    /// Warning payload. Must be empty for serving.
    pub warnings: Vec<Value>,
    /// Stable canonical payload hash.
    pub normalized_graph_hash: String,
    /// Typed nodes.
    pub nodes: Vec<GraphNode>,
    /// Explicit reviewed edges.
    pub edges: Vec<GraphEdge>,
    /// Additional importer metadata retained for hash validation.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One graph node.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GraphNode {
    /// Stable node ID.
    pub id: String,
    /// Node kind.
    #[serde(rename = "type")]
    pub node_type: String,
    /// Display title.
    pub title: String,
    /// Canonical vault-relative source path.
    pub source_path: String,
    /// Source byte hash.
    pub source_hash: String,
    /// Reviewed namespace.
    pub namespace: String,
    /// Reviewed sensitivity.
    pub sensitivity: String,
    /// Additional node metadata retained for hash validation.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One explicit reviewed edge.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GraphEdge {
    /// Stable edge ID.
    pub edge_id: String,
    /// Origin node ID.
    pub from_id: String,
    /// Target node ID.
    pub to_id: String,
    /// Typed relation.
    pub relation_type: String,
    /// Provenance source path.
    pub source_path: String,
    /// Provenance source hash.
    pub source_hash: String,
    /// Relation origin; serving requires `explicit`.
    pub origin: String,
    /// Review status; serving requires `accepted`.
    pub review_status: String,
    /// Additional edge metadata retained for hash validation.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Bounded expansion output.
#[derive(Debug, Clone)]
pub struct Expansion {
    /// Related source paths in traversal order.
    pub paths: Vec<String>,
    /// Traversed explicit edges.
    pub edges: Vec<GraphEdge>,
    /// True when a fanout/node bound stopped expansion.
    pub limited: bool,
}

/// Load, hash-check, and source-check a v2 graph snapshot.
pub fn load_graph(root: &Path, path: &Path, expected_schema: &str) -> Result<Graph, EvidenceError> {
    let absolute = secure_source_path(root, path)?;
    let bytes = fs::read(&absolute).map_err(|source| EvidenceError::Io {
        context: format!("read graph {}", absolute.display()),
        source,
    })?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(EvidenceError::InvalidGraph("graph exceeds 16 MiB".into()));
    }
    let graph: Graph = serde_json::from_slice(&bytes)
        .map_err(|error| EvidenceError::InvalidGraph(format!("invalid graph JSON: {error}")))?;
    validate_graph(root, &graph, expected_schema)?;
    Ok(graph)
}

/// Validate serving invariants without loading from disk.
pub fn validate_graph(
    root: &Path,
    graph: &Graph,
    expected_schema: &str,
) -> Result<(), EvidenceError> {
    if graph.schema_version != expected_schema {
        return Err(EvidenceError::InvalidGraph(format!(
            "unsupported schema {}",
            graph.schema_version
        )));
    }
    if graph.node_count != graph.nodes.len()
        || graph.edge_count != graph.edges.len()
        || graph.warning_count != graph.warnings.len()
        || !graph.warnings.is_empty()
    {
        return Err(EvidenceError::InvalidGraph(
            "graph counts or warnings are not serving-valid".into(),
        ));
    }
    if graph.normalized_graph_hash != normalized_hash(graph)? {
        return Err(EvidenceError::InvalidGraph(
            "normalized graph hash mismatch".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut source_paths = BTreeSet::new();
    for node in &graph.nodes {
        if node.id.is_empty()
            || node.title.is_empty()
            || node.source_path.is_empty()
            || !valid_hash(&node.source_hash)
            || node.namespace.is_empty()
            || node.sensitivity.is_empty()
            || !ids.insert(&node.id)
            || !source_paths.insert(&node.source_path)
        {
            return Err(EvidenceError::InvalidGraph(
                "node fields are missing or duplicated".into(),
            ));
        }
        Sensitivity::parse(&node.sensitivity)?;
        validate_decision_node(node)?;
        if node.source_path != "index.md" {
            let source = secure_source_path(root, Path::new(&node.source_path))?;
            let bytes = fs::read(source).map_err(|source| EvidenceError::Io {
                context: format!("read graph source {}", node.source_path),
                source,
            })?;
            if sha256_prefixed(&bytes) != node.source_hash {
                return Err(EvidenceError::InvalidGraph(format!(
                    "stale source {}",
                    node.source_path
                )));
            }
        }
    }
    let nodes_by_id: BTreeMap<&str, &GraphNode> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    for edge in &graph.edges {
        if edge.edge_id.is_empty()
            || edge.relation_type.is_empty()
            || edge.source_path.is_empty()
            || !valid_hash(&edge.source_hash)
            || !ids.contains(&edge.from_id)
            || !ids.contains(&edge.to_id)
            || edge.origin != "explicit"
            || edge.review_status != "accepted"
        {
            return Err(EvidenceError::InvalidGraph(format!(
                "unsafe edge {}",
                edge.edge_id
            )));
        }
        validate_supersedes_edge(edge, &nodes_by_id)?;
        if edge.source_path != "index.md" {
            let source_path = secure_source_path(root, Path::new(&edge.source_path))?;
            let bytes = fs::read(source_path).map_err(|source| EvidenceError::Io {
                context: format!("read edge provenance {}", edge.source_path),
                source,
            })?;
            if sha256_prefixed(&bytes) != edge.source_hash {
                return Err(EvidenceError::InvalidGraph(format!(
                    "stale edge provenance {}",
                    edge.edge_id
                )));
            }
        }
    }
    Ok(())
}

fn valid_hash(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

/// Enforce that a supersession relation connects two distinct Decision nodes.
fn validate_supersedes_edge(
    edge: &GraphEdge,
    nodes_by_id: &BTreeMap<&str, &GraphNode>,
) -> Result<(), EvidenceError> {
    if edge.relation_type != "supersedes" {
        return Ok(());
    }
    let from = nodes_by_id.get(edge.from_id.as_str());
    let to = nodes_by_id.get(edge.to_id.as_str());
    if edge.from_id == edge.to_id
        || from.is_none_or(|node| node.node_type != "Decision")
        || to.is_none_or(|node| node.node_type != "Decision")
    {
        return Err(EvidenceError::InvalidGraph(format!(
            "supersedes edge must connect distinct Decision nodes: {}",
            edge.edge_id
        )));
    }
    Ok(())
}

fn normalized_hash(graph: &Graph) -> Result<String, EvidenceError> {
    let mut value = serde_json::to_value(graph)
        .map_err(|error| EvidenceError::InvalidGraph(format!("serialize graph: {error}")))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| EvidenceError::InvalidGraph("graph root is not object".into()))?;
    object.remove("generated_at");
    object.remove("normalized_graph_hash");
    for key in ["nodes", "edges"] {
        if let Some(array) = object.get_mut(key).and_then(Value::as_array_mut) {
            array.sort_by(|left, right| {
                let id_key = if key == "nodes" { "id" } else { "edge_id" };
                left.get(id_key)
                    .and_then(Value::as_str)
                    .cmp(&right.get(id_key).and_then(Value::as_str))
            });
        }
    }
    let encoded = serde_json::to_vec(&value)
        .map_err(|error| EvidenceError::InvalidGraph(format!("canonicalize graph: {error}")))?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(encoded))))
}

/// Traverse accepted explicit relations up to the reviewed bounds.
/// Traverse using the status set declared by the reviewed contract.
pub fn expand_with_statuses(
    graph: &Graph,
    seeds: &[String],
    request: &EvidenceRequest,
    max_hops: usize,
    max_nodes: usize,
    allowed_statuses: Option<&BTreeSet<DecisionStatus>>,
) -> Result<Expansion, EvidenceError> {
    let nodes: BTreeMap<&str, &GraphNode> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let by_path: BTreeMap<&str, &str> = graph
        .nodes
        .iter()
        .map(|node| (node.source_path.as_str(), node.id.as_str()))
        .collect();
    let mut adjacency: BTreeMap<&str, Vec<(&str, &GraphEdge)>> = BTreeMap::new();
    for edge in &graph.edges {
        if matches!(edge.relation_type.as_str(), "documents" | "references") {
            continue;
        }
        adjacency
            .entry(&edge.from_id)
            .or_default()
            .push((&edge.to_id, edge));
        adjacency
            .entry(&edge.to_id)
            .or_default()
            .push((&edge.from_id, edge));
    }
    for values in adjacency.values_mut() {
        values.sort_by_key(|(id, edge)| (*id, edge.edge_id.as_str()));
    }
    let allowed_nodes: BTreeMap<&str, bool> = graph
        .nodes
        .iter()
        .map(|node| {
            decision_node_allowed(node, request.decision_scope, allowed_statuses)
                .map(|allowed| (node.id.as_str(), allowed))
        })
        .collect::<Result<_, _>>()?;
    let mut queue = VecDeque::new();
    for (rank, seed) in seeds.iter().enumerate() {
        if let Some(id) = by_path.get(seed.as_str()) {
            queue.push_back((rank, 0_usize, *id));
        }
    }
    let ceiling = Sensitivity::parse(&request.sensitivity_ceiling)?;
    let mut visited = BTreeSet::new();
    let mut paths = Vec::new();
    let mut edges = BTreeMap::new();
    let mut limited = false;
    while let Some((_rank, depth, node_id)) = queue.pop_front() {
        if !visited.insert(node_id.to_owned()) {
            continue;
        }
        if visited.len() > max_nodes {
            limited = true;
            break;
        }
        let Some(node) = nodes.get(node_id) else {
            continue;
        };
        if node.namespace != request.namespace
            || Sensitivity::parse(&node.sensitivity)? > ceiling
            || node.sensitivity == "restricted"
            || !allowed_nodes.get(node_id).copied().unwrap_or(false)
        {
            continue;
        }
        paths.push(node.source_path.clone());
        if depth >= max_hops {
            continue;
        }
        let neighbors = adjacency.get(node_id).cloned().unwrap_or_default();
        if neighbors.len() > 20 {
            limited = true;
        }
        for (next, edge) in neighbors.into_iter().take(20) {
            if let Some(next_node) = nodes.get(next) {
                if allowed_nodes
                    .get(next_node.id.as_str())
                    .copied()
                    .unwrap_or(false)
                {
                    edges
                        .entry(edge.edge_id.clone())
                        .or_insert_with(|| edge.clone());
                    queue.push_back((0, depth + 1, next));
                }
            }
        }
    }
    Ok(Expansion {
        paths,
        edges: edges.into_values().collect(),
        limited,
    })
}

/// Validate lifecycle metadata copied into one derived Decision node.
fn validate_decision_node(node: &GraphNode) -> Result<(), EvidenceError> {
    decision_metadata(node)?;
    Ok(())
}

/// Check one graph node against the request and contract lifecycle scope.
fn decision_node_allowed(
    node: &GraphNode,
    scope: DecisionScope,
    allowed_statuses: Option<&BTreeSet<DecisionStatus>>,
) -> Result<bool, EvidenceError> {
    let Some(metadata) = decision_metadata(node)? else {
        return Ok(true);
    };
    Ok(allowed_statuses.map_or_else(
        || scope.allows(metadata.decision_status),
        |statuses| statuses.contains(&metadata.decision_status),
    ))
}

/// Normalize one graph node's copied Decision metadata once.
fn decision_metadata(node: &GraphNode) -> Result<Option<DecisionMetadata>, EvidenceError> {
    if node.node_type != "Decision" {
        return Ok(None);
    }
    let frontmatter = node
        .extra
        .iter()
        .map(|(key, value)| {
            serde_yaml::to_value(value)
                .map(|value| (key.clone(), value))
                .map_err(|error| {
                    EvidenceError::InvalidGraph(format!(
                        "invalid Decision metadata {}: {error}",
                        node.source_path
                    ))
                })
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let metadata = normalize_decision_metadata_for_graph(&frontmatter).map_err(|error| {
        EvidenceError::InvalidGraph(format!(
            "invalid Decision metadata {}: {error}",
            node.source_path
        ))
    })?;
    Ok(Some(metadata))
}

/// Return a validated graph Decision status for lexical seeding.
pub(crate) fn decision_node_status(
    node: &GraphNode,
) -> Result<Option<DecisionStatus>, EvidenceError> {
    Ok(decision_metadata(node)?.map(|metadata| metadata.decision_status))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, path: &str, sensitivity: &str) -> GraphNode {
        GraphNode {
            id: id.into(),
            node_type: "Decision".into(),
            title: id.into(),
            source_path: path.into(),
            source_hash: "sha256:test".into(),
            namespace: "test".into(),
            sensitivity: sensitivity.into(),
            extra: BTreeMap::new(),
        }
    }

    fn edge(id: &str, from: &str, to: &str) -> GraphEdge {
        GraphEdge {
            edge_id: id.into(),
            from_id: from.into(),
            to_id: to.into(),
            relation_type: "derived_from".into(),
            source_path: "docs/A.md".into(),
            source_hash: "sha256:test".into(),
            origin: "explicit".into(),
            review_status: "accepted".into(),
            extra: BTreeMap::new(),
        }
    }

    fn candidate_node(id: &str, path: &str) -> GraphNode {
        let mut value = node(id, path, "internal");
        value
            .extra
            .insert("decision_status".into(), Value::String("candidate".into()));
        value
            .extra
            .insert("recorded_by".into(), Value::String("ai".into()));
        value
            .extra
            .insert("source_refs".into(), serde_json::json!(["docs/source.md"]));
        value
    }

    #[test]
    fn rejects_warning_bearing_graph_before_traversal() {
        let graph = Graph {
            schema_version: "okf-derived-graph.v2".into(),
            node_count: 0,
            edge_count: 0,
            warning_count: 1,
            warnings: vec![Value::String("stale".into())],
            normalized_graph_hash: "sha256:nope".into(),
            nodes: vec![],
            edges: vec![],
            extra: BTreeMap::new(),
        };
        assert!(matches!(
            validate_graph(Path::new("."), &graph, "okf-derived-graph.v2"),
            Err(EvidenceError::InvalidGraph(_))
        ));
    }

    #[test]
    fn expansion_is_cycle_safe_two_hops_and_enforces_sensitivity() {
        let graph = Graph {
            schema_version: "okf-derived-graph.v2".into(),
            node_count: 4,
            edge_count: 4,
            warning_count: 0,
            warnings: vec![],
            normalized_graph_hash: String::new(),
            nodes: vec![
                node("a", "docs/A.md", "internal"),
                node("b", "docs/B.md", "internal"),
                node("c", "docs/C.md", "internal"),
                node("private", "docs/Private.md", "private"),
            ],
            edges: vec![
                edge("ab", "a", "b"),
                edge("bc", "b", "c"),
                edge("ca", "c", "a"),
                edge("bp", "b", "private"),
            ],
            extra: BTreeMap::new(),
        };
        let request = EvidenceRequest {
            kind: crate::EvidenceKind::Decision,
            query: "test".into(),
            namespace: "test".into(),
            sensitivity_ceiling: "internal".into(),
            max_sources: 6,
            timeout_ms: 1_000,
            decision_scope: crate::DecisionScope::Current,
            mode: crate::EvidenceMode::Hybrid,
        };
        let result =
            expand_with_statuses(&graph, &["docs/A.md".into()], &request, 2, 100, None).unwrap();
        assert_eq!(result.paths, ["docs/A.md", "docs/B.md", "docs/C.md"]);
        assert!(!result.paths.contains(&"docs/Private.md".into()));
        assert!(result.edges.len() <= 4);
    }

    #[test]
    fn expansion_skips_structural_and_generic_reference_edges() {
        let mut documents = edge("documents", "a", "b");
        documents.relation_type = "documents".into();
        let mut references = edge("references", "a", "c");
        references.relation_type = "references".into();
        let graph = Graph {
            schema_version: "okf-derived-graph.v2".into(),
            node_count: 3,
            edge_count: 2,
            warning_count: 0,
            warnings: vec![],
            normalized_graph_hash: String::new(),
            nodes: vec![
                node("a", "docs/A.md", "internal"),
                node("b", "docs/B.md", "internal"),
                node("c", "docs/C.md", "internal"),
            ],
            edges: vec![documents, references],
            extra: BTreeMap::new(),
        };
        let request = EvidenceRequest {
            kind: crate::EvidenceKind::Decision,
            query: "test".into(),
            namespace: "test".into(),
            sensitivity_ceiling: "internal".into(),
            max_sources: 6,
            timeout_ms: 1_000,
            decision_scope: crate::DecisionScope::Current,
            mode: crate::EvidenceMode::Hybrid,
        };

        let result =
            expand_with_statuses(&graph, &["docs/A.md".into()], &request, 2, 100, None).unwrap();

        assert_eq!(result.paths, ["docs/A.md"]);
        assert!(result.edges.is_empty());
    }

    #[test]
    fn current_expansion_skips_candidate_decision_nodes_and_edges() {
        let graph = Graph {
            schema_version: "okf-derived-graph.v2".into(),
            node_count: 2,
            edge_count: 1,
            warning_count: 0,
            warnings: vec![],
            normalized_graph_hash: String::new(),
            nodes: vec![
                node("a", "docs/A.md", "internal"),
                candidate_node("b", "docs/B.md"),
            ],
            edges: vec![edge("ab", "a", "b")],
            extra: BTreeMap::new(),
        };
        let request = EvidenceRequest {
            kind: crate::EvidenceKind::Decision,
            query: "test".into(),
            namespace: "test".into(),
            sensitivity_ceiling: "internal".into(),
            max_sources: 6,
            timeout_ms: 1_000,
            decision_scope: crate::DecisionScope::Current,
            mode: crate::EvidenceMode::Hybrid,
        };

        let result =
            expand_with_statuses(&graph, &["docs/A.md".into()], &request, 2, 100, None).unwrap();

        assert_eq!(result.paths, ["docs/A.md"]);
        assert!(result.edges.is_empty());
    }

    #[test]
    fn graph_rejects_malformed_decision_lifecycle_metadata() {
        let mut malformed = node("bad", "docs/Bad.md", "internal");
        malformed
            .extra
            .insert("decision_status".into(), Value::String("unknown".into()));

        let error = validate_decision_node(&malformed).unwrap_err();

        assert!(
            matches!(error, EvidenceError::InvalidGraph(message) if message.contains("Bad.md"))
        );
    }

    #[test]
    fn graph_rejects_self_supersedes_edge() {
        let source = node("bad", "docs/Bad.md", "internal");
        let nodes = BTreeMap::from([("bad", &source)]);
        let mut edge = edge("self", "bad", "bad");
        edge.relation_type = "supersedes".into();

        let error = validate_supersedes_edge(&edge, &nodes).unwrap_err();

        assert!(
            matches!(error, EvidenceError::InvalidGraph(message) if message.contains("distinct"))
        );
    }
}
