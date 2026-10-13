# al-runtime-rs

A Rust-native runtime for AL, the Business Central language. It compiles AL
source with its own front end and executes it in its own VM against an
in-memory database. No BC service tier, no .NET and no Microsoft DLLs are
involved.

It is a ground-up rewrite of the ideas behind
[BusinessCentral.AL.Runner](https://github.com/StefanMaron/BusinessCentral.AL.Runner)
(the C# runner, which executes Microsoft's compiled DLLs). It started as a
feasibility spike and passed it (`docs/history/`).

## What "correct" means here

Correctness is defined by real Business Central, not by documentation. The
[AL language test corpus](https://github.com/StefanMaron/BusinessCentral.AL.Language.Tests)
runs on BC Cloud in its own CI. Its JUnit result for BC 27.5 is the oracle in
`oracle/`. A test counts as passing only when it passes here **and** passed on
real BC. Observable behaviour must match BC; internals, speed and diagnostics
are free to be better (ADR-009).

## Status

Against BC 27.5, corpus commit `cf6e546`:

| | |
|---|---|
| Passing | **2412 / 4836 (49.9%)** |
| Failing (wrong answers) | **0** |
| Not run (explicitly refused: feature not implemented yet) | 2424 |
| Cold compile of the corpus (1769 objects) | ~0.5 s (budget 1 s) |
| Full corpus run, compile + 4836 tests | ~1 s |

Anything the runtime cannot do yet is refused with a reason and reported as
"not run". It never guesses. Per-area numbers, what is implemented and what's
next: `docs/status.md`.

## Quick start

Clone with `--recurse-submodules`; the corpus is a pinned submodule.

```
cargo build --release
./target/release/al-test-cli --root .                  # pass rate vs BC 27.5, ratchet check
./target/release/al-test-cli --root . --junit out.xml  # per-test outcomes with refusal reasons
./check.sh                                             # fmt, clippy, all tests, corpus ratchet
```

`al-test-cli` also takes `--markdown FILE`, `--json FILE`, `--oracle`,
`--baseline` and `--update-baseline`.

The corpus depends on Microsoft's apps. They are read from the project's
`.alpackages` folder first (the corpus ships System Application, Base
Application and Business Foundation there, with source) and compiled by this
runtime's own front end (ADR-013). Only what the tests reach is compiled. Nothing
of Microsoft's is committed to this repository. `--deps DIR` adds another folder
of `.app` packages, searched after `.alpackages`. `--no-deps` runs the corpus on
its own.

## Documentation

- `docs/architecture.md`: pipeline, crates, runtime model, budgets.
- `docs/status.md`: current coverage, what is refused, roadmap, known issues.
- `docs/adr/`: architecture decisions. Seams change only through an ADR.
- `docs/probes-needed.md`: behaviour taken from documentation only, waiting for a probe on real BC.
- `docs/corpus-inventory.md`: what the corpus contains.
- `CLAUDE.md`: the rules every contributor and agent follows.
- `oracle/`: the real-BC results the pass rate is measured against.
- `metrics/<major>/passing.txt`: the ratchet per BC major, i.e. cases that must keep passing.
- `docs/history/`: the original spike brief and the end-of-spike report.
