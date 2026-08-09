use agentmesh_evidence::{
    compile, evaluate, health, load_contract, CommandSpec, CompileOptions, EvidenceKind,
    EvidenceMode, EvidenceRequest,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

fn write_contract(root: &Path) -> agentmesh_evidence::Contract {
    let fixtures = (1..=20)
        .map(|index| format!("Q{index:02}"))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        r#"# Contract
```yaml
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
  restricted_path_fragments: [.env, credential, secret, token]
  evaluation:
    source_contract: docs/eval.md
    expected_query_count: 20
    fixture_ids: [{fixtures}]
    agent_run_fixture_ids: [Q18, Q19, Q20]
    default_namespace: test
    default_sensitivity_ceiling: internal
```
"#
    );
    let path = root.join("contract.md");
    fs::write(&path, body).unwrap();
    load_contract(&path).unwrap()
}

fn write_decision(root: &Path, name: &str, decision: &str) {
    fs::write(
        root.join("docs").join(name),
        format!("---\nstatus: adopted\n---\n# {name}\n## Decision\n{decision}\n## Rationale\nSource linked.\n## Alternatives\nNone.\n"),
    )
    .unwrap();
}

fn write_agent_run(root: &Path, name: &str) {
    fs::write(
        root.join("docs").join(name),
        format!("---\nsource_issue: SYNTH-1\nrun_id: run-{name}\nartifact: evidence.json\noutcome: success\n---\n# {name}\n## Evidence\nSynthetic source-linked run evidence.\n"),
    )
    .unwrap();
}

fn write_evaluation(root: &Path) {
    let mut body = String::from("# Evaluation\n```yaml\nevaluation_queries:\n");
    for index in 1..=20 {
        body.push_str(&format!(
            "  - id: Q{index:02}\n    category: Decision\n    query_ja: fixture {index}\n    expected_evidence_paths: [docs/Keyword.md]\n    qmd_top_results: []\n"
        ));
    }
    body.push_str("```\n");
    fs::write(root.join("docs/eval.md"), body).unwrap();
}

fn write_promotion_evaluation(root: &Path) {
    let mut body = String::from("# Evaluation\n```yaml\nevaluation_queries:\n");
    for index in 1..=20 {
        let expected = match index {
            1..=6 => "[docs/Standalone.md]",
            7..=17 => "[docs/Keyword.md, docs/Related.md]",
            18 => "[docs/Related2.md]",
            _ => "[docs/Missing.md]",
        };
        body.push_str(&format!(
            "  - id: Q{index:02}\n    category: Decision\n    query_ja: fixture {index}\n    expected_evidence_paths: {expected}\n    qmd_top_results: []\n"
        ));
    }
    body.push_str("```\n");
    fs::write(root.join("docs/eval.md"), body).unwrap();
}

fn source_hash(path: &Path) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(fs::read(path).unwrap()))
    )
}

fn graph_node(root: &Path, id: &str, source_path: &str) -> Value {
    json!({
        "id": id,
        "type": "Decision",
        "title": id,
        "source_path": source_path,
        "source_hash": source_hash(&root.join(source_path)),
        "namespace": "test",
        "sensitivity": "internal"
    })
}

fn write_promotion_graph(root: &Path) -> PathBuf {
    let keyword_hash = source_hash(&root.join("docs/Keyword.md"));
    let seed2_hash = source_hash(&root.join("docs/Seed2.md"));
    let mut graph = json!({
        "schema_version": "okf-derived-graph.v2",
        "node_count": 4,
        "edge_count": 2,
        "warning_count": 0,
        "warnings": [],
        "normalized_graph_hash": "",
        "nodes": [
            graph_node(root, "keyword", "docs/Keyword.md"),
            graph_node(root, "related", "docs/Related.md"),
            graph_node(root, "related2", "docs/Related2.md"),
            graph_node(root, "seed2", "docs/Seed2.md")
        ],
        "edges": [
            {"edge_id":"keyword-related","from_id":"keyword","to_id":"related","relation_type":"derived_from","source_path":"docs/Keyword.md","source_hash":keyword_hash,"origin":"explicit","review_status":"accepted"},
            {"edge_id":"seed2-related2","from_id":"seed2","to_id":"related2","relation_type":"derived_from","source_path":"docs/Seed2.md","source_hash":seed2_hash,"origin":"explicit","review_status":"accepted"}
        ]
    });
    let mut normalized = graph.clone();
    normalized
        .as_object_mut()
        .unwrap()
        .remove("normalized_graph_hash");
    graph["normalized_graph_hash"] = Value::String(format!(
        "sha256:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&normalized).unwrap()))
    ));
    let path = PathBuf::from("graph.json");
    fs::write(root.join(&path), serde_json::to_vec_pretty(&graph).unwrap()).unwrap();
    path
}

#[test]
fn compile_fuses_keyword_semantic_and_read_only_adaptive_paths_without_query_leak() {
    let Ok(node) = which::which("node") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("docs")).unwrap();
    write_decision(root, "Keyword.md", "Use keyword evidence.");
    write_decision(root, "Semantic.md", "Use semantic evidence.");
    write_decision(root, "Adaptive.md", "Use adaptive evidence.");
    let contract = write_contract(root);
    let qmd_script = root.join("fake-qmd.js");
    fs::write(
        &qmd_script,
        r#"
const operation = process.argv[2];
const file = operation === 'search' ? 'docs/Keyword.md' : 'docs/Semantic.md';
console.log(JSON.stringify([{file}]));
"#,
    )
    .unwrap();
    let adaptive_script = root.join("fake-adaptive.js");
    fs::write(&adaptive_script, r#"
if (process.argv.includes('--help')) { console.log('qmd-adaptive-search 1.3.0'); process.exit(0); }
if (!process.argv.includes('--read-only')) process.exit(9);
console.log(JSON.stringify({readOnly:true,results:[{path:'docs/Adaptive.md',why:['learned boost']}]}));
"#).unwrap();
    let qmd = CommandSpec {
        program: node.clone(),
        prefix_args: vec![qmd_script.into_os_string()],
    };
    let adaptive = CommandSpec {
        program: node,
        prefix_args: vec![adaptive_script.into_os_string()],
    };
    let query = "raw query privacy sentinel";
    let packet = compile(
        root,
        &contract,
        &EvidenceRequest {
            kind: EvidenceKind::Decision,
            query: query.into(),
            namespace: "test".into(),
            sensitivity_ceiling: "internal".into(),
            max_sources: 6,
            timeout_ms: 5_000,
            mode: EvidenceMode::QmdOnly,
        },
        &CompileOptions {
            collection: "test".into(),
            qmd: Some(qmd),
            adaptive: Some(adaptive),
            graph_path: None,
        },
    )
    .unwrap();
    let paths: Vec<_> = packet["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["source_path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"docs/Keyword.md"));
    assert!(paths.contains(&"docs/Semantic.md"));
    assert!(paths.contains(&"docs/Adaptive.md"));
    assert_eq!(packet["trace"]["streams"].as_array().unwrap().len(), 3);
    assert_eq!(packet["health"]["raw_query_persisted"], false);
    assert!(!serde_json::to_string(&packet).unwrap().contains(query));
    assert!(!root.join(".qmd-adaptive-search").exists());
}

#[test]
fn evaluation_persists_fixture_ids_and_metrics_but_not_queries() {
    let Ok(node) = which::which("node") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("docs")).unwrap();
    write_decision(root, "Keyword.md", "Use keyword evidence.");
    write_decision(root, "Adaptive.md", "Use adaptive evidence.");
    write_evaluation(root);
    let contract = write_contract(root);
    let qmd_script = root.join("eval-qmd.js");
    fs::write(
        &qmd_script,
        "console.log(JSON.stringify([{file:'docs/Keyword.md'}]));\n",
    )
    .unwrap();
    let adaptive_script = root.join("eval-adaptive.js");
    fs::write(
        &adaptive_script,
        "if(process.argv.includes('--help')) console.log('qmd-adaptive-search 1.3.0'); else console.log(JSON.stringify({readOnly:true,results:[{path:'docs/Adaptive.md'}]}));\n",
    )
    .unwrap();
    let report = evaluate(
        root,
        &contract,
        &CompileOptions {
            collection: "test".into(),
            qmd: Some(CommandSpec {
                program: node.clone(),
                prefix_args: vec![qmd_script.into_os_string()],
            }),
            adaptive: Some(CommandSpec {
                program: node,
                prefix_args: vec![adaptive_script.into_os_string()],
            }),
            graph_path: None,
        },
        1,
    )
    .unwrap();
    assert_eq!(report["fixture_count"], 20);
    assert_eq!(report["query_text_persisted"], false);
    assert_eq!(report["qmd_only"]["expected_hit_queries"], 20);
    assert_eq!(report["hybrid"]["complete_queries"], 20);
    assert_eq!(report["incremental_complete"], 0);
    assert_eq!(report["promotion"]["pass"], false);
    assert_eq!(report["promotion"]["graph_default_enabled"], false);
    let encoded = serde_json::to_string(&report).unwrap();
    assert!(!encoded.contains("fixture 1"));
}

#[test]
fn realistic_graph_increment_can_pass_promotion_without_enabling_the_default() {
    let Ok(node) = which::which("node") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("docs")).unwrap();
    for name in ["Standalone.md", "Keyword.md", "Related.md", "Filler.md"] {
        write_decision(root, name, &format!("Use {name} evidence."));
    }
    write_agent_run(root, "Seed2.md");
    write_agent_run(root, "Related2.md");
    write_promotion_evaluation(root);
    let contract = write_contract(root);
    let graph_path = write_promotion_graph(root);
    let graph_health = health(root, &contract, Some(&graph_path));
    assert_eq!(graph_health["status"], "ready", "{graph_health:#}");
    let qmd_script = root.join("promotion-qmd.js");
    fs::write(
        &qmd_script,
        r#"
const match = process.argv.join(' ').match(/fixture (\d+)/);
const fixture = match ? Number(match[1]) : 0;
let results = [];
if (fixture >= 1 && fixture <= 6) results = [{file:'docs/Standalone.md'},{file:'docs/Filler.md'}];
else if (fixture >= 7 && fixture <= 17) results = [{file:'docs/Keyword.md'},{file:'docs/Filler.md'}];
else if (fixture === 18) results = [{file:'docs/Seed2.md'},{file:'docs/Filler.md'}];
console.log(JSON.stringify(results));
"#,
    )
    .unwrap();
    let adaptive_script = root.join("promotion-adaptive.js");
    fs::write(
        &adaptive_script,
        "if(process.argv.includes('--help')) console.log('qmd-adaptive-search 1.3.0'); else console.log(JSON.stringify({readOnly:true,results:[]}));\n",
    )
    .unwrap();
    let report = evaluate(
        root,
        &contract,
        &CompileOptions {
            collection: "test".into(),
            qmd: Some(CommandSpec {
                program: node.clone(),
                prefix_args: vec![qmd_script.into_os_string()],
            }),
            adaptive: Some(CommandSpec {
                program: node,
                prefix_args: vec![adaptive_script.into_os_string()],
            }),
            graph_path: Some(graph_path),
        },
        2,
    )
    .unwrap();
    assert_eq!(report["direct_qmd"]["expected_hit_queries"], 17);
    assert_eq!(report["direct_qmd"]["complete_queries"], 6);
    assert_eq!(report["hybrid"]["expected_hit_queries"], 18);
    assert_eq!(report["hybrid"]["complete_queries"], 18);
    assert_eq!(report["incremental_complete"], 12);
    assert_eq!(report["promotion"]["pass"], true);
    assert_eq!(report["promotion"]["graph_default_enabled"], false);
}

#[test]
fn old_adaptive_binary_is_rejected_before_it_receives_query() {
    let Ok(node) = which::which("node") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("docs")).unwrap();
    write_decision(root, "Keyword.md", "Use keyword evidence.");
    let contract = write_contract(root);
    let qmd_script = root.join("old-qmd.js");
    fs::write(
        &qmd_script,
        "console.log(JSON.stringify([{file:'docs/Keyword.md'}]));\n",
    )
    .unwrap();
    let adaptive_script = root.join("old-adaptive.js");
    fs::write(
        &adaptive_script,
        r#"
const fs = require('fs'); const path = require('path');
if (process.argv.includes('--help')) { console.log('qmd-adaptive-search 1.2.3'); process.exit(0); }
fs.writeFileSync(path.join(__dirname, 'query-ran'), 'unsafe');
console.log(JSON.stringify({results:[]}));
"#,
    )
    .unwrap();
    let packet = compile(
        root,
        &contract,
        &EvidenceRequest {
            kind: EvidenceKind::Decision,
            query: "must stay ephemeral".into(),
            namespace: "test".into(),
            sensitivity_ceiling: "internal".into(),
            max_sources: 3,
            timeout_ms: 5_000,
            mode: EvidenceMode::QmdOnly,
        },
        &CompileOptions {
            collection: "test".into(),
            qmd: Some(CommandSpec {
                program: node.clone(),
                prefix_args: vec![qmd_script.into_os_string()],
            }),
            adaptive: Some(CommandSpec {
                program: node,
                prefix_args: vec![adaptive_script.into_os_string()],
            }),
            graph_path: None,
        },
    )
    .unwrap();
    assert!(!root.join("query-ran").exists());
    let adaptive = packet["trace"]["streams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["stream"] == "qmd_adaptive")
        .unwrap();
    assert_eq!(adaptive["error"], "qmd_protocol_error");
}
