# Release Draft — First private prerelease (Gate G1)

> **Status:** DRAFT NOTES ONLY — do not tag/publish until human explicitly answers `G1`.
> **Suggested first tag:** `v0.2.0-dev.1`
> **Workflow:** `.github/workflows/private-prerelease.yml`
> **Prepared:** Ralph iteration 10

## Release intent

Publish the first **immutable** private AgentMesh toolchain prerelease used by Vault Backlog Promoter shadow dogfood:

- `agentmesh` CLI
- `agentmesh-multica-selector-shadow` plugin
- `release-manifest.json` (`agentmesh-release-manifest.v0`)
- app template (`apps/backlog-promoter`) + docs
- generated consumer `pin.generated.toml` identity fields

## Identity tuple (must be recorded after publish)

| Field | Source |
|---|---|
| `tag` | exact prerelease tag (`v0.2.0-dev.1`) |
| `commit_sha` | full 40-char SHA of merged `main` commit |
| `target` | one of three supported triples |
| `release_manifest_sha256` | SHA-256 of that target's `release-manifest.json` |

No mutable `latest`. No asset replacement. If tag/release exists, publish must fail.

## Assets expected (exactly 3)

- `agentmesh-v0.2.0-dev.1-x86_64-pc-windows-msvc.zip`
- `agentmesh-v0.2.0-dev.1-x86_64-unknown-linux-gnu.tar.gz`
- `agentmesh-v0.2.0-dev.1-aarch64-apple-darwin.tar.gz`

## Publish procedure (AFTER G1 only)

```bash
# 1) Confirm absence
python scripts/assert_prerelease_absent.py --tag v0.2.0-dev.1 --json

# 2) Dispatch workflow (from GitHub UI or gh)
gh workflow run private-prerelease.yml \
  -R eiei114/agentmesh-private \
  -f tag=v0.2.0-dev.1 \
  -f confirm_publish=PUBLISH \
  -f draft=true

# 3) Wait for workflow success, then download each asset and smoke
gh release download v0.2.0-dev.1 -R eiei114/agentmesh-private --dir ./dl
# unpack per target, then:
python scripts/smoke_downloaded_bundle.py --bundle ./unpacked-windows --target x86_64-pc-windows-msvc
python scripts/smoke_downloaded_bundle.py --bundle ./unpacked-linux --target x86_64-unknown-linux-gnu
python scripts/smoke_downloaded_bundle.py --bundle ./unpacked-darwin --target aarch64-apple-darwin
```

Prefer `draft=true` first, verify assets/smoke, then undraft manually if desired. Never delete/re-upload to “fix” assets.

## Reinstall verification

```bash
agentmesh toolchain install --bundle ./unpacked-<target> --toolchain-cache ~/.agentmesh/toolchains
# second install must fail (immutable directory)
agentmesh toolchain install --bundle ./unpacked-<target> --toolchain-cache ~/.agentmesh/toolchains
# expect exit 2 / "already exists"
```

## Previous-pin rollback rehearsal (no mutation of Multica)

1. Keep `v0.2.0-dev.1` cache directory intact.
2. Consumer pin file points at `previous_tag` / previous digest when a later tag exists.
3. Rollback drill for G1 acceptance (even for first release): document that installing a second tag never overwrites the first; switching pins only changes which directory resolve uses.
4. Until a second tag exists, rollback rehearsal is: uninstall is **not** supported; retain the installed immutable dir and keep the pin file that matches it. Future releases must set `previous_tag = "v0.2.0-dev.1"`.

## Post-publish consumer pin (Vault later, Wave 3)

After G1 publish succeeds, commit a version-controlled pin (example):

```toml
schema_version = "agentmesh-toolchain-pin.v0"
tag = "v0.2.0-dev.1"
commit_sha = "<FULL_SHA>"
target = "x86_64-pc-windows-msvc"
release_manifest_sha256 = "<DIGEST_FROM_RELEASE_MANIFEST>"
```

Do **not** invent digests before download/smoke.

## Gate blockers until G1

- Campaign changes are still **uncommitted** on `campaign/agentmesh-multica-operations`
- No PR opened yet (body prepared in `docs/pr-draft-g1-agentmesh-multica-operations.md`)
- No tag/release assets published
