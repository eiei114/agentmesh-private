# Backlog Promoter Snapshot v0

Domain-owned immutable inventory contract for Python / AgentMesh dual-run parity.

Schema file: `schemas/backlog-promoter-snapshot-v0.schema.json`

## Identity

| Field | Value |
|---|---|
| `snapshot_schema_version` | `backlog-promoter-snapshot.v0` |
| `controller` | `backlog_promoter` |

This schema is **not** part of the host protocol envelope. AgentMesh core must remain unaware of Multica IDs.

## Canonical bytes and hash

1. Reject snapshots with a non-null top-level `error` for selector authority runs.
2. Sort every object key recursively.
3. Keep array order exactly as stored (producers must sort `issues` and `managed_projects` before write).
4. Encode UTF-8 JSON with separators `(',', ':')`, `ensure_ascii=false`, no trailing newline in the hashed payload.
5. Consume SHA-256 hex digest of those exact bytes.
6. Both selectors must report the same `consumed_snapshot_hash`.

Hash input is the immutable file bytes only when the file is already canonical. Prefer hashing the re-canonicalized object form so pretty-printed fixtures still yield a stable digest of semantic content; record which mode was used (`content` vs `raw_file`) in run context.

Default for differential parity: **content hash** of the parsed+re-canonicalized snapshot object.

## Limits

| Limit | Default | Hard max |
|---|---:|---:|
| `limits.max_issues` | 200 | 5000 |
| `limits.max_bytes` | 2_000_000 | 52_428_800 (50 MiB) |

Producers must set `issues_truncated` / `bytes_truncated` and a `truncation_reason` when caps fire. Truncated snapshots are valid for capture diagnostics but must not become selection authority input.

## Required evidence surfaces

- `schedule_inventory`: enough flattened autopilot rows for schedule admission
- `issues`: statuses scanned by Backlog Promoter
- `dependency_status_by_id`: statuses for blockers missing from `issues`
- `issue_run_presence`: frozen active/queued run presence by issue key
- `evidence_preflight_by_issue_id`: frozen preflight `ok` / `reason_code`
- `managed_projects`: secret-free project identity only (no credentials)

## Sanitization

Before any fixture enters git:

- strip tokens, cookies, Authorization headers, raw webhook secrets
- omit agent/runtime env values
- omit raw Multica CLI stderr
- keep issue metadata required for selection; redaction pointers may null sensitive free-text later

## Compatibility

Phase 1.0 recorded plugin inputs (`controller`/`mode`/`now`/`issues`) remain valid for the offline skeleton only. Live shadow and full parity use this snapshot v0 document.
