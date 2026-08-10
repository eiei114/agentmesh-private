# ADR 0002: AgentMesh owns Evidence Compiler runtime

Status: Accepted  
Date: 2026-08-09

## Context

The first Evidence Compiler implementation lived beside one Obsidian vault as
Python. That proved packet, graph, and evaluation contracts, but made the CLI a
vault-owned script and used only one QMD result stream. Operators need one
tool-neutral private runtime that can fuse direct keyword, semantic, and adaptive
discovery while leaving Obsidian as canonical content owner.

## Decision

AgentMesh owns the reusable Rust core and `agentmesh evidence` CLI. Runtime
contracts, canonical notes, evaluation fixtures, and generated graph snapshots
remain caller-owned inputs. QMD remains an external discovery dependency.

All three QMD streams run concurrently and contribute path-only candidates:

1. `qmd search`
2. `qmd query`
3. `qmd-adaptive-search search --read-only`

AgentMesh applies reviewed namespace/sensitivity policy, validates optional JSON
graph state, rereads canonical sources, and emits ephemeral packet JSON. It does
not own QMD learning, Obsidian Markdown, or graph generation.

For Decision packets, the Rust core also normalizes Any Decision Record
frontmatter. `current` retrieval serves adopted records by default;
`review` and `historical` are explicit diagnostic scopes. `review_status` is
reported separately from `decision_status`, so an unreviewed explicit AI
decision is usable without making human review a runtime gate. Candidate and
non-adopted nodes never displace current adopted candidates in graph expansion.

## Consequences

- AgentMesh becomes the install/distribution boundary for the CLI and tests.
- Vault Python remains a migration reference, not runtime authority.
- `qmd-adaptive-search >= 1.3.0` is required for adaptive participation; older
  versions fail closed because they do not confirm side-effect-free execution.
- SQLite cache from the Python pilot is intentionally dropped because AgentMesh
  Phase 0/1.0 forbids SQLite and the portable JSON graph already satisfies the
  pilot latency target.
- The migration is reversible: direct QMD and canonical Markdown continue to
  work if `agentmesh evidence` is disabled.

## Rejected alternatives

- Keep core in the vault and add a thin AgentMesh wrapper: preserves repo drift
  and fails the runtime-ownership goal.
- Duplicate Python and Rust implementations indefinitely: creates two packet
  authorities and untestable semantic drift.
- Put evidence types into `agentmesh-proto` or host lifecycle: expands the stable
  plugin protocol with a vault/search concern.
- Add a new daemon, MCP tool, or SQLite service: increases operational state and
  violates current project boundaries.
