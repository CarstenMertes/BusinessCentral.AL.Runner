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
5. **Seams change only through an ADR** in `docs/adr/`.

## Workflow

- After making corpus cases pass: `cargo run -p al-test-cli -- --update-baseline`
  and commit `metrics/passing.txt` with the code.
- `./check.sh` before every commit (fmt, clippy -D warnings, tests; fails on the first error).
- Core crates do no OS access (ADR-006).
