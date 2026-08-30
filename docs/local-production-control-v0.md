# Local production control v0 (foundation)

## Purpose

Bounded local production adapter for deterministic Multica controllers. One-shot Apps run on an existing Windows PC under Task Scheduler. No new cloud service.

## Components

| Component | Crate / App | Role |
|---|---|---|
| Pinned Multica CLI adapter | `agentmesh-multica-cli-adapter` | Absolute-path CLI spawn, bounded JSON stdout, synthetic contract tests |
| Local control ledger | `agentmesh-local-control-ledger` | App-local SQLite for leases, claims, watermarks, authority mode, decision/rollback metadata |
| Observer wiring | `agentmesh-production-controller-observer` | One-shot observer run: lease → read-only CLI → record decision → release |
| Production authority | `agentmesh-production-authority` | Authority modes through `todo_runner`, promotion gates, allowed CLI argv, Cursor recovery |
| Evaluation report | `agentmesh-production-evaluation-report` | 7-day rollback gate and 30-day result from compact aggregate inputs |

## Authority modes

Promotion ladder (external thresholds apply before mode changes):

| Mode | Minimum shadow window | Notes |
|---|---|---|
| `observer` | 3 days / 20 decisions | read-only audit surfaces |
| `safe_writer` | 7 days / 50 decisions | bounded writes, unauthorized write 0 |
| `queue` | 7 days / 50 decisions | Backlog Promoter authority |
| `todo_runner` | 14 days / 100 decisions | assign/rerun authority |

Foundation slice accepts `authority_mode: observer` only. The **authority slice** adds `production-authority` and `production-evaluation-report` Apps with promotion gates, allowed Multica CLI argv mapping, health-gated Cursor recovery, and synthetic/fake contract tests only.

## `production-authority` execution kind

`production-authority` `run_once` inputs require `execution_kind`:

- `shadow`: predecessor gate + allowed-operation validation, no Multica CLI process; records `{authority_mode}_shadow_run_once` decision evidence only; `mutation_performed: false`. Promotion consumes this shadow evidence.
- `live`: stored authority must match requested `authority_mode` and existing Multica CLI invocation/mutation applies; wrong-mode attempts record `unauthorized_write`.

## Deterministic exit reasons

Observer `run_once` emits stable `exit_reason` values including:

- `observer_success_no_mutation`
- `authority_not_observer`
- `lease_already_held`
- `duplicate_suppressed`
- `idempotency_claim_failed`
- `cli_path_not_absolute` / `cli_nonzero_exit` / `stdout_not_json` (via CLI adapter)
- `decision_record_failed`

## Idempotency and manual recovery

Observer and authority `run_once` paths claim controller-scoped idempotency **before** invoking the Multica CLI. Duplicate inputs suppress the CLI call and release leases.

Failed **non-Cursor** mutation runs that already claimed idempotency but exited with CLI failure, timeout, or uncertain Multica effect leave a **consumed ambiguous claim**. Operators must inspect ledger decisions and live Multica state and perform **explicit manual recovery**. There is no generic automatic retry for those mutations.

Cursor recovery is separate: one health-gated retry per issue via `cursor_recovery`, with lease + scope + idempotency ordering before rerun.

## Local control ledger exclusions

The ledger stores identifiers, hashes, timestamps, and bounded result codes. It never stores:

- prompts or comments
- task output bodies
- Multica auth tokens or other secrets

## Task Scheduler

Scripts under `scripts/task-scheduler/`:

- `install-production-controller.ps1` — register one-shot `agentmesh app run` task
- `uninstall-production-controller.ps1` — remove task
- `rollback-production-controller.ps1` — disable task, record rollback in ledger when `data.recorded=true`

Operators install/configure tasks manually; CI does not activate schedules.

## Development smoke

```bash
cargo build -p agentmesh-production-controller-observer
cargo test -p agentmesh-multica-cli-adapter
cargo test -p agentmesh-local-control-ledger
cargo test -p agentmesh-production-controller-observer
```

App validate:

```bash
agentmesh app validate \
  --manifest apps/production-controller-observer/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
```

## Related

- `docs/adr/0003-local-production-control.md`
- Multica strategy ADR-0009 (accepted in vault)
