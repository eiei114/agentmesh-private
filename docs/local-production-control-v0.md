# Local production control v0 (foundation)

## Purpose

Bounded local production adapter for deterministic Multica controllers. One-shot Apps run on an existing Windows PC under Task Scheduler. No new cloud service.

## Components

| Component | Crate / App | Role |
|---|---|---|
| Pinned Multica CLI adapter | `agentmesh-multica-cli-adapter` | Absolute-path CLI spawn, bounded JSON stdout, synthetic contract tests |
| Local control ledger | `agentmesh-local-control-ledger` | App-local SQLite for leases, claims, watermarks, authority mode, decision/rollback metadata |
| Observer wiring | `agentmesh-production-controller-observer` | One-shot observer run: lease → read-only CLI → record decision → release |

## Authority modes

Promotion ladder (external thresholds apply before mode changes):

| Mode | Minimum shadow window | Notes |
|---|---|---|
| `observer` | 3 days / 20 decisions | read-only audit surfaces |
| `safe_writer` | 7 days / 50 decisions | bounded writes, unauthorized write 0 |
| `queue` | 7 days / 50 decisions | Backlog Promoter authority |
| `todo_runner` | 14 days / 100 decisions | assign/rerun authority |

Foundation slice accepts `authority_mode: observer` only.

## Deterministic exit reasons

Observer `run_once` emits stable `exit_reason` values including:

- `observer_success_no_mutation`
- `authority_not_observer`
- `lease_already_held`
- `cli_path_not_absolute` / `cli_nonzero_exit` / `stdout_not_json` (via CLI adapter)
- `decision_record_failed`

## Local control ledger exclusions

The ledger stores identifiers, hashes, timestamps, and bounded result codes. It never stores:

- prompts or comments
- task output bodies
- Multica auth tokens or other secrets

## Task Scheduler

Scripts under `scripts/task-scheduler/`:

- `install-production-controller.ps1` — register one-shot `agentmesh app run` task
- `uninstall-production-controller.ps1` — remove task
- `rollback-production-controller.ps1` — disable task and emit rollback correlation JSON

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
