# Evidence Compiler v0

Status: **conditional private pilot**

## Purpose

`agentmesh evidence` owns the local runtime that turns QMD candidates and a
reviewed explicit graph into source-linked Decision or AgentRun evidence.
Canonical notes, policy, fixtures, and graph inputs remain outside AgentMesh and
are supplied at runtime. The compiler never writes them.

For `Decision` packets, AgentMesh normalizes Any Decision Record frontmatter
without changing the canonical note. Explicit AI records can be
`decision_status: adopted` immediately; ambiguous records remain
`candidate`. The default `current` scope serves adopted records only. Use
`--decision-scope review` for candidate/deferred review work or
`--decision-scope historical` when inspecting the full lifecycle. Human review
status is reported separately and is not an adoption gate.

```text
ephemeral query file
        |
        +--> qmd search ---------------- keyword/BM25 --------+
        +--> qmd query ----------------- semantic/hybrid -----+--> RRF v1
        +--> qmd-adaptive --read-only -- aliases/boosts ------+
                                                               |
reviewed namespace + sensitivity policy -----------------------+--> filter
validated okf-derived-graph.v2 --------------------------------+--> 2-hop BFS
canonical Markdown --------------------------------------------+--> reread/hash
                                                               |
                                                               +--> Evidence Packet JSON
```

Fusion uses versioned reciprocal-rank scoring plus a small kind-specific
canonical-authority prior. Every contributing stream/rank remains visible in the
bounded packet trace; no opaque QMD/adaptive score becomes semantic authority.

## Runtime dependencies

- `qmd` for direct keyword and semantic/hybrid streams.
- `qmd-adaptive-search >= 1.3.0` for side-effect-free adaptive discovery.
- A canonical Markdown contract containing `okf-evidence-contract.v2`.
- Optional warning-free `okf-derived-graph.v2` JSON.

AgentMesh preflights the adaptive version without sending the query, then
requires `"readOnly": true` in the search response. Older commands fail closed
before query execution and cannot mutate or contribute candidates. Capability
was introduced by `pi-qmd-adaptive-search` PR #51.

## Compile

The query file must be inside `--root` and is caller-owned. AgentMesh reads it,
does not copy it into output, and does not delete it.

```bash
agentmesh evidence compile \
  --root C:/vault/obsidian-note \
  --contract 4_Project/Multica-Agent-Strategy/Research/okf-evidence-compiler-contract-v2.md \
  --query-file .scratch/evidence-query.txt \
  --kind Decision \
  --namespace lane:multica-agent-strategy \
  --decision-scope current \
  --mode hybrid \
  --graph 4_Project/Multica-Agent-Strategy/Data/okf-evidence-derived-graph-v2.json
```

Use `--adaptive-command <path-to-bin/qmd-adaptive-search.js>` before version
1.3.0 is installed globally. Use `--no-adaptive` only as an explicit degraded
mode; keyword and semantic streams still run.

Modes:

- `direct-qmd`: historical `qmd search` baseline only.
- `qmd-only`: keyword + semantic + read-only adaptive fusion.
- `hybrid`: fused QMD candidates plus explicit graph traversal.
- `graph-only`: graph nodes seeded from lexical title/path matches.

Decision scopes:

- `current`: `adopted` only (default).
- `review`: `candidate` and `deferred` only.
- `historical`: all lifecycle states, including `rejected` and `superseded`.

Every Decision evidence item carries normalized `record_status`
(`decision_status` is retained as a deprecated compatibility alias),
`decision_kind`, `recorded_by`, `review_status`, `adoption_mode`, `impact`,
`source_refs`, and `supersedes` fields. Malformed AI records without
`source_refs` are rejected with `invalid_decision_record`; the runtime never
promotes candidates or writes a correction back to Markdown.

## Health

```bash
agentmesh evidence health \
  --root C:/vault/obsidian-note \
  --contract 4_Project/Multica-Agent-Strategy/Research/okf-evidence-compiler-contract-v2.md \
  --graph 4_Project/Multica-Agent-Strategy/Data/okf-evidence-derived-graph-v2.json
```

`status=ready` means the contract and graph can be served. `fallback` means use
QMD fusion without graph traversal and rebuild the graph outside the request.

## Evaluate

```bash
agentmesh evidence evaluate \
  --root C:/vault/obsidian-note \
  --contract 4_Project/Multica-Agent-Strategy/Research/okf-evidence-compiler-contract-v2.md \
  --graph 4_Project/Multica-Agent-Strategy/Data/okf-evidence-derived-graph-v2.json \
  --repeat 3 > evaluation.json
```

Evaluation reports `direct_qmd`, `qmd_fused`, and `hybrid` separately so the
historical direct baseline and graph incremental value remain comparable. Output
stores fixture IDs, metrics, hits, and source paths, not query text, query hashes,
or embeddings.

## Safety invariants

- Fixed argv; no shell command construction.
- Shared monotonic deadline across all three discovery streams.
- Bounded stdout/stderr and process-tree termination.
- Path traversal, absolute source paths, symlink escapes, non-files, restricted
  fragments, cross-namespace paths, and over-ceiling graph nodes are rejected.
- Graph schema, counts, warnings, normalized hash, source hashes, accepted
  explicit edges, hop count, fanout, and visited-node count are checked.
- Graph relation IDs are mapped to emitted Evidence IDs.
- Packet excerpts shrink to the contract byte cap; citation metadata is retained.
- No SQLite, daemon, TUI, canonical write-back, feedback write, or query telemetry.

## Rollback

Stop invoking `agentmesh evidence`, use direct QMD, and delete derived JSON graph
snapshots if needed. Canonical Markdown is unchanged. Caller-owned query and
packet files are never removed by rollback.
