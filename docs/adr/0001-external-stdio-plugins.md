# ADR 0001: External stdio plugins

- Status: **Proposed**
- Date: 2026-07-15
- Deciders: Phase 0 spike authors

## Context

AgentMesh must keep tracker/tool-specific behavior out of core while preserving crash isolation and language freedom. In-process dynamic libraries couple ABI/stability risks to the host.

## Decision

Communicate with plugins as **external OS processes** over **strict JSON-RPC 2.0** framed with LSP-style `Content-Length` headers on stdio. Phase 0 accepts only absolute native executable paths. The host owns lifecycle/audit envelopes; plugin payloads remain opaque.

## Consequences

- Clear crash isolation and language-neutral design path
- Requires process supervision, framing, timeouts, and audit persistence in host
- Phase 0 proves Rust fixtures only; polyglot evidence and protocol 1.0 remain later gates
- ADR stays Proposed until Phase 0 exit review accepts the conformance matrix
