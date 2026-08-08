# Repair request: sync local main branch with origin

Multica Issues: DOT-974, DOT-1379, DOT-1411

## Failure code

- `agentmesh:repair:repo_main_behind`

## First repair candidate

The maintenance inspector reported that the shared local `main` branch was behind
`origin/main`, which can cause later repair or request generation to inspect stale
repository state. DOT-1411 was materialized from the same repair family after the
inspector observed `ahead=1` and `behind=11`; any `behind>0` result is
repair-required evidence until a sync/check record clears `repo_main_behind`.

## Reproducible repair path

Use the maintenance repair helper from the worktree where `refs/heads/main` is
checked out. If `main` is not checked out in any worktree, any repository worktree
can update the ref:

```bash
python scripts/repair_sync_local_main_with_origin.py
```

The helper fetches `origin/main` into `refs/remotes/origin/main`, reports the
local `main` ahead/behind distance, fast-forwards the local `main` ref when it is
a safe ancestor of `origin/main`, and then reports whether `repo_main_behind` is
still present. For post-repair inspection without fast-forwarding or otherwise
mutating `refs/heads/main`, run:

```bash
python scripts/repair_sync_local_main_with_origin.py --check
```

`--check` still runs a fetch equivalent to:

```bash
git fetch --prune origin +refs/heads/main:refs/remotes/origin/main
```

`refs/remotes/origin/main` and `FETCH_HEAD` can change while `refs/heads/main`
remains unchanged.

## Reconciliation record

This worktree was checked out from `origin/main` after fetching the repository via
Multica's repository checkout path. The implementation branch starts at
`origin/main`, so the repair PR is generated from the current upstream state rather
than the stale local `main` pointer. DOT-1379 also adds the script above so future
runs can repair and verify the shared local `main` ref instead of relying on an
ad-hoc Git command sequence.

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
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
