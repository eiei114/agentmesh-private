# ADR 0003: Local production control boundary

## Status

Accepted (foundation slice)

## Context

ADR-0009 (Multica strategy vault) adopts AgentMesh as the local deterministic Multica control plane. Phase 0/1.0 repository boundaries explicitly deferred scheduler integration, SQLite, and live Multica mutation. Production rollout v2 requires a bounded local adapter while keeping `agentmesh-proto` / `agentmesh-host` tool-neutral.

## Decision

1. **Pinned Multica CLI adapter** (`agentmesh-multica-cli-adapter`): plugin-owned absolute-path CLI invocation with shell-free process abstraction and synthetic contract tests. No direct HTTP, no token persistence in AgentMesh state.
2. **Local control ledger** (`agentmesh-local-control-ledger`): app-local SQLite storing schedule leases, scope claims, watermarks, authority mode, decision hashes, and rollback correlation only. Explicitly excludes prompts, comments, full outputs, and secrets.
3. **Observer one-shot wiring** (`agentmesh-production-controller-observer`): combines adapter + ledger for read-only observer runs with deterministic exit reasons and `mutation_performed: false`.
4. **Windows Task Scheduler scripts** under `scripts/task-scheduler/` generate install/uninstall/rollback commands; operators run them manually during promotion.
5. **Authority ladder** reserved for later slices: `shadow` → `observer` → `safe_writer` → `queue` → `todo_runner`. Foundation implements observer only; higher modes reject mutation until their slice lands.

## Consequences

- `AGENTS.md` and `README.md` revise the shadow-only production deferral to describe the bounded local adapter phase.
- Multica domain types remain plugin-owned; core protocol crates unchanged.
- Live Multica mutation, Todo Runner authority, and Task Scheduler activation stay out of foundation tests except via fakes.
- Coordinator lane owns workspace membership for new plugin crates.

## Rollback

Disable Task Scheduler job via `scripts/task-scheduler/rollback-production-controller.ps1`, record rollback event in the local control ledger, resume the paused legacy Multica Autopilot for the affected controller surface.
