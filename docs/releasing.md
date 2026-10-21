# Releasing

How `alrunner` builds reach testers (ADR-016, "Owner decision (2026-10-15): alpha").
Development stays on `main`. The branch `release` marks what testers get.

Rules:

- `release` is fast-forwarded from `main`. Nobody commits on it directly, except
  for a hotfix (below).
- The `edge` pre-release is the newest build of `release`. Each build replaces
  the assets of the one release named `edge` and moves its tag.
- Every night at 02:17 UTC, `nightly-edge.yml` fast-forwards `release` to the
  newest commit of `main` whose CI passed on a push, and builds `edge` from it.
  It does nothing when `release` already has that commit. It fails, and changes
  nothing, when `release` has commits that `main` lacks (a hotfix without
  merge-back). Numbered alphas stay manual (owner decision 2026-10-20).
- A numbered alpha is a tag `v3.*` on a commit of `release`. The workflow
  `release-tag.yml` creates a normal GitHub release (not a draft, not a
  pre-release), marked "latest". mise's github backend lists only such
  releases, so `mise up` finds them (owner decision 2026-10-20).
- `alrunner --version` tells them apart. Edge shows the build date:
  `alrunner 3.0.0-alpha.2+edge.20261021 (c26a65651)`. A numbered alpha and a
  local build show `alrunner 3.0.0-alpha.2 (c26a65651)`.
- Nothing from Microsoft goes into any archive.

## 1. A new edge build

The nightly run does this by itself. To get a build sooner, run "Nightly edge"
from the Actions tab (workflow_dispatch), or push `release` by hand:

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
6. The Release workflow (`release-tag.yml`) checks that the tag equals the crate
   version and that the commit is on `release`. It runs the CI checks, builds,
   smoke tests and attests the archives, and checks that `alrunner --version`
   prints the tag's version. Then it publishes the release with the archives
   (`alrunner-<version>-<target>.tar.xz` or `.zip`), `SHA256SUMS`, `install.sh`
   and `install.ps1`. The notes are a short fixed text plus the commit list.
7. Check: `mise ls-remote github:aldevtools/BusinessCentral.AL.Runner` lists the version.

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
