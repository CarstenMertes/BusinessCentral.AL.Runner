# ADR-007: Oracle source, emulated version, and the passing-set ratchet

- Status: Accepted. Amended by ADR-015: one oracle and one ratchet per BC major, under `oracle/<major>/` and `metrics/<major>/`.
- Date: 2026-10-13

## Context
The runtime is measured against real BC. The corpus repository already runs
every test on real BC service tiers (BC 27.0 through 29.0) in GitHub Actions
and uploads a JUnit report per version. No BC container is available to agents
in this project's sessions.

## Decision
- **Oracle**: the corpus CI's `junit-test-results-27.5` artifact for the exact
  corpus commit the `corpus` submodule is pinned to, stored in `oracle/`. The
  CLI refuses to run (exit 2) if the pin and `oracle/oracle.toml` disagree.
- **Emulated version**: BC 27.5, the lowest version in the original spike
  matrix. Preprocessor symbols are those the corpus CI defines for 27.5:
  `BC27PLUS`.
- **Probes**: when behaviour is unclear, write the probe as a corpus test on a
  branch of the corpus repo and read its result from that branch's CI run.
  Once merged there, bump the submodule and the oracle together.
- **Ratchet**: `metrics/<major>/passing.txt` lists every corpus case the runtime passes (one file per BC major since ADR-015).
  CI fails if any listed case stops passing. The file only grows, via
  `al-test-cli --update-baseline`; removing a line needs a recorded reason in
  the commit message.

## Consequences
- Oracle refresh is a two-file change (submodule + `oracle/`), easy to review.
- A probe round-trip costs one corpus CI run (~20 min) instead of a local run.

## Conformance
`al-test-cli/tests/corpus_harness.rs`: pin mismatch exits 2; a baseline case
that does not pass exits 1.
