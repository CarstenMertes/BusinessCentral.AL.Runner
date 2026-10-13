# Agent instructions

Read `docs/handoff.md` (the spike brief) and `docs/adr/README.md` first.

## Hard rules

1. **Red/green TDD, no exceptions.** Every behaviour starts as a failing test;
   every bug fix starts with a test that reproduces it. Show the red run before
   the green one. Nothing merges with `cargo test` red. Corpus cases that are
   not yet implemented are tracked by the pass-rate report, not by tests.
2. **Never weaken an oracle.** Never edit the corpus submodule, `oracle/`, or
   remove lines from `metrics/passing.txt` to make CI green. If real BC and the
   corpus disagree, open an issue on the corpus repo.
3. **No guessing at semantics.** Unclear behaviour → write a probe test on a
   branch of the corpus repo, read the result from its CI (ADR-007), get it
   merged there, then bump the submodule + oracle and implement.
4. **Unsupported means a compile-time error.** A builtin that is not
   implemented fails in `al-sema` with a diagnostic naming it, never at runtime.
5. **Seams change only through an ADR** in `docs/adr/`. Draft it as
   Proposed and ask the project owner (AskUserQuestion) to accept it before
   building on it.
6. **Match BC where it is observable; improve on it everywhere else**
   (ADR-009). Values, errors, database state, trigger and event order, host
   interaction and whether code compiles must match real BC. Internals,
   performance, determinism, diagnostics and tooling are free to be better.
   Never reject code BC accepts, and never "fix" BC behaviour AL code can see.

## Workflow

- After making corpus cases pass: `cargo run -p al-test-cli -- --update-baseline`
  and commit `metrics/passing.txt` with the code.
- Perf budgets (cold compile of the corpus <= 1 s, parser >= 50 MB/s) are
  `#[ignore]`d tests that need release: `cargo test --release --no-fail-fast
  -p al-syntax --test perf_parse_throughput -p al-test-cli --test perf_budget
  -- --ignored --nocapture`. CI runs them. `cargo run --release -p al-test-cli
  --example phases` prints per-phase timings; `--example compile_dump` dumps
  every compile output, to diff before/after a performance change.
- `./check.sh` before every commit (fmt, clippy -D warnings, tests; fails on the first error).
- Core crates do no OS access (ADR-006).
