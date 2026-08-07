# Repair request: sync local main branch with origin

Multica Issues: DOT-974, DOT-1379

## Failure code

- `agentmesh:repair:repo_main_behind`

## First repair candidate

The maintenance inspector reported that the shared local `main` branch was behind
`origin/main`, which can cause later repair or request generation to inspect stale
repository state.

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

Release tags, package publishing, assets, secrets, permissions, production actions,
and Multica authority changes are intentionally out of scope for this repair.

## Verification

- `git status --short --branch`
- `python scripts/repair_sync_local_main_with_origin.py --check`
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
