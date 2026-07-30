# Public 0.x readiness evidence gate

## Purpose

Run this gate before any public 0.x readiness claim. It is an evidence gate only: it does not tag, publish, upload assets, change production authority, or mutate Multica state.

## Readiness checklist

A readiness packet must show all of the following:

- Protocol checkpoint: `docs/protocol-v0.md`, host/plugin compact envelopes, and schema versions remain accepted for the target claim.
- Adapter compatibility checkpoint: Markdown request validation and non-Multica request materialization both succeed for the same request title and source references.
- Rollback checkpoint: the packet names the previous good immutable artifact, the revert command or plan, and a successful verification note after rollback rehearsal.
- Evidence retention checkpoint: parser snapshots, adapter parity outputs, rollback notes, and command logs are retained in a durable review location for at least 30 days.

## Required artifacts

- Deterministic parser output snapshot: compact JSON from `agentmesh-markdown-request-validator`.
- Adapter parity evidence: compact JSON from `agentmesh-non-multica-request-adapter` showing `agentmesh-request.v0`, retained source document references, and matching request title.
- Readiness gate compact output: compact JSON from `agentmesh-public-0x-readiness` with `valid: true` and zero issues.
- Rollback replay bundle: compact JSON from `agentmesh-public-0x-rollback-replay` with `valid: true`, retained `manifest_hash`, deterministic `adapter_digest_hash`, `replay_transcript_hash`, `request_hash`, rollback commands, and at least 30-day `evidence_retention`.
- Rollback verification notes: previous good artifact name, release-manifest SHA-256, rollback/revert command, and the exact post-rollback test command output.

## Operating steps

```bash
cargo build -p agentmesh-cli -p agentmesh-markdown-request-validator -p agentmesh-non-multica-request-adapter -p agentmesh-adapter-metadata-canonicalizer
cargo run -p agentmesh-cli -- app validate \
  --manifest apps/public-0x-readiness/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
cargo run -p agentmesh-cli -- app run \
  --manifest apps/public-0x-readiness/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input plugins/adapter-metadata-canonicalizer/testdata/public_0x_readiness_input.json \
  --sidecar-dir .agentmesh/runs \
  --mode development \
  --dev-plugin "$(pwd)/target/debug/agentmesh-public-0x-readiness"
cargo run -p agentmesh-cli -- app validate \
  --manifest apps/public-0x-rollback-replay/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml
cargo run -p agentmesh-cli -- app run \
  --manifest apps/public-0x-rollback-replay/agentmesh-app.toml \
  --toolchain-pin toolchains/agentmesh-pin.v0.example.toml \
  --input plugins/adapter-metadata-canonicalizer/testdata/public_0x_rollback_replay_input.json \
  --sidecar-dir .agentmesh/runs \
  --mode development \
  --dev-plugin "$(pwd)/target/debug/agentmesh-public-0x-rollback-replay"
```

Before replacing the fixture input with live evidence, preserve the markdown-validator and non-Multica adapter compact outputs exactly as generated. If any assertion fails, do not make a public readiness claim; repair the missing evidence or document the rollback blocker first.
