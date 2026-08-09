# Repair request: sync local main branch with origin

Multica Issues: DOT-974, DOT-1379, DOT-1394, DOT-1411, DOT-1422

## Failure code

- `agentmesh:repair:repo_main_behind`

## First repair candidate

The maintenance inspector reported that the shared local `main` branch was behind
`origin/main`, which can cause later repair or request generation to inspect stale
repository state. DOT-1411 was materialized from the same repair family after the
inspector observed `ahead=1` and `behind=11`; any `behind>0` result is
repair-required evidence until a sync/check record clears `repo_main_behind`.

## Reproducible repair path

Run the maintenance repair helper from any clean repository worktree. If
`refs/heads/main` is checked out in another worktree, the helper refuses to
update the ref and instructs you to run the command from that worktree instead.
When `main` is not checked out anywhere, the helper asks Git to check it out in
a temporary worktree and performs an `--ff-only` merge there. Git's worktree
checkout coordination prevents an ordinary concurrent checkout from racing the
update, and the temporary worktree is removed afterward.

```bash
python scripts/repair_sync_local_main_with_origin.py
```

The helper serializes repair runs with a repository-local lock whose published
owner identity is protected by an OS lock. A crashed owner is reclaimed after
the kernel releases that lock. It then fetches `origin/main` into
`refs/remotes/origin/main`, reports the local `main` ahead/behind distance,
fast-forwards the local `main` only when it is a safe ancestor of `origin/main`
and `behind>0` was observed, and reports `dirty_count`, `repo_main_behind`,
`repo_main_aligned`, and the derived `request_action`. For post-repair inspection
without fetching or mutating any refs, run:

```bash
python scripts/repair_sync_local_main_with_origin.py --check
```

`--check` compares the existing `refs/heads/main` and `refs/remotes/origin/main`
refs without running `git fetch` or changing any refs.

## PR evidence checklist

For each recurrence, capture these bounded reports in the repair PR so reviewers
can verify request generation resumed from the current upstream state:

1. Pre-repair inspection with `python scripts/repair_sync_local_main_with_origin.py --check`, including the reported `current_branch`, `current_head`, `dirty_count`, `before_behind` / `after_behind` distance, `repo_main_behind=present`, and `request_action=repair_first`.
2. Repair run with `python scripts/repair_sync_local_main_with_origin.py`, including `repair_action`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`, and `request_action=seed_app_requests`.
3. Post-repair inspection with `python scripts/repair_sync_local_main_with_origin.py --check`, confirming `dirty_count=0`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`, and `request_action=seed_app_requests`.
4. The focused Python checks: `python -m py_compile scripts/repair_sync_local_main_with_origin.py scripts/test_repair_sync_local_main_with_origin.py` and `python -m unittest scripts/test_repair_sync_local_main_with_origin.py`.
5. The local-only AgentMesh repository checks: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`. For CI matrix parity, Clippy and workspace tests run per target as `cargo clippy --workspace --all-targets --target <matrix.target> -- -D warnings` and `cargo test --workspace --target <matrix.target>`.

Do not resume daily request materialization from a repaired workspace until the
post-repair inspection reports `repo_main_aligned=yes`.

## Reconciliation record

This worktree was checked out from `origin/main` after fetching the repository via
Multica's repository checkout path. The implementation branch starts at
`origin/main`, so the repair PR is generated from the current upstream state rather
than the stale local `main` pointer. DOT-1379 also adds the script above so future
runs can repair and verify the shared local `main` ref instead of relying on an
ad-hoc Git command sequence.

## DOT-1422 execution record

- request_id: `DOT-1422`
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-08-09-repair-local-agentmesh-maintenance-repo-main-branch-behi.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-08-09-repair-local-agentmesh-maintenance-repo-main-branch-behi.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-08-09-repair-local-agentmesh-maintenance-repo-main-branch-behi.md`
- stable scope: `agentmesh:repair:repo-main-behind`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- repair worktree branch: `agent/DOT-1422-main-sync-repair`
- repair worktree head: `9108a39cd3689f2c7b12b1bdd6a477100dfe8e0b`
- pre-repair inspection: `dirty_count=0`, `before_local=92b9d57b83bf6ea53f7cc78c2663f3a721dcc606`, `before_remote=0f8864ac6b41fc4f70dea2c04e51b458850fc858`, `before_ahead=0`, `before_behind=1`, `repo_main_behind=present`, `request_action=repair_first`
- repair run: `dirty_count=0`, `repair_action=fast_forward_ref`, `after_local=0f8864ac6b41fc4f70dea2c04e51b458850fc858`, `after_remote=0f8864ac6b41fc4f70dea2c04e51b458850fc858`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`
- post-repair check: `dirty_count=0`, `repair_action=check_only`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`
- post-review hardening: future unchecked-branch repairs use `repair_action=fast_forward_temporary_worktree`; the historical repair action above remains unchanged.

## DOT-1411 execution record

- request_id: `DOT-1411`
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-08-08-repair-local-agentmesh-maintenance-repo-main-behind-orig.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-08-08-repair-local-agentmesh-maintenance-repo-main-behind-orig.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-08-08-repair-local-agentmesh-maintenance-repo-main-behind-orig.md`
- stable scope: `agentmesh:repair:repo_main_behind:v4`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- audited local head: `60f83470771e229441e1e844273427bd44be6343`
- audited remote head: `60f83470771e229441e1e844273427bd44be6343`
- repair run: `before_ahead=0`, `before_behind=0`, `repair_action=already_aligned`, `after_ahead=0`, `after_behind=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`

Release tags, package publishing, assets, secrets, permissions, production actions,
and Multica authority changes are intentionally out of scope for this repair.

## Verification

- `git status --short --branch`
- `python scripts/repair_sync_local_main_with_origin.py --check`
- Focused Python checks: `python -m py_compile scripts/repair_sync_local_main_with_origin.py scripts/test_repair_sync_local_main_with_origin.py` and `python -m unittest scripts/test_repair_sync_local_main_with_origin.py`
- Local-only checks: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`
- CI matrix checks: `cargo clippy --workspace --all-targets --target <matrix.target> -- -D warnings` and `cargo test --workspace --target <matrix.target>`
