# Public 0.x readiness report App

## Purpose

`agentmesh-public-0x-readiness-report` is a post-dogfood reporting App. It consumes retained request evidence digests and adapter evidence envelopes, then emits a deterministic compact report for the public 0.x launch-readiness packet.

The App is evidence-only. It does not call Multica, mutate tracker authority, tag releases, publish packages, upload assets, or perform production cutover.

## Input contract

Input schema: `public-0x-readiness-report-input.v0` (`apps/public-0x-readiness-report/schemas/public-0x-readiness-report-input-v0.schema.json`).

Required top-level fields:

- `generated_at` — caller-supplied report timestamp retained verbatim.
- `freshness.fresh_after` — deterministic freshness cutoff. Evidence with `captured_at` before this RFC 3339 timestamp fails freshness after absolute timestamp parsing.
- `coverage.minimum_request_count` — minimum unique request ids expected in the packet.
- `coverage.minimum_evidence_count` — optional minimum source evidence artifacts required per request for adapter comparison; defaults to `2`.
- `coverage.required_request_kinds` — request kinds that must be observed in source evidence, normally `app`.
- `coverage.required_evidence_fields` — digest fields that must be present per request, normally `title`, `request_kind`, `source_prd`, `source_design`, and `source_roadmap`.
- `coverage.required_envelopes[]` — adapter/phase pairs that must be present per request, for example Markdown validation and non-Multica execution envelopes.
- `request_evidence[]` — retained source evidence artifacts with `artifact_id`, `request_id`, `adapter_id`, `captured_at`, and an `agentmesh-adapter-evidence-digest.v0` payload under `digest`.
- `adapter_envelopes[]` — retained adapter evidence envelope artifacts with `artifact_id`, `request_id`, `captured_at`, and an `adapter-evidence-envelope-compact.v0` payload under `envelope`.

The App intentionally accepts retained JSON artifacts instead of live paths or tracker references, so local/non-Multica runners can assemble the same packet from their own artifact store.

## Output contract

Output schema: `public-0x-readiness-report-compact.v0` (`apps/public-0x-readiness-report/schemas/public-0x-readiness-report-compact-v0.schema.json`).

The compact report contains:

- `valid` — true only when all checks pass.
- `summary` — request/artifact counts plus pass/fail status for `coverage`, `freshness`, and `adapter_consistency`.
- `checks[]` — fixed-order check results. Each check carries deterministic `reasons[]`; pass states include an explicit `*_satisfied` reason, and fail states include stable blocker codes with optional `request_id`, `artifact_id`, and `field`.
- `requests[]` — normalized per-request digest with title, request kind, source references, source evidence artifact summaries, and adapter envelope summaries sorted by stable identifiers.
- `serialization` — deterministic ordering/null-policy notes for snapshot comparison.

Important failure codes include:

- Coverage: `minimum_request_count_not_met`, `required_request_kind_missing`, `required_evidence_field_missing`, `required_envelope_missing`.
- Freshness: `request_evidence_stale`, `adapter_envelope_stale`, `*_captured_at_missing`, `*_captured_at_invalid`, `fresh_after_missing`, `fresh_after_invalid`.
- Adapter consistency: `evidence_comparison_insufficient`, `evidence_field_mismatch`, `adapter_envelope_invalid_result`, `adapter_envelope_result_not_success`, schema/request-id mismatch codes.

## Local/non-Multica workflow

Generate or retain the source request evidence digest and adapter envelope JSON in any local artifact store, then assemble a report input file. No Multica credentials are required.

```bash
cargo build -p agentmesh-cli -p agentmesh-adapter-metadata-canonicalizer
cargo run -p agentmesh-cli -- app validate \
  --manifest apps/public-0x-readiness-report/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
cargo run -p agentmesh-cli -- app run \
  --manifest apps/public-0x-readiness-report/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input plugins/adapter-metadata-canonicalizer/testdata/public_0x_readiness_report_success_input.json \
  --sidecar-dir .agentmesh/runs \
  --mode development \
  --dev-plugin "$(pwd)/target/debug/agentmesh-public-0x-readiness-report"
```

A local scheduler can repeat the same command after dogfood/repair cycles, compare the compact payload against retained snapshots, and block a readiness claim whenever any check returns `fail`.
