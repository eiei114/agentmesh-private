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
