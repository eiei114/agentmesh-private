# AgentMesh contributor guide (Phase 0)

## Scope

Private Phase 0 contract spike. Do not port Multica business logic, add daemon/TUI/SQLite, or claim polyglot proof.

## Workspace rules

- Only the coordinator lane edits root `Cargo.toml` / `Cargo.lock`.
- Every crate is `publish = false`.
- Fixture plugins depend on `agentmesh-fixture-support` + `agentmesh-proto` only — never `agentmesh-host` or `agentmesh-conformance`.
- Malformed framing/JSON fixtures use independent raw writers.
- No vault paths, Multica types, or secrets in fixtures.

## Checks before PR

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Documentation status

- `docs/protocol-v0.md` is DRAFT until exit review.
- ADR 0001 stays Proposed until Phase 0 exit review accepts evidence.
