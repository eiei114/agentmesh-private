# PR Draft — Campaign `agentmesh-multica-operations` (Gate G1)

> **Status:** DRAFT ONLY — do not merge until human explicitly answers `G1`.
> **Branch:** `campaign/agentmesh-multica-operations`
> **Base:** `main` @ `376f849893654a8d2de868e79f0c5d0aefb4308c`
> **Prepared:** Ralph iteration 10 (Wave 2 close-out)

## Title (suggested)

```text
feat(wave2): AgentMesh App manifest, toolchain pin/cache, and private prerelease channel
```

## Summary

This campaign branch adds the Wave 1–2 foundation required to dogfood Backlog Promoter shadow selection through a pinned private AgentMesh toolchain:

- Snapshot v0 schema + Rust parity selector path
- `agentmesh-app` crate (`agentmesh-app.toml` v0, toolchain pin v0, validate/run/resolve/install)
- Three-target packaging + downloaded-bundle smoke
- Private prerelease workflow that **refuses asset replacement**

Python remains the live selection authority. Vault shadow wrapper changes stay separate and wait for a published pin after G1.

## Scope included (AgentMesh repo)

### Wave 1 — Snapshot / parity

- `schemas/backlog-promoter-snapshot-v0.schema.json`
- `docs/backlog-promoter-snapshot-v0.md`
- `plugins/multica-selector-shadow` snapshot parity + `agentmesh-multica-select-snapshot` bin
- Fixture: `testdata/one_candidate.snapshot.json`

### Wave 2 — App + private distribution

- `crates/agentmesh-app` (manifest/pin/validate/resolve/run_policy/install)
- CLI: `agentmesh app validate|run`, `agentmesh toolchain install`
- Example app: `apps/backlog-promoter/`
- Example/generated pins: `toolchains/`, packaging `pin.generated.toml`
- Scripts: `package_toolchain_bundle.py`, `smoke_downloaded_bundle.py`, `assert_prerelease_absent.py`
- CI matrix packaging/smoke for windows/linux/darwin
- `.github/workflows/private-prerelease.yml` (manual; requires `confirm_publish=PUBLISH`)
- Docs: `docs/agentmesh-app-v0.md`, `docs/private-prerelease-v0.md`

## Explicitly out of scope / not done in this PR

- Merging to `main` before G1
- Creating git tags or GitHub release assets before G1
- Running `private-prerelease.yml` with `PUBLISH`
- Vault Wave 3 live shadow wrapper / runtime config (blocked on published pin)
- Authority cutover away from Python

## Test plan (reviewer)

```bash
# from AgentMesh worktree
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p agentmesh-app

# local Windows package+smoke (monitor-rerunnable)
cargo build -p agentmesh-cli -p agentmesh-multica-selector-shadow
python scripts/package_toolchain_bundle.py \
  --out dist/pr-draft-smoke \
  --target x86_64-pc-windows-msvc \
  --tag v0.0.0-pr-draft.local \
  --commit "$(git rev-parse HEAD)" \
  --bin-dir target/debug \
  --also-flat-phase0-manifest
python scripts/smoke_downloaded_bundle.py \
  --bundle dist/pr-draft-smoke \
  --target x86_64-pc-windows-msvc
```

CI must be green on the three-target matrix after branch push.

## Merge gate (G1)

Human must reply exactly with approval token **`G1`** after reviewing this draft and CI.

**After G1 only:**

1. Commit/push campaign branch (split commits as needed; no secrets).
2. Open/mark PR ready and merge to `main` after review.
3. Dispatch `private-prerelease` with a fresh tag (e.g. `v0.2.0-dev.1`) and `confirm_publish=PUBLISH`, draft=true first if desired.
4. Verify download → `smoke_downloaded_bundle.py` on each target archive.
5. Rehearse previous-pin rollback notes from `docs/release-draft-g1-private-prerelease.md`.

## Create PR later (do not run until G1 + commits exist)

```bash
# AFTER human G1 and AFTER commits/push (not now)
gh pr create -R eiei114/agentmesh-private \
  --base main \
  --head campaign/agentmesh-multica-operations \
  --title "feat(wave2): AgentMesh App manifest, toolchain pin/cache, and private prerelease channel" \
  --body-file docs/pr-draft-g1-agentmesh-multica-operations.md
```

## Pre-G1 safeguards observed this iteration

- No `git commit`
- No `git push`
- No `gh pr create`
- No `gh release create` / workflow dispatch with `PUBLISH`
