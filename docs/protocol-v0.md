# Protocol v0 (DRAFT)

Status: **DRAFT** — guides implementation; not a compatibility promise until Phase 0 exit review.

Wire version: `2026-07-15`

## Transport

- JSON-RPC 2.0 over stdio
- LSP-style framing: `Content-Length: <bytes>\r\n\r\n` + UTF-8 JSON body
- Byte length (not character count)
- Header bounds: 8 KiB block, 1 KiB/line, 16 lines; exactly one `Content-Length`
- Unknown syntactically valid headers ignored + audited
- Duplicate keys rejected; batch arrays rejected
- Request IDs: non-empty visible ASCII, ≤128 bytes

## Lifecycle

1. Host spawns absolute native executable (`shell=false`)
2. `initialize` with supported protocol versions + host capabilities
3. Plugin returns selected protocol version, plugin semver, capabilities
4. Host sends one `agentmesh.run` with `run_id` + opaque `input`
5. Plugin returns result with opaque `payload` or application error
6. Host closes stdin
7. Plugin exits within exit grace (no shutdown RPC in v0)
8. Host write-once commits sidecar, prints one compact JSON object, exits

No notifications, concurrent requests, streaming, cancellation RPC, or host callbacks in v0.

## Compact stdout

Exactly one JSON object:

```json
{
  "schema_version": "2026-07-15",
  "run_id": "...",
  "outcome": "ok",
  "payload": {},
  "artifacts": [".agentmesh/runs/.../full.json"],
  "diagnostics": []
}
```

Host owns the envelope; `payload` is plugin-owned opaque JSON.

## Limits (normative defaults)

| Limit | Default |
|---|---|
| input | 1 MiB |
| frame | 4 MiB |
| retained stderr | 256 KiB (then discard-drain) |
| initialize timeout | 5 s |
| run timeout | 60 s (configurable 1 s … 1 h) |
| exit grace | 2 s |
| sidecar | 10 MiB |
| JSON depth / nodes | 64 / 100_000 |

## Capabilities

Phase 0: `compact_output`, `sidecar_refs`. Unknown names ignored but audited.

## JSON Schemas

Generated schemas live in `schemas/protocol-v0.schema.json` (kept in sync by unit tests).

## Language neutrality

Protocol is language-neutral **by design**. Phase 0 proves Rust fixture separation only; polyglot evidence is deferred.
