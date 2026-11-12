# Request Markdown Renderer v0

## Purpose

`apps/request-markdown-renderer/agentmesh-app.toml` declares a tool-neutral AgentMesh App that renders one validated canonical request summary back into a canonical `agentmesh-request.v0` Markdown source document and reports an explicit round-trip digest status.

The App is the reverse direction of `apps/request-markdown-normalizer/`: the normalizer parses Markdown into a canonical projection, and the renderer proves that a canonical summary regenerates Markdown which re-enters the same canonical projection with an identical digest. A local runner or non-Multica adapter can therefore regenerate a request source before materializing its own downstream artifact, and compare the regenerated source against the stored one with a machine-checkable result.

The App does not read or overwrite files, call a tracker, discover adapters, or echo credentials, raw logs, sidecar paths, or tracker identifiers. It accepts only one JSON envelope.

## Input contract

```json
{
  "schema_version": "request-markdown-renderer-input.v0",
  "request_schema_version": "agentmesh-request.v0",
  "summary": {
    "valid": true,
    "canonical": {
      "title": "Add a canonical request Markdown renderer App",
      "request_kind": "app",
      "issue_type": "AFK",
      "status": "ready",
      "project_key": "agentmesh-private",
      "source_prd": "synthetic://requests/request-markdown-renderer",
      "source_design": "synthetic://docs/agentmesh-request-operations-v1",
      "source_roadmap": "synthetic://roadmaps/agentmesh-private",
      "blocked_by": [],
      "unblocks": [],
      "sequence_index": 1,
      "sequence_total": 1
    },
    "sections": {
      "What to build": "Render one validated canonical summary.",
      "Acceptance criteria": "- [ ] Round-trip digest matches.",
      "Blocked by": "- None.",
      "User stories covered": "- As a non-Multica adapter maintainer.",
      "Notes": "- Tracker identifiers are never rendered."
    }
  }
}
```

- `summary.valid` — the summary retained from the pinned request parser or its fingerprint manifest. The renderer renders only `true`; `false` is a deterministic failure with no Markdown.
- `summary.canonical` — canonical frontmatter fields. The required fields are `title`, `request_kind`, `issue_type`, `status`, `project_key`, `source_prd`, `source_design`, `source_roadmap`, `blocked_by`, `unblocks`, `sequence_index`, and `sequence_total`. Supported `request_kind` values are `app` and `repair`.
- `summary.sections` — either an array of `{heading, body}` objects or a map of heading to body. Supported headings, in canonical render order: `Parent`, `What to build`, `Acceptance criteria`, `Blocked by`, `User stories covered`, `Notes`. `What to build`, `Acceptance criteria`, `Blocked by`, `User stories covered`, and `Notes` are required.
- `adapter_extras` — optional adapter-owned map. Values are reported as bounded diagnostics and never rendered.

Whole-envelope input is bounded to 64 KiB. Non-canonical fields inside `summary.canonical` and adapter-owned keys under `adapter_extras` are reported as diagnostics, not errors.

## Output contract

The compact payload is `request-markdown-renderer-compact.v0` and contains:

- `valid`, `renderer_version`, `request_schema_version`, `serialization`
- `request_slug`, `markdown`, `content_hashes` (`markdown_sha256`, `input_canonical_sha256`)
- `round_trip` with `status`, both canonical digests, and normalized `mismatches[]`
- `diagnostic_count` / `diagnostics[]` (bounded to 32 records plus one truncation record)
- `error_count` / `errors[]`

Rendered Markdown properties:

- frontmatter keys follow the canonical order with `key: value` single-line entries and JSON arrays for `blocked_by` / `unblocks`
- supported H2 sections follow the canonical order with LF line endings and normalized `-` bullets
- acceptance criteria are emitted as canonical `- [ ]` / `- [x]` checklist lines sorted by requirement text
- an empty section body renders the canonical `- none` placeholder
- the writer is the same canonical writer the normalizer App exposes, so rendered sources cannot drift from their canonical projection by construction

`round_trip.status` values:

- `match` — the rendered Markdown re-entered the canonical parser and produced the same canonical digest, fields, sections, requirements, and slug
- `mismatch` — normalized mismatch codes identify what drifted

Normalized mismatch codes:

- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_RENDER_UNPARSEABLE`
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_RENDER_PARSER_ERRORS`
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_DIGEST_MISMATCH`
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_MISMATCH`
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_SECTION_MISMATCH`
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_REQUIREMENT_MISMATCH`
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_SLUG_MISMATCH`

Diagnostic codes:

- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_ADAPTER_OWNED_FIELD`
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_NON_CANONICAL_FIELD`
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_NESTED_HEADING`
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_DIAGNOSTICS_TRUNCATED`

Failure codes:

- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_INPUT_INVALID` — unsupported `schema_version` / `request_schema_version`, oversized input, wrong envelope types
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_REQUIRED` — missing envelope or canonical field
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_FIELD_INVALID` — wrong scalar type, unsupported `request_kind`, `sequence_index > sequence_total`
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_INVALID` — `summary.valid` is not `true`
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_SUMMARY_MALFORMED` — unknown summary key, non-object `summary` / `canonical` / section entry
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_SECTION_REQUIRED` — missing canonical section
- `AGENTMESH_REQUEST_MARKDOWN_RENDERER_SECTION_UNSUPPORTED` — unknown or duplicated section heading

Every failure returns `markdown: null` and `round_trip: null`, so a caller never receives partial Markdown.

## Regenerate and diff a request source

A local runner or non-Multica adapter uses the pair as one verification step before materializing its downstream artifact:

1. parse or normalize the stored request source and keep its canonical summary (`canonical` fields plus section bodies and the canonical digest)
2. run `request-markdown-renderer` on that summary
3. fail the step unless `valid` is `true` and `round_trip.status` is `match`
4. compare `markdown` byte-for-byte with the stored source, or store `content_hashes.markdown_sha256` as the regenerated digest
5. keep `diagnostics[]` as bounded evidence: adapter-owned fields are reported but never appear in the rendered source

A hand-edited source that drifted from its canonical projection shows up as a `mismatch` (or as a stored-vs-regenerated byte difference) instead of silently materializing.

## Fixtures

`plugins/markdown-request-validator/testdata/` records the JSON inputs and expected compact payloads:

- `request_markdown_renderer_complete_input.json` — complete `app` summary with intentionally unsorted sections
- `request_markdown_renderer_repair_input.json` — `request_kind: repair` summary
- `request_markdown_renderer_missing_field_input.json` — summary missing `source_design`
- `request_markdown_renderer_adapter_extras_input.json` — summary plus adapter-owned extras (`multica_issue_id`, `sidecar_path`, `tracker_assignee`, `raw_log_path`)

## Development smoke

```bash
cargo build -p agentmesh-cli -p agentmesh-markdown-request-validator --bins
agentmesh app validate \
  --manifest apps/request-markdown-renderer/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
agentmesh app run \
  --manifest apps/request-markdown-renderer/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input plugins/markdown-request-validator/testdata/request_markdown_renderer_complete_input.json \
  --sidecar-dir .agentmesh/runs \
  --mode development \
  --dev-plugin /absolute/path/to/target/debug/agentmesh-request-markdown-renderer
```

Release, tag, asset upload, Multica authority changes, and production cutover remain outside this App contract.
