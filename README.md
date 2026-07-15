# AgentMesh — Phase 0 Contract Spike

Temporary private name: **agentmesh**. Final naming is deferred until the public-release gate.

## What this is

Phase 0 proves a **stable host ↔ plugin contract**:

- strict JSON-RPC 2.0 over LSP-style `Content-Length` framing
- one-shot initialize → run → close lifecycle
- exactly one compact JSON object on stdout
- bounded, potentially-sensitive audit sidecar on the local filesystem
- deterministic primary/secondary failure classification

Phase 0 does **not** port Backlog Promoter / Todo Runner business logic, Multica credentials, daemon/TUI/SQLite, or claim polyglot evidence.

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

- Backlog Promoter / Todo Runner parity (Phase 1+)
- Real Multica plugin
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
- `agentmesh-cli` — one-shot `run` command

Internal / test-only:

- `agentmesh-conformance` — reusable host-driven contract suite
- `agentmesh-fixture-support` — fixture lifecycle helpers (depends on `proto` only)
- Fixture plugins under `plugins/fixtures/*`

Every crate is `publish = false` during private Phase 0.

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
