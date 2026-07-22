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

## Non-Multica request adapter App

`apps/non-multica-request-adapter/agentmesh-app.toml` declares a tracker-neutral adapter contract for `agentmesh-request.v0` sources. Input accepts exactly one Markdown source or one Markdown-compatible JSON object:

```json
{"schema_version":"non-multica-request-adapter-input.v0","markdown":"---\ntitle: \"Add app\"\nrequest_kind: app\nissue_type: AFK\n---\n..."}
```

The compact payload is deterministic and contains `schema_version`, `adapter_version`, `request_schema_version`, `valid`, `canonical`, `issue_count`, and `issues[]`. `canonical` exposes stable fields such as `title`, `request_kind`, `issue_type`, `ready_for_multica`, `project_key`, source document paths, dependency arrays, and sequence fields. Unsupported request shapes produce deterministic `issues[].code` values such as `unsupported_request_kind`, `issue_type_missing`, and `sequence_incomplete` instead of tracker-specific exceptions.

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
