# Repair request: daily AgentMesh autopilot failure

Multica Issue: DOT-1267

## Failure code

- `agentmesh:repair:daily_autopilot_failed`

## Compact evidence summary

- Autopilot: `Schedule - AgentMesh Daily App Request`
- Autopilot ID: `dcc0deb2-bcb2-4f2f-b15f-eccd073f31f2`
- Failed run ID: `42405989-ccd4-40b0-a291-2ecc3007c752`
- Triggered at: `2026-07-27T22:40:19Z`
- Completed at: `2026-07-28T00:57:54Z`
- Status: `failed`
- Failure reason: `agent produced no new messages for 2h0m0s and message queue was empty; force-stopped by idle watchdog`
- Stable scope: `agentmesh:repair:daily_autopilot_failed`

## Deduplicated maintenance request

The follow-up is represented by DOT-1267 with source request path
`4_Project/OSS/agentmesh-private/Requests/Repair/2026-07-28-repair-daily-agentmesh-autopilot-failure.md`
and source issue path
`4_Project/OSS/agentmesh-private/Issues/2026-07-28-repair-daily-agentmesh-autopilot-failure.md`.

Use `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-07-28-repair-daily-agentmesh-autopilot-failure.md`
as the tracker dedupe key and `agentmesh:repair:daily_autopilot_failed` as the stable scope. Repeated sentinel runs
should update or skip this same repair request rather than creating another issue.

## Follow-up path

Repair requests now pass through the same AgentMesh request parse and non-Multica/local tracker adapter contracts as daily App requests. Downstream triage can materialize the request as `request_kind: repair`, keep failure/run details in adapter-owned passthrough metadata when needed, and route the bounded repair through the normal issue flow.

Release tags, package publishing, assets, secrets, permissions, production actions,
and Multica authority changes are intentionally out of scope for this repair.

## Verification

- `multica autopilot runs dcc0deb2-bcb2-4f2f-b15f-eccd073f31f2 --output json`
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
