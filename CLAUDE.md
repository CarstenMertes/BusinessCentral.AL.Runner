# Agent instructions

al-runtime-rs is a Rust-native AL runtime (see `README.md`). Read
`docs/architecture.md`, `docs/status.md` and `docs/adr/README.md` first.

## Hard rules

1. **Red/green TDD, no exceptions.** Every behaviour starts as a failing test;
   every bug fix starts with a test that reproduces it. Show the red run before
   the green one. Nothing merges with `cargo test` red. Corpus cases that are
   not yet implemented are tracked by the pass-rate report, not by tests.
2. **Never weaken an oracle.** Never edit the corpus submodule or `oracle/`,
   and never remove lines from `metrics/passing.txt` to make CI green. If real
   BC and the corpus disagree, open an issue on the corpus repo. The only
   legitimate removal is a case shown to have passed by accident (for example
   against an unseeded store). Say so explicitly in the commit message.
3. **No guessing at semantics.** Cite the corpus test (file:line) that pins a
   behaviour in a code comment. If nothing pins it, write a probe test on a
   branch of the corpus repo and read the result from its CI (ADR-007), get it
   merged there, then bump the submodule and oracle and implement. Until then,
   record a documentation-only assumption in `docs/probes-needed.md`, or
   refuse. Error texts come from the corpus and are never invented.
4. **Unsupported is refused explicitly, never run silently.** Refuse
   statically wherever possible: a lowering reason, or a diagnostic in
   `al-sema`, so the runner reports the test as not run before it starts.
   What can only be discovered mid-run raises `Signal::Unsupported`. A
   guessed or partial behaviour is never an acceptable stand-in.
5. **Seams change only through an ADR** in `docs/adr/`. Draft it as Proposed
   and ask the project owner (AskUserQuestion) to accept it before building on
   it. Additive changes (a new `Value` variant, an IR field, a defaulted `Ctx`
   method) do not need an ADR; changing or removing a seam does.
6. **Match BC where it is observable; improve on it everywhere else**
   (ADR-009). Values, errors, database state, trigger and event order, host
   interaction and whether code compiles must match real BC. Internals,
   performance, determinism, diagnostics and tooling are free to be better.
   Never reject code BC accepts, and never "fix" BC behaviour AL code can see.
7. **Do not fake objects from dependent apps.** Microsoft's apps are compiled
   from their real source (ADR-013, Proposed). Native code exists only for
   platform objects that have no AL body. Nothing from Microsoft is ever
   committed to this repository.

## Workflow

- Build once in release and use the binary: `cargo build --release`, then
  `./target/release/al-test-cli --root .` (add `--junit FILE` for per-test
  outcomes and refusal reasons).
- After making corpus cases pass: `./target/release/al-test-cli --root .
  --update-baseline` and commit `metrics/passing.txt` with the code. The
  update refuses while any baseline case regresses.
- `./check.sh` before every commit: fmt, clippy -D warnings, all tests, and the
  corpus ratchet. It fails on the first error.
- Perf budgets (cold compile of the corpus ≤ 1 s, parser ≥ 50 MB/s) are
  `#[ignore]`d tests that need release:
  `cargo test --release --no-fail-fast -p al-syntax --test perf_parse_throughput -p al-test-cli --test perf_budget -- --ignored --nocapture`.
  CI runs them. `cargo run --release -p al-test-cli --example phases` prints
  timings per phase. `--example compile_dump` dumps every compile output, to
  diff before and after a performance change.
- Core crates do no OS access (ADR-006).
- Parallel work: one `git worktree` and branch per area
  (`git worktree add ../wt-<area> -b <area> main`). Merge `main` into the branch
  before handing it back, and regenerate the baseline after merging; never
  hand-merge `metrics/passing.txt`.
- Shell hygiene: never build a file list with `$(grep -l …)` that can come back
  empty. `grep PATTERN` with no files waits on stdin forever; use `grep -rn
  PATTERN dir/`.
