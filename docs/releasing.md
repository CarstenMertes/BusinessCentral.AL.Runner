# Releasing

How `alrunner` builds reach testers (ADR-016, "Owner decision (2026-10-15): alpha").
Development stays on `main`. The branch `release` marks what testers get.

Rules:

- `release` is fast-forwarded from `main`. Nobody commits on it directly, except
  for a hotfix (below).
- The `edge` pre-release is the newest build of `release`. Each build replaces
  the assets of the one release named `edge` and moves its tag.
- A numbered alpha is a tag `v3.*` on a commit of `release`. The workflow creates
  a draft pre-release. A person publishes it.
- Nothing from Microsoft goes into any archive.

## 1. A new edge build

1. Check that CI is green on the `main` commit you want.
2. `git fetch origin`
3. `git push origin origin/main:refs/heads/release`
4. CI runs on `release`. When it passes, the Edge workflow builds the three
   targets, smoke tests them and replaces the `edge` assets.
5. If the push is refused (not a fast-forward), see "Hotfix" below.

## 2. A numbered alpha

1. On `main`, change `version` in `[workspace.package]` of `Cargo.toml`, for
   example to `3.0.0-alpha.2`. Run `cargo check` so `Cargo.lock` follows.
2. Commit and push to `main`. Wait for green CI.
3. `git push origin origin/main:refs/heads/release` (as in section 1).
4. `git fetch origin && git tag v3.0.0-alpha.2 origin/release`
5. `git push origin v3.0.0-alpha.2`
6. The Release workflow checks that the tag equals the crate version and that the
   commit is on `release`. It runs the CI checks, builds, smoke tests and
   attests the archives, then creates a draft pre-release.
7. Read the draft, then publish it on GitHub.

## 3. A hotfix that cannot come from main

1. `git checkout release && git pull --ff-only origin release`
2. Commit the fix, or `git cherry-pick <commit>`.
3. `git push origin release`. The edge build follows when CI passes.
4. For a numbered alpha, bump the version in that commit and tag it as in section 2.
5. Merge back, so `release` can fast-forward again:
   `git checkout main && git pull --ff-only origin main && git merge release && git push origin main`
   This makes a merge commit on `main`.

If the push in section 1 fails with "not a fast-forward", `release` has a hotfix
that `main` does not have. The merge-back in step 5 is missing. Do it, then push again.

## Dry runs

- Release workflow: Actions, "Release", "Run workflow". It builds and packages
  all targets and creates no release.
- Edge workflow: the same, from "Edge".

## Open points for the owner

- Branch protection on `release` (no force push, no deletion) keeps it a
  fast-forward-only branch. The repository settings are not changed from here.
- Protect the tag pattern `v3.*` and the tag `edge` the same way, if wanted.
