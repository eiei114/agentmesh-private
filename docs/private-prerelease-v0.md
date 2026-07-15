# Private Prerelease Channel (v0)

## Policy

- Private GitHub **prereleases** only (`v0.2.0-dev.N` style).
- Identity tuple: `(tag, full commit SHA, target, release-manifest SHA-256)`.
- **No asset replacement**: never `--clobber`, never delete+recreate a release to upload again.
- If a tag or release already exists, publish must fail (`scripts/assert_prerelease_absent.py`).
- Gate **G1** required before merge/tag/publish for real consumer pins.

## Workflow

`.github/workflows/private-prerelease.yml` (manual `workflow_dispatch`):

1. Guard: `confirm_publish` must equal `PUBLISH`.
2. Guard: assert tag/release absent.
3. Build/package/smoke all three targets.
4. Create **draft** prerelease (default) and upload exactly three archives once.

Do **not** run this workflow until human G1 approval.

## Downloaded smoke (all targets)

After unpacking a zip/tar:

```bash
python scripts/smoke_downloaded_bundle.py \
  --bundle /path/to/unpacked \
  --target x86_64-pc-windows-msvc
```

Checks:

- Phase-0 `manifest.json` hashes (when present)
- `release-manifest.json` ↔ `pin.generated.toml` digest/tag/commit/target
- `agentmesh app validate`
- `agentmesh toolchain install` + overwrite rejection
- `agentmesh app run` (production, pinned) against packaged `testdata/one_candidate.snapshot.json`

CI matrix runs this smoke on Windows/Linux/macOS after packaging.

## Consumer install

```bash
gh release download <tag> --pattern '*<target>*' --dir ./dl
# unpack archive, then:
agentmesh toolchain install --bundle ./unpacked --toolchain-cache ~/.agentmesh/toolchains
# commit pin.generated.toml (or rewrite consumer pin) into version control
```

Rollback = point consumer pin to previous tag (previous immutable cache directory remains).
