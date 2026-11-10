# Repair request: sync local main branch with origin

Multica Issues: DOT-974, DOT-1379, DOT-1394, DOT-1411, DOT-1450, DOT-1466, DOT-1506, DOT-1517, DOT-1555, DOT-1578, DOT-1615, DOT-1641, DOT-1703, DOT-1733, DOT-1742, DOT-1789, DOT-1823, DOT-1841, DOT-1865, DOT-3455

## Failure code

- `agentmesh:repair:repo_main_behind`

## First repair candidate

The maintenance inspector reported that the shared local `main` branch was behind
`origin/main`, which can cause later repair or request generation to inspect stale
repository state. DOT-1411 was materialized from the same repair family after the
inspector observed `ahead=1` and `behind=11`; any `behind>0` result is
repair-required evidence until a sync/check record clears `repo_main_behind`.

## Reproducible repair path

Run the maintenance repair helper from any clean maintenance worktree. When
`refs/heads/main` is not checked out in any worktree, the helper creates a
coordinated temporary worktree, fast-forwards `main` there, removes that worktree
afterward, and reports `fast_forward_temporary_worktree`. If another worktree has
`main` checked out, the helper refuses to update the ref and instructs you to run
the command from that worktree instead.

```bash
python scripts/repair_sync_local_main_with_origin.py
```

The helper fetches `origin/main` into `refs/remotes/origin/main`, reports the
local `main` ahead/behind distance, fast-forwards the local `main` ref when it is
a safe ancestor of `origin/main`, and then reports whether `repo_main_behind` is
still present. For post-repair inspection without fetching or mutating any refs,
run:

```bash
python scripts/repair_sync_local_main_with_origin.py --check
```

`--check` compares the existing `refs/heads/main` and `refs/remotes/origin/main`
refs without running `git fetch` or changing any refs.

## Uncommitted local changes

The helper classifies every uncommitted local change against the paths the
incoming commits touch, instead of refusing the whole repair whenever the
worktree is dirty:

- `local_changes=none` — nothing uncommitted; the fast-forward proceeds as before.
- `local_changes=preserved` — local changes exist only on paths the incoming
  commits leave alone. The helper records their count and paths, fast-forwards,
  and then compares a before/after snapshot of those paths. What is compared
  depends on the path class: a modified tracked file by its content digest, a
  modified tracked symlink by its link target (never by the bytes of its target),
  a modified submodule by its recorded commit and its own `git status` output
  (never by reading the directory), and an untracked or ignored entry by presence
  only, so a large local build directory is not rehashed. A worktree fast-forward
  then reports `repair_action=fast_forward_worktree_preserving_local_changes`.
  The temporary-worktree path reports `fast_forward_temporary_worktree` and takes
  no snapshot at all: the update lands in a pristine checkout of that branch, so
  nothing in this worktree is touched and there is nothing to verify. Nothing is
  stashed, reset, or discarded on either path.
- `local_changes=collision` — at least one local path is also changed by the
  incoming commits. The helper reports `repair_action=refused_local_changes_collide`
  plus `collision_paths`, leaves `refs/heads/main` where it was, and explains what
  to move, commit, or stash. Git itself would abort for a tracked or untracked
  overlap, but two ignored cases are silent: an ignored local file at a path the
  incoming commits add, and an ignored local file (or any other non-directory
  entry) that blocks an incoming directory. Git replaces both without a warning,
  so the helper checks every incoming path and its ancestors on disk and treats
  those as collisions too. The same worktree check drives `--check`, so a
  pre-repair inspection never reports a collision that the repair path would not
  honour.

The `request_action` line keeps its existing contract: `seed_app_requests`
requires an aligned refs **and** a clean worktree, so a repair that preserved local
changes still prints `request_action=repair_first` together with
`worktree_state=dirty`. That is a statement about the checkout, not about the sync:
`repo_main_behind=absent` and `repo_main_aligned=yes` are what clears the drift.

## PR evidence checklist

For each recurrence, capture these bounded reports in the repair PR so reviewers
can verify request generation resumed from the current upstream state:

1. Pre-repair inspection with `python scripts/repair_sync_local_main_with_origin.py --check`, including the reported `before_behind` / `after_behind` distance and `repo_main_behind=present` when behind drift triggered the repair; an already-aligned no-op path may record `repo_main_behind=absent`.
2. Repair run with `python scripts/repair_sync_local_main_with_origin.py`, including `repair_action`, `after_behind=0`, `repo_main_behind=absent`, and `repo_main_aligned=yes`.
3. Post-repair inspection with `python scripts/repair_sync_local_main_with_origin.py --check`, confirming `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, and `repo_main_aligned=yes`.
4. The local-only AgentMesh repository checks: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`. For CI matrix parity, Clippy and workspace tests run per target as `cargo clippy --workspace --all-targets --target <matrix.target> -- -D warnings` and `cargo test --workspace --target <matrix.target>`.

Do not resume daily request materialization from a repaired workspace until the
post-repair inspection reports `repo_main_aligned=yes`.

## Reconciliation record

This worktree was checked out from `origin/main` after fetching the repository via
Multica's repository checkout path. The implementation branch starts at
`origin/main`, so the repair PR is generated from the current upstream state rather
than the stale local `main` pointer. DOT-1379 also adds the script above so future
runs can repair and verify the shared local `main` ref instead of relying on an
ad-hoc Git command sequence.

## DOT-1555 execution record

- request_id: `DOT-1555`
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-08-17-repair-local-maintenance-repo-branch-behind-origin-main.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-08-17-repair-local-maintenance-repo-branch-behind-origin-main.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-08-17-repair-local-maintenance-repo-branch-behind-origin-main.md`
- stable scope: `agentmesh:repair:repo_main_behind:v9`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- pre-repair check: `before_ahead=0`, `before_behind=1`, `after_behind=1`, `repo_main_behind=present`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=1`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`
- repair path: `refs/heads/main` was not checked out in any worktree, so the helper created a temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/codex-release-engineer/9fc5dfc3e08f`, `current_head=4416f08fae5fc40a82ed00af90f5f22486703b2e`, `branch=main`, `head=4416f08fae5fc40a82ed00af90f5f22486703b2e`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1578 execution record

- request_id: `DOT-1578`
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-08-22-repair-local-maintenance-branch-behind-origin-main.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-08-22-repair-local-maintenance-branch-behind-origin-main.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-08-22-repair-local-maintenance-branch-behind-origin-main.md`
- stable scope: `agentmesh:repair:repo_main_behind:v11`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- trigger evidence: DOT-1578 was materialized from a `repo_main_behind` report that said the local maintenance branch was 8 commits behind `origin/main`.
- live pre-repair check: `before_ahead=0`, `before_behind=0`, `after_behind=0`, `repo_main_behind=absent`, `request_action=seed_app_requests`
- repair run: `before_ahead=0`, `before_behind=0`, `repair_action=already_aligned`, `after_ahead=0`, `after_behind=0`
- repair path: no branch movement was required; DOT-1578 reached implementation after DOT-1517 had already fast-forwarded the shared local `main` ref and merged the evidence PR.
- post-repair repo metadata: `current_branch=agent/codex-release-engineer/30570e6a1116`, `current_head=4416f08fae5fc40a82ed00af90f5f22486703b2e`, `branch=main`, `head=4416f08fae5fc40a82ed00af90f5f22486703b2e`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1733 execution record

- request_id: `DOT-1733`
- executed_at: `2026-09-06` (JST)
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-09-06-synchronize-local-maintenance-main-with-origin-main.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-09-06-synchronize-local-maintenance-main-with-origin-main.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-09-06-synchronize-local-maintenance-main-with-origin-main.md`
- stable scope: `agentmesh:repair:repo_main_behind:v13`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- preserved local state before repair: `refs/heads/main=85cc51b75f4b36e70726b45497eb361910296bff`, `refs/remotes/origin/main=014931466b58e2f92c6a4cf0a6c8c368b45dc4e9`, `before_ahead=0`, `before_behind=1`, `dirty_count=0`; no uncommitted working-tree file modification was present in the issue worktree, and the fast-forward kept the previous main tip recoverable through Git reflog instead of force-resetting.
- pre-repair check: `repair_action=check_only`, `before_ahead=0`, `before_behind=1`, `after_behind=1`, `repo_main_behind=present`, `repo_main_aligned=no`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=1`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`
- repair path: `refs/heads/main` was not checked out in any worktree, so the helper created a coordinated temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/cursor-composer-builder/77ba37305078`, `current_head=014931466b58e2f92c6a4cf0a6c8c368b45dc4e9`, `branch=main`, `head=014931466b58e2f92c6a4cf0a6c8c368b45dc4e9`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1742 execution record

- request_id: `DOT-1742`
- executed_at: `2026-09-07` (JST)
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-09-07-synchronize-local-main-with-origin-main-safely.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-09-07-synchronize-local-main-with-origin-main-safely.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-09-07-synchronize-local-main-with-origin-main-safely.md`
- stable scope: `agentmesh:agentmesh-private:repo-main-behind`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- preserved local state before repair: `refs/heads/main=014931466b58e2f92c6a4cf0a6c8c368b45dc4e9`, `refs/remotes/origin/main=8f3091caa32f586bf4bc47f4c2d11ff30da4977a`, `before_ahead=0`, `before_behind=1`, `dirty_count=0`; no uncommitted working-tree file modification was present in the issue worktree, and the fast-forward kept the previous main tip recoverable through Git reflog instead of force-resetting.
- pre-repair check: `repair_action=check_only`, `before_ahead=0`, `before_behind=1`, `after_behind=1`, `repo_main_behind=present`, `repo_main_aligned=no`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=1`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`
- repair path: `refs/heads/main` was not checked out in any worktree, so the helper created a coordinated temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/cursor-composer-builder-2/0d65f9bb1842`, `current_head=8f3091caa32f586bf4bc47f4c2d11ff30da4977a`, `branch=main`, `head=8f3091caa32f586bf4bc47f4c2d11ff30da4977a`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1865 execution record

- request_id: `DOT-1865`
- executed_at: `2026-09-14` (JST)
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-09-14-synchronize-local-main-with-origin-main-safely-follow-up.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-09-14-synchronize-local-main-with-origin-main-safely-follow-up.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-09-14-synchronize-local-main-with-origin-main-safely-follow-up.md`
- stable scope: `agentmesh:repair:repo_main_behind:v17`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- preserved local state before repair: `refs/heads/main=3055f96dbe987e502d0039bd004305a789b462c1`, `refs/remotes/origin/main=43e989101083fcefe9c29b4cffafc50630a7865d`, `before_ahead=0`, `before_behind=63`, `dirty_count=0`; no uncommitted working-tree file modification was present in the issue worktree, and the fast-forward kept the previous main tip recoverable through Git reflog instead of force-resetting.
- pre-repair check: `repair_action=check_only`, `before_ahead=0`, `before_behind=63`, `after_behind=63`, `repo_main_behind=present`, `repo_main_aligned=no`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=63`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`
- repair path: `refs/heads/main` was not checked out in any worktree, so the helper created a coordinated temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/cursor-composer-builder-2/8470a6874b77`, `current_head=43e989101083fcefe9c29b4cffafc50630a7865d`, `branch=main`, `head=43e989101083fcefe9c29b4cffafc50630a7865d`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1841 execution record

- request_id: `DOT-1841`
- executed_at: `2026-09-13` (JST)
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-09-13-synchronize-local-main-with-origin-main-safely.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-09-13-synchronize-local-main-with-origin-main-safely.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-09-13-synchronize-local-main-with-origin-main-safely.md`
- stable scope: `agentmesh:repair:repo_main_behind:v16`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- preserved local state before repair: `refs/heads/main=7b82d59d40111c399e3c1e38f82cf887ee7b8a38`, `refs/remotes/origin/main=d4d8d31f50ba1fa4cbf2d1586094ae9816061adc`, `before_ahead=0`, `before_behind=2`, `dirty_count=0`; no uncommitted working-tree file modification was present in the issue worktree, and the fast-forward kept the previous main tip recoverable through Git reflog instead of force-resetting.
- pre-repair check: `repair_action=check_only`, `before_ahead=0`, `before_behind=2`, `after_behind=2`, `repo_main_behind=present`, `repo_main_aligned=no`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=2`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`
- repair path: `refs/heads/main` was not checked out in any worktree, so the helper created a coordinated temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/cursor-composer-builder/5170609bde44`, `current_head=d4d8d31f50ba1fa4cbf2d1586094ae9816061adc`, `branch=main`, `head=d4d8d31f50ba1fa4cbf2d1586094ae9816061adc`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1823 execution record

- request_id: `DOT-1823`
- executed_at: `2026-09-12` (JST)
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-09-12-synchronize-local-main-with-origin-main-safely.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-09-12-synchronize-local-main-with-origin-main-safely.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-09-12-synchronize-local-main-with-origin-main-safely.md`
- stable scope: `agentmesh:repair:repo_main_behind:v15`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- preserved local state before repair: `refs/heads/main=ea7c7d30cebfc07a895caa1755ab8bf54f871048`, `refs/remotes/origin/main=7b82d59d40111c399e3c1e38f82cf887ee7b8a38`, `before_ahead=0`, `before_behind=5`, `dirty_count=0`; no uncommitted working-tree file modification was present in the issue worktree, and the fast-forward kept the previous main tip recoverable through Git reflog instead of force-resetting.
- pre-repair check: `repair_action=check_only`, `before_ahead=0`, `before_behind=5`, `after_behind=5`, `repo_main_behind=present`, `repo_main_aligned=no`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=5`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`
- repair path: `refs/heads/main` was not checked out in any worktree, so the helper created a coordinated temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/cursor-composer-builder/ab322629446f`, `current_head=7b82d59d40111c399e3c1e38f82cf887ee7b8a38`, `branch=main`, `head=7b82d59d40111c399e3c1e38f82cf887ee7b8a38`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1789 execution record

- request_id: `DOT-1789`
- executed_at: `2026-09-09` (JST)
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-09-09-synchronize-local-main-with-origin-main-safely.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-09-09-synchronize-local-main-with-origin-main-safely.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-09-09-synchronize-local-main-with-origin-main-safely.md`
- stable scope: `agentmesh:repair:repo_main_behind:v14`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- preserved local state before repair: `refs/heads/main=8f3091caa32f586bf4bc47f4c2d11ff30da4977a`, `refs/remotes/origin/main=ea7c7d30cebfc07a895caa1755ab8bf54f871048`, `before_ahead=0`, `before_behind=2`, `dirty_count=0`; no uncommitted working-tree file modification was present in the issue worktree, and the fast-forward kept the previous main tip recoverable through Git reflog instead of force-resetting.
- pre-repair check: `repair_action=check_only`, `before_ahead=0`, `before_behind=2`, `after_behind=2`, `repo_main_behind=present`, `repo_main_aligned=no`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=2`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`
- repair path: `refs/heads/main` was not checked out in any worktree, so the helper created a coordinated temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/cursor-composer-builder/0b53596cd921`, `current_head=ea7c7d30cebfc07a895caa1755ab8bf54f871048`, `branch=main`, `head=ea7c7d30cebfc07a895caa1755ab8bf54f871048`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1703 execution record

- request_id: `DOT-1703`
- executed_at: `2026-09-03` (JST)
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-09-03-synchronize-local-maintenance-main-with-origin-main.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-09-03-synchronize-local-maintenance-main-with-origin-main.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-09-03-synchronize-local-maintenance-main-with-origin-main.md`
- stable scope: `agentmesh:repair:repo_main_behind:v12`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- preserved local state before repair: `refs/heads/main=8d8a979ebb01e6e046933e7127548b23fcf14298`, `refs/remotes/origin/main=85cc51b75f4b36e70726b45497eb361910296bff`, `before_ahead=0`, `before_behind=5`, `dirty_count=0`; no uncommitted working-tree file modification was present in the issue worktree, and the fast-forward kept the previous main tip recoverable through Git reflog instead of force-resetting.
- pre-repair check: `repair_action=check_only`, `before_ahead=0`, `before_behind=5`, `after_behind=5`, `repo_main_behind=present`, `repo_main_aligned=no`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=5`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`
- repair path: `refs/heads/main` was not checked out in any worktree, so the helper created a coordinated temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/cursor-composer-builder/898868808051`, `current_head=85cc51b75f4b36e70726b45497eb361910296bff`, `branch=main`, `head=85cc51b75f4b36e70726b45497eb361910296bff`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1641 execution record

- request_id: `DOT-1641`
- executed_at: `2026-08-30` (JST; matches commit `54ce7b3` author date)
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-08-30-synchronize-the-local-main-branch-with-origin-main.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-08-30-synchronize-the-local-main-branch-with-origin-main.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-08-30-synchronize-the-local-main-branch-with-origin-main.md`
- stable scope: `agentmesh:repair.repo-main-behind`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- preserved local state before repair: `refs/heads/main=b714e30096ce67f7489754c8f4bdcd2e043b9f37`, `refs/remotes/origin/main=8d8a979ebb01e6e046933e7127548b23fcf14298`, `before_ahead=0`, `before_behind=3`, `dirty_count=0`; no uncommitted working-tree file modification was present in the issue worktree, and the fast-forward kept the previous main tip recoverable through Git reflog instead of force-resetting.
- pre-repair check: `repair_action=check_only`, `before_ahead=0`, `before_behind=3`, `after_behind=3`, `repo_main_behind=present`, `repo_main_aligned=no`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=3`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`
- repair path: `refs/heads/main` was not checked out in any worktree, so the helper created a coordinated temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/cursor-composer-builder/9d2c695bd75b`, `current_head=8d8a979ebb01e6e046933e7127548b23fcf14298`, `branch=main`, `head=8d8a979ebb01e6e046933e7127548b23fcf14298`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1615 execution record

- request_id: `DOT-1615`
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-08-27-synchronize-the-local-agentmesh-private-main-branch-with.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-08-27-synchronize-the-local-agentmesh-private-main-branch-with.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-08-27-synchronize-the-local-agentmesh-private-main-branch-with.md`
- stable scope: `agentmesh:repo-main-behind`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- preserved local state before repair: `refs/heads/main=4416f08fae5fc40a82ed00af90f5f22486703b2e`, `refs/remotes/origin/main=b714e30096ce67f7489754c8f4bdcd2e043b9f37`, `before_ahead=0`, `before_behind=4`, `dirty_count=0`; no uncommitted working-tree file modification was present in the issue worktree, and the fast-forward kept the previous main tip recoverable through Git reflog instead of force-resetting.
- pre-repair check: `repair_action=check_only`, `before_ahead=0`, `before_behind=4`, `after_behind=4`, `repo_main_behind=present`, `repo_main_aligned=no`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=4`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`
- repair path: `refs/heads/main` was not checked out in any worktree, so the helper created a coordinated temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/codex-release-engineer/2f99d5457c77`, `current_head=b714e30096ce67f7489754c8f4bdcd2e043b9f37`, `branch=main`, `head=b714e30096ce67f7489754c8f4bdcd2e043b9f37`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1517 execution record

- request_id: `DOT-1517`
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-08-24-repair-local-maintenance-branch-drift-against-origin-main.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-08-24-repair-local-maintenance-branch-drift-against-origin-main.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-08-24-repair-local-maintenance-branch-drift-against-origin-main.md`
- stable scope: `agentmesh:repair:repo_main_behind:v10`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- pre-repair check: `before_ahead=0`, `before_behind=8`, `after_behind=8`, `repo_main_behind=present`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=8`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`
- repair path: `refs/heads/main` was not checked out in any worktree, so the helper created a temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/codex-release-engineer/1dd0dee2d7db`, `current_head=6eefc46d3841a54906f9ff20e55ee58c2c3f01e0`, `branch=main`, `head=6eefc46d3841a54906f9ff20e55ee58c2c3f01e0`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1506 execution record

- request_id: `DOT-1506`
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-08-13-repair-local-agentmesh-maintenance-branch-behind-origin.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-08-13-repair-local-agentmesh-maintenance-branch-behind-origin.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-08-13-repair-local-agentmesh-maintenance-branch-behind-origin.md`
- stable scope: `agentmesh:repair:repo-main-behind:v7`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- pre-repair check: `before_ahead=0`, `before_behind=1`, `after_behind=1`, `repo_main_behind=present`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=1`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`
- repair path: `refs/heads/main` was not checked out in any existing worktree, so the helper created a temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/codex-release-engineer/86e988b0`, `current_head=2820cba047a569a788948feaebae2651c77aa1b7`, `branch=main`, `head=2820cba047a569a788948feaebae2651c77aa1b7`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-1466 execution record

- request_id: `DOT-1466`
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-08-11-repair-local-agentmesh-maintenance-branch-behind-origin-v6.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-08-11-repair-local-agentmesh-maintenance-branch-behind-origin-v6.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-08-11-repair-local-agentmesh-maintenance-branch-behind-origin-v6.md`
- stable scope: `agentmesh:repair:repo-main-behind:v6`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- pre-repair check: `before_ahead=0`, `before_behind=2`, `repo_main_behind=present`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=2`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`
- repair path: `refs/heads/main` was not checked out in any existing worktree, so the helper created a temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left the current issue worktree clean.
- post-repair repo metadata: `current_branch=agent/codex-release-engineer/12c627b4`, `current_head=5f0ea4e40e538c4117b582316013e905a6a9a7e4`, `branch=main`, `head=5f0ea4e40e538c4117b582316013e905a6a9a7e4`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

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

## DOT-1450 execution record

- request_id: `DOT-1450`
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-08-11-repair-local-agentmesh-maintenance-branch-behind-origin.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-08-11-repair-local-agentmesh-maintenance-branch-behind-origin.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-08-11-repair-local-agentmesh-maintenance-branch-behind-origin.md`
- stable scope: `agentmesh:repair:repo_main_behind:v5`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- pre-repair check: `before_ahead=0`, `before_behind=13`, `repo_main_behind=present`, `request_action=repair_first`
- repair run: `before_ahead=0`, `before_behind=13`, `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`
- repair path: `refs/heads/main` was not checked out in any existing worktree, so the helper created a temporary worktree, fast-forwarded `main` with `--ff-only`, removed the temporary worktree afterward, and left `git worktree list` clean
- post-repair repo metadata: `current_branch=agent/codex-release-engineer/39eaa8ca`, `current_head=c77e3f79cecc83ca52c65df0da3eed3e1ad3a973`, `branch=main`, `head=c77e3f79cecc83ca52c65df0da3eed3e1ad3a973`, `ahead=0`, `behind=0`, `dirty_count=0`
- post-repair check: `repair_action=check_only`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`

## DOT-2048 execution record

- request_id: `DOT-2048`
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-09-28-main.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-09-28-main.md`
- audited branch: `main`
- preserved local state before repair: `refs/heads/main=d4d8d31f50ba1fa4cbf2d1586094ae9816061adc`, `refs/remotes/origin/main=345b8208d6e2dffaf88d1b36478c204048324a44`, `before_ahead=0`, `before_behind=13`, `dirty_count=0`
- pre-repair check: `repair_action=check_only`, `before_ahead=0`, `before_behind=13`, `repo_main_behind=present`, `repo_main_aligned=no`, `request_action=repair_first`
- repair run: `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`
- repair path: `refs/heads/main` was not checked out in any worktree, so the helper created a temporary worktree, fast-forwarded `main` with `--ff-only`, and removed the temporary worktree afterward.
- post-repair check: `repair_action=check_only`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`
- preserved working state: the issue worktree remained clean throughout; no uncommitted changes were lost.

## DOT-2114 execution record

- request_id: `DOT-2114`
- audited branch: `main`
- preserved local state before repair: `refs/heads/main=345b8208d6e2dffaf88d1b36478c204048324a44`, `refs/remotes/origin/main=2f2599e6b9d0bc70c6e1db53fcd8ecc1adc6610b`, `before_ahead=0`, `before_behind=2`, `dirty_count=0`
- pre-repair check: `repair_action=check_only`, `after_behind=2`, `repo_main_behind=present`, `repo_main_aligned=no`, `request_action=repair_first`
- repair run: `repair_action=fast_forward_temporary_worktree`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`
- repair path: `refs/heads/main` was not checked out in a worktree, so the helper used a coordinated temporary worktree and removed it after the fast-forward.
- post-repair check: `repair_action=check_only`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `request_action=seed_app_requests`
- preserved working state: the issue worktree remained clean; no local changes were discarded.

## DOT-3455 execution record

- request_id: `DOT-3455`
- executed_at: `2026-11-09` (JST)
- source request: `4_Project/OSS/agentmesh-private/Requests/Repair/2026-11-09-re-synchronize-the-local-agentmesh-maintenance-main-bran.md`
- derived issue: `4_Project/OSS/agentmesh-private/Issues/2026-11-09-re-synchronize-the-local-agentmesh-maintenance-main-bran.md`
- dedupe key: `agentmesh-private:4_Project/OSS/agentmesh-private/Issues/2026-11-09-re-synchronize-the-local-agentmesh-maintenance-main-bran.md`
- stable scope: `agentmesh:repair:repo_main_behind:v18`
- request status: `ready_for_multica=true`, `status=ready`
- audited branch: `main`
- preserved local state before repair: `refs/heads/main=ecfbe29720ba2b37b57aeac18da25dae92a70bf7`, `refs/remotes/origin/main=db0dc8bd59266fdcf029676d9a76e368bfdc27b6`, `before_ahead=0`, `before_behind=22`, `dirty_count=6` (3 modified tracked files, 1 untracked directory, 1 untracked `__pycache__`, 1 untracked toolchain pin, plus ignored `.agentmesh/`, `.pi/`, `.ralph/`, `dist/`, `target/`). This is the first recurrence where the maintenance worktree itself had uncommitted changes, so the historical dirty-worktree refusal would have blocked the sync again.
- pre-repair check: `repair_action=check_only`, `before_ahead=0`, `before_behind=22`, `after_behind=22`, `repo_main_behind=present`, `repo_main_aligned=no`, `request_action=repair_first`, `local_changes=preserved`, `preserved_local_count=798`
- repair run: `before_ahead=0`, `before_behind=22`, `repair_action=fast_forward_worktree_preserving_local_changes`, `after_local=db0dc8bd59266fdcf029676d9a76e368bfdc27b6`, `after_ahead=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `local_changes=preserved`, `worktree_state=dirty`
- repair path: `refs/heads/main` **was** checked out in this maintenance worktree, so the helper took the worktree path instead of the temporary-worktree path. It classified all local changes against the 72 paths the incoming 22 commits touch, found no overlap, snapshotted the three modified tracked files by SHA-256, ran `git merge --ff-only origin/main`, and re-read those paths to confirm the digests were identical. Nothing was stashed, reset, checked out, or force-pushed; the previous main tip stays reachable through `HEAD@{1}`.
- post-repair check: `repair_action=check_only`, `before_behind=0`, `after_behind=0`, `repo_main_behind=absent`, `repo_main_aligned=yes`, `local_changes=preserved`, `worktree_state=dirty`, `request_action=repair_first` (the worktree is legitimately still dirty; the sync itself is complete)
- post-repair maintenance inspector: `repair_candidates=[]`, `repo.ahead=0`, `repo.behind=0`, `request_action=seed_app_request`, `reason=daily_app_request_missing` — the `repo_main_behind` candidate this scope tracked is gone.
- preserved content check after repair: `crates/agentmesh-evidence/src/discovery.rs=58147c6e5915b51ef7c67ce2df3243bad3ab0192bf175bcf4482d5ab2f0534d4`, `crates/agentmesh-host/src/process.rs=3871ee97513c9312bf6306d394d87f03f0fb4a5bb0e92f608cd65f81e1044501`, `toolchains/agentmesh-v0.2.0-dev.2.x86_64-pc-windows-msvc.toml=506966e7899cd646e49c9062ca103215969614a531cd48e2b35777c7ad78ccda`, `toolchains/agentmesh-v0.2.0-dev.5.x86_64-pc-windows-msvc.toml=b88b80e6fed94165e01ba6357d95b3bf5008349d69dc1d72c34943617bd49236` — byte-identical to the pre-repair hashes, and `git stash list` is empty.
- helper change: the previous helper refused any worktree with uncommitted changes (`dirty_count != 0`) even when none of those paths were touched by the incoming commits, which is why this drift kept re-triggering the repair-first branch. The helper now classifies local changes into `preserved` / `collision`, refuses only real overlaps, reports both cases in the existing key=value report, and verifies preserved content after the fast-forward.

Release tags, package publishing, assets, secrets, permissions, production actions,
and Multica authority changes are intentionally out of scope for this repair.

## Verification

- `git status --short --branch`
- `python scripts/repair_sync_local_main_with_origin.py --check`
- Local-only checks: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`
- CI matrix checks: `cargo clippy --workspace --all-targets --target <matrix.target> -- -D warnings` and `cargo test --workspace --target <matrix.target>`
