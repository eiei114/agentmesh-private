# AgentMesh — Phase 0 Contract + Phase 1.0 Shadow Adapter

Temporary private name: **agentmesh**. Final naming is deferred until the public-release gate.

## What this is

Phase 0 proves a **stable host ↔ plugin contract**:

- strict JSON-RPC 2.0 over LSP-style `Content-Length` framing
- one-shot initialize → run → close lifecycle
- exactly one compact JSON object on stdout
- bounded, potentially-sensitive audit sidecar on the local filesystem
- deterministic primary/secondary failure classification

Phase 1.0 adds a **shadow-mode Multica selector adapter skeleton** (`agentmesh-multica-selector-shadow`): recorded backlog listing → opaque plugin payload shaped like the Python compact selector contract. Sidecar remains audit evidence only; this is **not** production cutover.

The Markdown request validator App (`agentmesh-markdown-request-validator`) is tool-neutral: it accepts a bounded Markdown request document and emits compact JSON another orchestrator can consume without Multica fields, credentials, or domain types.

The adapter metadata canonicalizer App (`agentmesh-adapter-metadata-canonicalizer`) compares two adapter request metadata payloads, promotes only equal stable fields into a deterministic canonical subset, and preserves adapter-specific extensions separately.

The adapter evidence envelope App (`agentmesh-adapter-evidence-envelope`) normalizes validation/execution evidence into stable request id, capability hash, adapter identity, result class, diagnostics, and replay transcript digest fields.

The local tracker adapter App (`agentmesh-local-tracker-adapter`) is a second concrete non-Multica request target: it maps `agentmesh-request.v0` sources into deterministic local taskfile payloads, keeps stable canonical fields separate from adapter extensions, and emits schema-stable validation errors.

Still out of scope: Todo Runner parity, Multica credentials/live CLI, daemon/TUI/SQLite, WorkItem promotion into core, polyglot evidence.

## Supported targets (exact)

| Target | Required |
|---|---|
| `x86_64-pc-windows-msvc` | full tests + artifact smoke |
| `x86_64-unknown-linux-gnu` | full tests + artifact smoke |
| `aarch64-apple-darwin` | full tests + artifact smoke |

All other OS/arch combinations are unsupported in Phase 0.

## Five-minute local roundtrip (from source)

```bash
cargo build --release -p agentmesh-cli -p agentmesh-fixture-echo

# Windows example (adjust path separators / EXE suffix)
./target/release/agentmesh run \
  --plugin "$(pwd)/target/release/agentmesh-fixture-echo.exe" \
  --input ./examples/echo-input.json \
  --sidecar-dir ./.agentmesh/runs
```

Phase 1.0 shadow adapter (offline recorded listing; absolute plugin path required):

```bash
cargo build --release -p agentmesh-cli -p agentmesh-multica-selector-shadow

./target/release/agentmesh run \
  --plugin "$(pwd)/target/release/agentmesh-multica-selector-shadow.exe" \
  --input ./plugins/multica-selector-shadow/testdata/recorded_empty_backlog_input.json \
  --sidecar-dir ./.agentmesh/runs
```

Stdout is exactly one compact JSON envelope. The audit sidecar is written under:

```text
.agentmesh/runs/YYYY-MM-DD/<run-id>/full.json
```

## Downloaded artifact smoke

CI uploads immutable zip/tar bundles containing:

- `agentmesh` / `agentmesh.exe`
- `agentmesh-fixture-echo` / `agentmesh-fixture-echo.exe`
- `manifest.json` with full commit SHA, protocol version, versions, toolchain, target triple, and SHA-256 for each binary

Verify:

```bash
# After extracting an artifact bundle
python scripts/verify_artifact_manifest.py ./manifest.json
./agentmesh run --plugin ./agentmesh-fixture-echo --input ./examples/echo-input.json --sidecar-dir ./.agentmesh/runs
```

Rollback: choose a previous known-good workflow artifact by immutable name + manifest hashes. There is no mutable `latest` dependency. Source rollback is `git revert`.

## Trust boundary (Phase 0)

- Plugins are **trusted local absolute native executables** only. Relative paths, PATH lookup, command strings, shebangs, and shell wrappers are rejected.
- Host uses direct process APIs (`shell=false`); never constructs shell commands.
- Environment inheritance is cleared by default. Only a documented OS/runtime minimum is restored, plus explicitly allowlisted `--plugin-env KEY` names (values from parent env, never CLI values).
- This is **not a sandbox**. A trusted plugin may still read accessible files under inherited OS authority.
- Sidecars are **potentially sensitive** even with structured redaction. Do not attach them to public issues without inspection.
- Plugin stderr is hash/count-only by default. `--capture-plugin-stderr` stores bounded raw bytes and marks `sensitive_content=true`.
- Timeout / Ctrl-C terminate only the **directly managed child**. Grandchild containment is deferred.
- Sandboxing, signing, registry discovery, remote plugins: unsupported and deferred (see `docs/threat-model-v0.md`).

## Failure categories → operator action

| Exit | Category | Operator action |
|---:|---|---|
| 0 | success | continue |
| 2 | input | fix input path/JSON/size; no plugin spawn occurred |
| 10 | protocol | inspect framing/version/RPC/schema; check fixture health |
| 11 | plugin | check absolute path, spawn permissions, plugin application error |
| 12 | timeout / cancelled | increase timeout or stop interrupting; inspect hang |
| 13 | audit | fix sidecar directory permissions/space; never treat as success |
| 70 | internal | file bug with `run_id` / correlation from stderr |

## Explicit deferrals

- Backlog Promoter full parity / Todo Runner parity (later Phase 1+)
- Live Multica credentials / production cutover (shadow skeleton only in Phase 1.0)
- Non-Rust conformance claim
- Protocol notifications/progress/callbacks/shutdown/streaming/batching/cancellation
- Daemon / TUI / scheduler / production SQLite
- Public releases, crates.io, Homebrew/Scoop/Winget, auto-update
- Protocol `1.0` (needs Multica + Markdown adapters + migration policy)
- Final project/binary naming and public license

## Workspace crates

Production (`default-members`):

- `agentmesh-proto` — wire types, versions, JSON Schema
- `agentmesh-host` — process supervision, framing, lifecycle, sidecar
- `agentmesh-cli` — one-shot `run` command and `app` / `toolchain` subcommands
- `agentmesh-app` — App manifest validation, toolchain install, and pinned run policy

Apps / packaging (version-controlled, not `default-members`):

- `apps/backlog-promoter/` — reference `agentmesh-app.toml` for the Multica selector App
- `apps/markdown-request-validator/` — tool-neutral request validator manifest + IO schemas
- `apps/non-multica-request-adapter/` — tracker-neutral request adapter manifest + IO schemas
- `apps/local-tracker-adapter/` — local taskfile tracker adapter manifest + IO schemas
- `apps/adapter-metadata-canonicalizer/` — adapter metadata comparison/canonicalization manifest + IO schemas
- `apps/adapter-evidence-envelope/` — deterministic adapter evidence envelope manifest + IO schemas
- `apps/adapter-error-contract/` — shared adapter error boundary contract manifest + IO schemas
- `apps/public-0x-readiness/` — public 0.x readiness evidence gate manifest + IO schemas
- `toolchains/*.toml` — consumer pins for private prereleases (see `docs/private-prerelease-v0.md`)

Internal / test-only:

- `agentmesh-conformance` — reusable host-driven contract suite
- `agentmesh-fixture-support` — fixture lifecycle helpers (depends on `proto` only)
- Fixture plugins under `plugins/fixtures/*`
- `agentmesh-multica-selector-shadow` — Phase 1.0 offline Multica selector adapter (plugin-owned types only)
- `agentmesh-markdown-request-validator` — deterministic Markdown App request validator (plugin-owned types only)
- `agentmesh-non-multica-request-adapter` — tracker-neutral request adapter contract (plugin-owned types only)
- `agentmesh-local-tracker-adapter` — local taskfile tracker adapter contract (plugin-owned types only)
- `agentmesh-adapter-metadata-canonicalizer` — deterministic adapter metadata drift comparison and canonical subset emitter (plugin-owned types only)
- `agentmesh-adapter-evidence-envelope` — deterministic adapter evidence envelope binary (`adapter-metadata-canonicalizer` package)
- `agentmesh-adapter-error-contract` — shared adapter error boundary contract binary (`markdown-request-validator` package)
- `agentmesh-public-0x-readiness` — public 0.x readiness evidence gate binary (`adapter-metadata-canonicalizer` package)

Every crate is `publish = false` during private Phase 0/1.0.

## Development checks

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Docs status

- `docs/protocol-v0.md` — **Accepted for Phase 0** (private 0.x; not protocol 1.0)
- `docs/threat-model-v0.md` — **Accepted for Phase 0**
- `docs/adr/0001-external-stdio-plugins.md` — **Accepted** (Phase 0 exit review)
