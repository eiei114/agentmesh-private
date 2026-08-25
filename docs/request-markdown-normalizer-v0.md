# Request Markdown Normalizer v0

## Purpose

`apps/request-markdown-normalizer/agentmesh-app.toml` declares a tool-neutral AgentMesh App that turns one validated request Markdown document into a deterministic preview/projection payload. It is for local runners and non-Multica adapters that need stable request diffs before materialization.

The App does not read the filesystem, does not require Multica credentials, and accepts only one JSON envelope with `schema_version: request-markdown-normalizer-input.v0` and a bounded `markdown` string.

## Input contract

```json
{"schema_version":"request-markdown-normalizer-input.v0","request_schema_version":"agentmesh-request.v0","markdown":"---\ntitle: \"Add app\"\nrequest_kind: app\nissue_type: AFK\n---\n..."}
```

Supported Markdown shape:

- YAML frontmatter fence with single-line `key: value` entries.
- Required stable fields: `title`, `request_kind`, `issue_type`, `status`, `project_key`, `source_prd`, `source_design`, `source_roadmap`, `blocked_by`, `unblocks`, `sequence_index`, and `sequence_total`.
- Optional compatibility fields such as `ready_for_multica` may appear in source frontmatter, but local-runner projection fields omit them.
- Supported H2 sections, emitted in canonical order: `Parent`, `What to build`, `Acceptance criteria`, `Blocked by`, `User stories covered`, and `Notes`.
- Required H2 sections: `What to build`, `Acceptance criteria`, `Blocked by`, `User stories covered`, and `Notes`.

## Output contract

The compact payload contains `valid`, `serialization`, `request_slug`, `slug_metadata`, `projection`, `content_hashes`, `error_count`, and `errors[]`.

When `valid` is `true`, `projection` contains:

- `fields` in canonical frontmatter order, excluding Multica-only readiness/authority hints.
- `sections` in canonical H2 order with LF line endings, trimmed edges, collapsed inline whitespace, and normalized `- ` bullets.
- `requirements`, derived from `Acceptance criteria`, sorted by requirement text and rendered as canonical `- [ ]` / `- [x]` checklist lines.
- `normalized_markdown`, a deterministic Markdown preview suitable for golden fixtures.
- `local_runner` with `uses_multica_fields: false` and a stable `agentmesh-request://<project>/<slug>` request id.

`request_slug` is derived only from `title`: lowercase ASCII alphanumeric runs joined by `-`, trimmed to 80 characters, with `untitled-request` fallback. `slug_metadata` records the algorithm so local replays can explain id derivation.

## Normalized errors

Unsupported JSON request objects, missing sections, unknown/nested section headings, malformed frontmatter arrays, duplicate fields, and malformed checklist markers produce stable adapter errors. Each record includes `code`, `category`, `path`, `message`, and `remediation_hint`.

Important codes include:

- `AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_UNSUPPORTED_SHAPE`
- `AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FRONTMATTER_MALFORMED`
- `AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_FIELD_REQUIRED`
- `AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_SECTION_UNSUPPORTED`
- `AGENTMESH_REQUEST_MARKDOWN_NORMALIZER_CHECKLIST_MALFORMED`

## Development smoke

```bash
cargo build -p agentmesh-cli -p agentmesh-markdown-request-validator --bins
agentmesh app validate \
  --manifest apps/request-markdown-normalizer/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
agentmesh app run \
  --manifest apps/request-markdown-normalizer/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input plugins/markdown-request-validator/testdata/request_markdown_normalizer_success_input.json \
  --sidecar-dir .agentmesh/runs \
  --mode development \
  --dev-plugin /absolute/path/to/target/debug/agentmesh-request-markdown-normalizer
```

Release, tag, asset upload, Multica authority changes, and production cutover remain outside this App contract.
