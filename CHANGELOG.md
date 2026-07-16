# Changelog

All notable changes to this private Phase 0 workspace are documented here.

## [Unreleased]

### Added

- Phase 1.0 shadow-mode Multica selector adapter skeleton (`plugins/multica-selector-shadow`): opaque `agentmesh.run` input/payload, recorded offline fixtures, named shadow compact-shape compare, host roundtrip with audit sidecar. No production cutover.

### Changed

- Phase 0 exit review: reconcile `docs/protocol-v0.md` and `docs/threat-model-v0.md` to implementation evidence; accept ADR 0001.
- README workspace crate list: document `agentmesh-app`, `apps/backlog-promoter`, and `toolchains/` consumer pins to match the current workspace layout.

## [0.1.0] — 2026-07-15

### Added

- Cargo workspace with `agentmesh-proto`, `agentmesh-host`, `agentmesh-cli`, internal `agentmesh-conformance`, and test-only fixture support.
- Strict JSON-RPC 2.0 + LSP-style Content-Length framing.
- One-shot initialize/run/close lifecycle, compact stdout envelope, bounded audit sidecar.
- Compiled fixture plugins and three-target CI with immutable artifact manifests.
- Draft `protocol-v0.md`, threat model, and Proposed ADR 0001.
