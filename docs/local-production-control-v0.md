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

Observer CLI reads are fixed to the current shell-free Multica contract:
`multica issue list --output json`. Arbitrary query arguments are not accepted.

On Windows, Multica-invoking App manifests forward only `USERPROFILE`,
`LOCALAPPDATA`, `APPDATA`, and `PROGRAMDATA` so the pinned CLI can locate its
existing owner-local state after the host clears the plugin environment. Token
and API-key environment variables remain forbidden.

CLI subprocess timeouts are constrained to 1,000–70,000 ms. Every composing
App has a 120,000 ms host limit, reserving 50 seconds for process-tree
termination, bounded pipe drain, ledger persistence, and compact output.
Windows process-tree containment requires Windows 10 version 1809 or later.
Before activation, run the observer from its installed Task Scheduler context;
an outer Job that rejects nested Job attachment must fail closed at spawn.

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

Observer and authority `run_once` paths claim controller-scoped idempotency **before** invoking the Multica CLI. Duplicate inputs suppress the CLI call and release leases. Scheduled observer inputs include a bounded `occurrence_id` derived from the anchored schedule slot; retries inside the same slot retain that identity even though `now` advances for lease safety.

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
- `run-production-controller.ps1` — materialize a unique UTC `now`/`lease_id` for each scheduled occurrence, run the pinned App, then remove the temporary input
- `uninstall-production-controller.ps1` — remove task
- `rollback-production-controller.ps1` — disable task, then record rollback through the durable pinned ledger App/cache; missing or malformed receipt exits nonzero

Operators install/configure tasks manually; CI does not activate schedules.
The checked-in input is a template. The installer pins one UTC schedule anchor and interval into the task action. The runner derives a stable fixed-length occurrence/lease identity from that slot, so retries of one occurrence deduplicate while the next interval receives a distinct identity.
Before registration, the installer verifies the pin, release manifest, and every
pinned binary, then copies the AgentMesh host, runner/rollback/uninstall scripts,
observer and ledger App directories, toolchain pin/cache, and input template into
an immutable content-addressed directory under
`%LOCALAPPDATA%\AgentMesh\scheduler-assets\`. Scheduled runs and rollback pass
that durable cache explicitly, so they never depend on a temporary release
extraction or mutable default cache. `-PrepareOnly` stages and verifies those
assets without registering a task. Supported intervals are 1–1440 minutes.

Task Scheduler success requires both process exit zero and a valid compact
observer result. Only `observer_success_no_mutation` with a successful,
non-truncated CLI summary and complete ledger receipts is success. A duplicate
claim is fail-closed because a claim alone does not prove the prior occurrence
completed; CLI, ledger, duplicate, malformed-envelope, or host failures return
nonzero.

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
