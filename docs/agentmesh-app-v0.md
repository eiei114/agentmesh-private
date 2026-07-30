# AgentMesh App Manifest v0

## Purpose

`agentmesh-app.toml` declares a versioned AgentMesh **App**: which logical plugin it binds,
which IO schemas it uses, and sidecar/env/conformance policy. Apps are validated against a
**toolchain pin** (`agentmesh-toolchain-pin.v0`) before run/package smoke.

## Files

| Artifact | Schema id | Location (example) |
|---|---|---|
| App manifest | `agentmesh-app.v0` | `apps/<name>/agentmesh-app.toml` |
| Toolchain pin | `agentmesh-toolchain-pin.v0` | `toolchains/*.toml` (version-controlled) |

## Manifest shape (v0)

Required top-level fields:

- `schema_version = "agentmesh-app.v0"`
- `name` — lowercase ASCII identity
- `protocol_version` — must equal host `PROTOCOL_VERSION`
- `[plugin].logical_name` — logical binary name (never a filesystem path)

Optional: `[limits]`, `[sidecar]`, `[env].allowlist`, `[schemas].input` / `.output`,
`[conformance].cargo_package`.

Forbidden keys (rejected by `agentmesh app validate`): `command`, `shell`, `exec`, `script`,
`run`, `args`, `cwd`, `install_path`, `plugin_path`.

## Validate

```bash
agentmesh app validate \
  --manifest apps/backlog-promoter/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
```

JSON mode:

```bash
agentmesh app validate \
  --manifest apps/backlog-promoter/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --json
```

## Markdown request validator App

`apps/markdown-request-validator/agentmesh-app.toml` declares a non-Multica App. Its input is:

```json
{"schema_version":"markdown-request-validator-input.v0","markdown":"---\ntitle: \"Add validator\"\n---\n..."}
```

Its compact payload is adapter-neutral: `valid`, `title`, `required_sections`, and deterministic `issues[]` codes/messages. Another orchestrator can read this JSON directly and decide whether to create, route, or reject work without linking Multica credentials or types.

Development smoke:

```bash
cargo build -p agentmesh-cli -p agentmesh-markdown-request-validator
agentmesh app validate \
  --manifest apps/markdown-request-validator/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
agentmesh app run \
  --manifest apps/markdown-request-validator/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input plugins/markdown-request-validator/testdata/valid_request_input.json \
  --sidecar-dir .agentmesh/runs \
  --mode development \
  --dev-plugin /absolute/path/to/target/debug/agentmesh-markdown-request-validator
```

Pinned production smoke uses the same manifest/input without `--mode development --dev-plugin` after installing a verified bundle that contains `agentmesh-markdown-request-validator`.

## Stable request parse CLI

`agentmesh request parse --input <json>` is the shared parser boundary for `agentmesh-request.v0` request sources. The input JSON accepts exactly one of:

- `markdown`: a Markdown request with YAML frontmatter and required `## What to build` / `## Acceptance criteria` sections.
- `request`: a Markdown-compatible JSON object containing the same stable frontmatter fields.
- Direct request fields at the top level for simple non-Multica adapters that already decoded their source envelope.

The parser accepts the shared `agentmesh-request-parse-input.v0` schema marker and existing adapter input schema markers (`markdown-request-validator-input.v0`, `non-multica-request-adapter-input.v0`, `local-tracker-adapter-input.v0`) so adapters can hand the same source envelope to the CLI before doing adapter-owned work.

Successful output is deterministic JSON with `schema_version: agentmesh-request-parse-output.v0`, `request_schema_version: agentmesh-request.v0`, `valid`, `canonical`, `error_count`, and `errors[]`. `canonical` contains only stable request fields: `title`, `request_kind`, `issue_type`, `ready_for_multica`, `status`, `project_key`, source document paths, dependency arrays, and sequence fields. `request_kind` accepts daily App supply (`app`) and deterministic maintenance follow-up (`repair`) so repair-first controller findings can use the same adapter handoff. Adapter-specific passthrough/routing fields stay outside this output; markdown and non-Multica adapters consume `canonical` first, then attach tracker-specific payloads under their own adapter-owned keys.

Request materializers also emit an adapter-neutral `evidence_digest` using `schema_version: agentmesh-adapter-evidence-digest.v0`. The digest serializes deterministic `sections[]` in the contract order `identity`, `sources`, then `routing`; each section contains ordered `fields[]` with `key`, `value`, and a human-readable `rationale`. Optional scalar values are serialized as `null`, dependency fields as arrays, and adapter-owned routing/passthrough values are excluded. Debug digest mismatches by comparing `section_order`, then each `sections[].fields[].key` and `value`; a mismatch means an adapter changed stable request parsing, not merely compact-output formatting.

Invalid input exits with code `2` and still writes a machine-readable payload. Stable error codes include `invalid_schema`, `unsupported_request_shape`, and `missing_required_section`.

Fixture smoke:

```bash
cargo test -p agentmesh-cli --test request_parse_cli
agentmesh request parse --input crates/agentmesh-cli/testdata/valid_request_input.json
agentmesh request parse --input crates/agentmesh-cli/testdata/invalid_request_input.json
```

## Non-Multica request adapter App

`apps/non-multica-request-adapter/agentmesh-app.toml` declares a tracker-neutral adapter contract for `agentmesh-request.v0` sources. Input accepts exactly one Markdown source or one Markdown-compatible JSON object:

```json
{"schema_version":"non-multica-request-adapter-input.v0","markdown":"---\ntitle: \"Add app\"\nrequest_kind: app\nissue_type: AFK\n---\n..."}
```

The compact payload is deterministic and contains `schema_version`, `adapter_version`, `request_schema_version`, `valid`, `canonical`, `evidence_digest`, `issue_count`, and `issues[]`. `canonical` exposes stable fields such as `title`, `request_kind`, `issue_type`, `ready_for_multica`, `project_key`, source document paths, dependency arrays, and sequence fields. `request_kind` accepts `app` and `repair`. `evidence_digest` uses the shared adapter evidence digest contract above so fixtures can compare Markdown and non-Multica materialization without duplicating parser-specific formatting logic. Unsupported request shapes produce deterministic `issues[].code` values such as `unsupported_request_kind`, `issue_type_missing`, and `sequence_incomplete` instead of tracker-specific exceptions.

Development smoke:

```bash
cargo build -p agentmesh-cli -p agentmesh-non-multica-request-adapter
agentmesh app validate \
  --manifest apps/non-multica-request-adapter/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
agentmesh app run \
  --manifest apps/non-multica-request-adapter/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input plugins/non-multica-request-adapter/testdata/valid_request_input.json \
  --sidecar-dir .agentmesh/runs \
  --mode development \
  --dev-plugin /absolute/path/to/target/debug/agentmesh-non-multica-request-adapter
```

Pinned production smoke uses the same manifest/input without `--mode development --dev-plugin` after installing a verified bundle that contains `agentmesh-non-multica-request-adapter`.

## Local tracker adapter App

`apps/local-tracker-adapter/agentmesh-app.toml` declares a second concrete non-Multica request adapter for a local taskfile-style tracker. It accepts exactly one Markdown source or Markdown-compatible JSON object plus optional adapter-owned passthrough:

```json
{
  "schema_version": "local-tracker-adapter-input.v0",
  "markdown": "---\ntitle: \"Add app\"\nrequest_kind: app\nissue_type: AFK\nstatus: ready\n---\n...",
  "adapter": {"passthrough": {"lane": "local-afk", "labels": ["agentmesh"]}}
}
```

The compact payload partitions stable request data under `canonical`, adapter-neutral review facts under `evidence_digest`, and local-tracker-specific wiring under `adapter`. Local runners can consume `tracker_ready_payload` directly: it contains a deterministic `local-taskfile://<project>/<title-slug>` id, title, project, kind, state, source document references, dependency arrays, and sequence fields for both `app` and `repair` requests. Multica-only metadata such as `ready_for_multica` is intentionally not emitted in `canonical` or tracker payloads, but it is retained inside `evidence_digest` when present so parity fixtures can compare local and non-Multica materialization evidence. Adapter extensions are preserved only under `adapter.extension` so downstream local tools can opt into them without changing the stable canonical contract. Malformed inputs return deterministic `issues[].code` values such as `title_missing`, `unsupported_request_kind`, `sequence_incomplete`, and `adapter_passthrough_not_object`.

Development smoke:

```bash
cargo build -p agentmesh-cli -p agentmesh-local-tracker-adapter
agentmesh app validate \
  --manifest apps/local-tracker-adapter/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
agentmesh app run \
  --manifest apps/local-tracker-adapter/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input plugins/local-tracker-adapter/testdata/valid_request_input.json \
  --sidecar-dir .agentmesh/runs \
  --mode development \
  --dev-plugin /absolute/path/to/target/debug/agentmesh-local-tracker-adapter
```

Pinned production smoke uses the same manifest/input without `--mode development --dev-plugin` after installing a verified bundle that contains `agentmesh-local-tracker-adapter`.

## Adapter metadata canonicalizer App

`apps/adapter-metadata-canonicalizer/agentmesh-app.toml` declares a tool-neutral App for comparing two adapter-owned request metadata payloads before downstream adapter-specific handling. Input names the two adapters and supplies each adapter's opaque metadata object:

```json
{
  "schema_version": "adapter-metadata-canonicalizer-input.v0",
  "left": {"adapter_id": "multica", "request_id": "DOT-1048", "metadata": {"title": "Add app", "request_kind": "app", "status": "ready"}},
  "right": {"adapter_id": "markdown", "request_id": "DOT-1048", "metadata": {"title": "Add app", "request_kind": "app", "status": "ready"}}
}
```

The compact payload is deterministic and contains `schema_version`, `app_version`, `request_schema_version`, `stable_fields`, `valid`, `canonical`, `schema_drift`, `mismatch_count`, `mismatches[]`, `adapters[]`, `issue_count`, and `issues[]`. The canonical contract promotes only stable fields that are present with equal JSON values in both adapter payloads; `request_id` is promoted only when both sides provide the same value. Stable fields with unequal values or one-sided presence are reported in deterministic `mismatches[]` order and remain under each adapter's `specific` object. Non-stable extension fields are never promoted; they are preserved under `adapters[].specific` so adapter-specific parsing can remain adapter-owned.

Development smoke:

```bash
cargo build -p agentmesh-cli -p agentmesh-adapter-metadata-canonicalizer
agentmesh app validate \
  --manifest apps/adapter-metadata-canonicalizer/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
agentmesh app run \
  --manifest apps/adapter-metadata-canonicalizer/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input plugins/adapter-metadata-canonicalizer/testdata/matching_metadata_input.json \
  --sidecar-dir .agentmesh/runs \
  --mode development \
  --dev-plugin /absolute/path/to/target/debug/agentmesh-adapter-metadata-canonicalizer
```

Pinned production smoke uses the same manifest/input without `--mode development --dev-plugin` after installing a verified bundle that contains `agentmesh-adapter-metadata-canonicalizer`.

## Adapter evidence envelope App

`apps/adapter-evidence-envelope/agentmesh-app.toml` declares an adapter-neutral App for normalizing validation and execution evidence. It consumes a request id, phase (`validation` or `execution`), adapter identity, capability descriptor, result class, deterministic diagnostics, and replay transcript facts:

```json
{
  "schema_version": "adapter-evidence-envelope-input.v0",
  "request_id": "DOT-1279",
  "phase": "validation",
  "adapter": {"id": "markdown-request-validator", "version": "markdown-request-validator.v0", "capabilities": ["request_parse"]},
  "capability": {"name": "agentmesh-request-validation", "schema_version": "agentmesh-request.v0", "operation": "validate"},
  "result": {"class": "success"},
  "diagnostics": [],
  "transcript": [{"step": "input", "digest": "sha256:request-fixture"}]
}
```

The compact payload is deterministic and contains `schema_version`, `app_version`, `valid`, `request_id`, `phase`, `capability_hash`, `adapter`, `result_class`, `deterministic_diagnostics`, `replay_transcript_digest`, `serialization`, and `retention`. Diagnostics are normalized to `code`, `field`, `severity`, and `message`, then sorted by `code`, `field`, `severity`, and `message`; replay transcripts are not copied into compact output, only hashed as canonical JSON. Result classes include `success`, `malformed_input`, `adapter_parity_mismatch`, `adapter_error`, and `execution_error`, so adapters can compare success, malformed input, and parity-mismatch evidence without adapter-specific keys.

Evidence retention is `owner_local`: `agentmesh app run` writes host sidecars below the caller-provided `--sidecar-dir/YYYY-MM-DD/<run-id>/full.json`. Adapter-facing tooling should retain the compact envelope and the sidecar reference; raw replay transcript material remains outside the compact payload unless separately retained by the owner.

Development smoke:

```bash
cargo build -p agentmesh-cli -p agentmesh-adapter-metadata-canonicalizer
agentmesh app validate \
  --manifest apps/adapter-evidence-envelope/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
agentmesh app run \
  --manifest apps/adapter-evidence-envelope/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input plugins/adapter-metadata-canonicalizer/testdata/evidence_envelope_success_input.json \
  --sidecar-dir .agentmesh/runs \
  --mode development \
  --dev-plugin /absolute/path/to/target/debug/agentmesh-adapter-evidence-envelope
```

Pinned production smoke uses the same manifest/input without `--mode development --dev-plugin` after installing a verified bundle that contains `agentmesh-adapter-evidence-envelope`.

## Public 0.x readiness gate App

`apps/public-0x-readiness/agentmesh-app.toml` declares an evidence-only readiness-capability App. It consumes retained compact outputs from the Markdown request validator and non-Multica request adapter, plus an explicit checklist for protocol acceptance, adapter compatibility, rollback proof, and evidence retention. It emits deterministic `public-0x-readiness-compact.v0` JSON with `valid`, `assertions[]`, and `issues[]` so public readiness claims can be reviewed without live Multica credentials or production authority.

Run it only after generating and retaining the parser snapshot and adapter parity outputs for the request under review:

```bash
cargo build -p agentmesh-cli -p agentmesh-markdown-request-validator -p agentmesh-non-multica-request-adapter -p agentmesh-adapter-metadata-canonicalizer
agentmesh app validate \
  --manifest apps/public-0x-readiness/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
agentmesh app run \
  --manifest apps/public-0x-readiness/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input plugins/adapter-metadata-canonicalizer/testdata/public_0x_readiness_input.json \
  --sidecar-dir .agentmesh/runs \
  --mode development \
  --dev-plugin /absolute/path/to/target/debug/agentmesh-public-0x-readiness
```

See `docs/public-0x-readiness-gate.md` for the checklist, required artifacts, rollback proof, and retention rules. The gate must not be used to tag, publish, upload assets, mutate Multica authority, or perform production cutover.

## Run

Production/canary (default) resolves the logical plugin under the local toolchain cache,
verifies release-manifest digest + binary SHA-256 + protocol, rejects path escape, then
delegates an absolute path to the one-shot host:

```bash
agentmesh app run \
  --manifest apps/backlog-promoter/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input path/to/snapshot.json \
  --sidecar-dir .pi/tmp/agentmesh/sidecars \
  --toolchain-cache ~/.agentmesh/toolchains
```

Development override (explicit): `--dev-plugin` is rejected unless `--mode development`:

```bash
agentmesh app run \
  --manifest apps/backlog-promoter/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input path/to/snapshot.json \
  --sidecar-dir .pi/tmp/agentmesh/sidecars \
  --mode development \
  --dev-plugin /absolute/path/to/plugin
```

Unpinned runs write `agentmesh-app-run-marker.txt` under `--sidecar-dir` and append a
`app_run_mode=unpinned ...` diagnostic to the compact envelope.

## Cache layout (resolve)

```text
~/.agentmesh/toolchains/<tag>/<target>/
  release-manifest.json
  bin/<logical-name>[.exe]
```

`release-manifest.json` schema id: `agentmesh-release-manifest.v0`.
Consumer pin `release_manifest_sha256` must match the file digest before any binary is used.

## Package (all three targets)

Local/CI packaging:

```bash
python scripts/package_toolchain_bundle.py \
  --out dist/x86_64-pc-windows-msvc \
  --target x86_64-pc-windows-msvc \
  --tag v0.2.0-dev.1 \
  --commit "$(git rev-parse HEAD)" \
  --bin-dir target/x86_64-pc-windows-msvc/release \
  --also-flat-phase0-manifest \
  --include-echo
```

Targets: `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`.
Bundle always includes `agentmesh` + `agentmesh-multica-selector-shadow` + `agentmesh-markdown-request-validator` + apps/docs + `release-manifest.json`.

## Install (atomic / immutable)

```bash
agentmesh toolchain install \
  --bundle dist/x86_64-pc-windows-msvc \
  --toolchain-cache ~/.agentmesh/toolchains
```

Behavior:

- exclusive per-tag/target `.install.lock` (`create_new`)
- copy into sibling `.staging-<target>-*` then verify hashes
- atomic rename into `~/.agentmesh/toolchains/<tag>/<target>/`
- refuse overwrite if the final directory already exists (Windows-safe immutable dir)
- best-effort mark installed files read-only

## Pin rules

- Pins MUST record `tag`, `commit_sha`, `target`, and `release_manifest_sha256`.
- Pins MUST NOT contain machine-local fields (`install_path`, `cache_path`, `local_path`, `path`, `plugin_path`).
- Supported targets: `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`.
- Real prerelease pins replace the placeholder digest before Gate G1.

## Protocol

Top-level `protocol_version` MUST equal the CLI build's `agentmesh_proto::PROTOCOL_VERSION`
(`2026-07-15` at Wave 2 start).

## Wave 2 follow-ups

- Prepare private PR + release draft notes; **stop at G1** (no merge/tag/assets without human approval)
