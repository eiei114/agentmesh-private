# Markdown Adapter Replay Fixture v0

## Purpose

`apps/markdown-adapter-replay-fixture/agentmesh-app.toml` declares a tool-neutral AgentMesh App that replays one validated request Markdown document against a declared adapter fixture and an expected canonical result. Local runners and non-Multica adapters can compare deterministic pass or mismatch diagnostics before issue materialization without Multica credentials, orchestrator IDs, or filesystem access.

## Input contract

```json
{
  "schema_version": "markdown-adapter-replay-fixture-input.v0",
  "request_schema_version": "agentmesh-request.v0",
  "markdown": "---\ntitle: \"Add app\"\nrequest_kind: app\nissue_type: AFK\n---\n...",
  "fixture": {
    "fixture_id": "matching-projection-v0",
    "adapter": "local-runner-adapter",
    "scenario": "matching_result"
  },
  "expected": {
    "replay_status": "pass",
    "request_slug": "add-app",
    "projection_fields": {
      "title": "Add app",
      "request_kind": "app",
      "issue_type": "AFK",
      "project_key": "agentmesh-private"
    }
  }
}
```

The input intentionally excludes tracker-specific fields. Adapter execution failures are declared under `fixture.adapter_failure` and replayed through the shared adapter error contract when present.

## Output contract

The compact payload is `markdown-adapter-replay-fixture-compact.v0` and always separates:

- `request` — common request summary (`title`, `request_kind`, `issue_type`, `project_key`, `request_slug`)
- `projection` — canonical projection hashes and slug metadata from the Markdown normalizer
- `fixture` — adapter-specific fixture identifiers, scenario, optional `adapter_failure`, and adapter digest
- `adapter` — normalized adapter error codes when an adapter failure fixture is declared
- `comparison` — expected vs actual replay status plus deterministic field mismatches

`replay_status` values:

- `pass` — actual canonical projection and adapter diagnostics match `expected`
- `canonical_projection_mismatch` — normalized projection fields or slug differ from `expected`
- `adapter_error_mismatch` — normalized adapter error codes differ from `expected.adapter_error_codes`
- `malformed_input` — Markdown normalization failed with stable normalizer error codes

## Fixture scenarios

Recorded fixtures cover:

1. `matching_result` — valid Markdown with matching projection fields
2. `canonical_projection_mismatch` — valid Markdown with intentionally wrong expected slug/title
3. `adapter_error_mismatch` — adapter timeout fixture with wrong expected adapter error codes
4. `malformed_input` — missing frontmatter with deterministic normalizer error codes

## Development smoke

```bash
cargo build -p agentmesh-cli -p agentmesh-markdown-request-validator --bins
agentmesh app validate \
  --manifest apps/markdown-adapter-replay-fixture/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
agentmesh app run \
  --manifest apps/markdown-adapter-replay-fixture/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input plugins/markdown-request-validator/testdata/markdown_adapter_replay_fixture_success_input.json \
  --sidecar-dir .agentmesh/runs \
  --mode development \
  --dev-plugin /absolute/path/to/target/debug/agentmesh-markdown-adapter-replay-fixture
```

Example compact result shape:

```json
{
  "schema_version": "markdown-adapter-replay-fixture-compact.v0",
  "app_version": "markdown-adapter-replay-fixture.v0",
  "valid": true,
  "replay_status": "pass",
  "request": {
    "title": "Add a Markdown request normalizer App",
    "request_slug": "add-a-markdown-request-normalizer-app"
  },
  "comparison": {
    "expected_replay_status": "pass",
    "actual_replay_status": "pass",
    "matches": true,
    "mismatch_count": 0,
    "mismatches": []
  }
}
```

Release, tag, asset upload, Multica authority changes, and production cutover remain outside this App contract.
