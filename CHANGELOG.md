# Changelog

All notable changes to this private Phase 0 workspace are documented here.

## [Unreleased]

### Added

- Offline `agentmesh docs show <name>` lookup with exact-name JSON contracts and
  coding-agent guidance that preserves existing stdout envelopes.
- Bounded OKF graph candidate blending for evidence hybrid mode, excluding
  structural hub relations and keeping graph default enablement behind the
  existing promotion gates.
- Read-only `agentmesh evidence compile|health|evaluate` runtime with direct QMD,
  semantic QMD, side-effect-free adaptive discovery, deterministic fusion,
  namespace/sensitivity enforcement, optional v2 graph traversal, and bounded
  source-linked Decision / AgentRun packets.
- Phase 1.0 shadow-mode Multica selector adapter skeleton (`plugins/multica-selector-shadow`): opaque `agentmesh.run` input/payload, recorded offline fixtures, named shadow compact-shape compare, host roundtrip with audit sidecar. No production cutover.
- Markdown request validator App (`apps/markdown-request-validator/`, `plugins/markdown-request-validator/`): tool-neutral manifest + deterministic compact payload validation without Multica credentials.
- Adapter metadata canonicalizer App (`apps/adapter-metadata-canonicalizer/`, `plugins/adapter-metadata-canonicalizer/`): deterministic cross-adapter metadata drift report, canonical stable-field subset, and preserved adapter-specific extensions.

### Changed

- README workspace list: document `agentmesh-request-evidence` alongside other `crates/` workspace members.
- README workspace list: document `apps/adapter-error-contract`, `apps/public-0x-readiness`, and their plugin binaries to match the current `apps/` layout.

- Phase 0 exit review: reconcile `docs/protocol-v0.md` and `docs/threat-model-v0.md` to implementation evidence; accept ADR 0001.
- README workspace crate list: document `agentmesh-app`, `apps/backlog-promoter`, and `toolchains/` consumer pins to match the current workspace layout.

## [0.1.0] — 2026-07-15

### Added

- Cargo workspace with `agentmesh-proto`, `agentmesh-host`, `agentmesh-cli`, internal `agentmesh-conformance`, and test-only fixture support.
- Strict JSON-RPC 2.0 + LSP-style Content-Length framing.
- One-shot initialize/run/close lifecycle, compact stdout envelope, bounded audit sidecar.
- Compiled fixture plugins and three-target CI with immutable artifact manifests.
- Draft `protocol-v0.md`, threat model, and Proposed ADR 0001.
