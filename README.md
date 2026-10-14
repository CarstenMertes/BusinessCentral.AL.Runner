# al-runtime-rs

A Rust-native runtime for AL, the Business Central language. It parses and
checks AL source with its own front end and runs it in its own VM against an
in-memory database. It needs no BC service tier, no Microsoft DLLs and no .NET.
AL code that uses the `DotNet` type calls an optional .NET helper process
(see "DotNet" below).

It is a Rust rewrite of the ideas behind
[BusinessCentral.AL.Runner](https://github.com/StefanMaron/BusinessCentral.AL.Runner).
That C# runner executes Microsoft's compiled code; this runtime compiles and
runs AL source itself. The original spike brief and report are in `docs/history/`.

## What "correct" means here

Correctness is defined by real Business Central, not by documentation. The
[AL language test corpus](https://github.com/StefanMaron/BusinessCentral.AL.Language.Tests)
runs its CI against real BC in containers, once per BC major (27, 28 and 29).
The JUnit result of each run is the oracle for that major (`oracle/`). A test
counts as passing only when it passes here **and** passed on real BC.
Observable behavior must match BC. Internals, speed and diagnostics are free to
be better (ADR-009).

## Status

Corpus commit `6219bf4`. Each major has its own oracle and its own baseline of
cases that must keep passing (`metrics/<major>/passing.txt`).

| BC version | Passing | Failing | Not run |
|---|---|---|---|
| 27.5 | 4913 / 5147 (95.5%) | 0 | 234 |
| 28.5 | 4913 / 5147 (95.5%) | 0 | 234 |
| 29.0 | 4913 / 5147 (95.5%) | 0 | 234 |

Failing means the runtime gave a different answer than real BC. That count is 0.
"Not run" means the runtime refused the test, with a stated reason, because a
feature is missing. It never guesses. Per-area numbers and the roadmap are in
`docs/status.md`. The corpus also has a smaller OnPrem test app (42 of 64
passing on each version) and an isolation probe, reported separately.

On BC 28.5 and 29.0 the corpus compiles Microsoft's apps from the corpus's
27.5 `.alpackages`. Only the test libraries come from the artifact of the
matching version.

Time for the corpus on BC 29.0 on a shared 12-thread machine under load
(`al-test-cli --timings`, total of load, compile, install baseline and run):

- With Microsoft's apps: 2.9 to 6.2 s in six runs. `al-test-cli` compiles Base
  Application, System Application and the others from source on every run, only
  what the tests reach. `alrunner test` keeps their complete compile in a compile
  cache (ADR-022, `<cache dir>/compile/`, about 13 MB per set of apps) and loads it
  on later runs; the first run builds it (about 5 s, 4 GB). `--no-compile-cache`
  turns it off, and `al-test-cli --compile-cache` turns it on for the corpus.
- With `--no-deps` (the corpus alone; 4255 of 5147 tests pass): 0.4 to 0.9 s in
  five runs.

## Quick start

Clone with `--recurse-submodules`; the corpus is a pinned submodule.

```
cargo build --release
./target/release/al-test-cli --root .                  # pass rate for the newest BC version, ratchet check
./target/release/al-test-cli --root . --all-versions   # 27.5, 28.5 and 29.0, then compare
./target/release/al-test-cli --root . --explain TEST   # why one test passed, failed or was not run
./check.sh                                             # fmt, clippy, all tests, corpus ratchet
```

## Command lines

**`al-test-cli`** runs the corpus and reports the pass rate against the oracle.
Exit code 0 is ok, 1 means a baseline case no longer passes, 2 is a setup error.
Main flags by purpose (`--help` lists all of them):

- Version: `--bc-version N`, `--all-versions`.
- Test selection: `--codeunit ID`, `--test NAME`, `--area FOLDER`, `--file SUFFIX`.
- Diagnosis: `--explain TEST`, `--top-refusals [N]`, `--baseline-diff`, `--timings`.
- Baselines and reports: `--update-baseline`, `--no-ratchet`, `--junit FILE`,
  `--json FILE`, `--markdown FILE`.

**`alrunner`** runs the tests of any app, or of one of Microsoft's
`Tests-*.Source.zip` archives. Every test codeunit runs in its own session.

```
alrunner test ./MyApp                                       # an app folder with app.json
alrunner test ./MyApp --filter Posting --verbose            # selected tests, one line per test
alrunner test Tests-ERM.Source.zip                          # Microsoft's tests: apps, System.app and CRONUS data fetched
alrunner test ./MyApp --no-test-data                        # without the CRONUS company
```

Exit codes: 0 all selected tests passed, 1 a test failed, 2 bad invocation,
6 the selection matched no test, 7 some tests were not run
(`--allow-not-run` makes that 0). `alrunner test --help` lists the rest.

## Dependencies

Microsoft's apps are compiled from their own source (ADR-013). Nothing of
Microsoft's is committed to this repository.

1. `alrunner` searches `--dep` and `--package-cache` first, then each app's
   `.alpackages` folder. The corpus ships System Application, Base Application
   and Business Foundation in its `.alpackages`, with source.
2. What is still missing comes from Microsoft's artifact feed: the apps, the
   test libraries, the platform `System.app` and the demo data. For Microsoft's
   own test archives the build is the build of the tests when it is still
   available. Downloads are kept in a local cache (`--cache-dir`).
   `--no-download` turns the feed off.
3. Only the objects the selected tests reach are compiled.

`al-test-cli` searches `.alpackages` first, then another folder of `.app`
packages given with `--deps DIR`, then the feed.

## Test data

Test data is on by default. A run that uses Microsoft's apps restores the CRONUS
demo company of the same BC build from its backup (`.bak`); a table is read the
first time a test touches it. The backup is looked for in
`~/.bcartifacts.cache/sandbox/<build>/w1`, then in the artifact cache, then it is
downloaded once (about 520 MB, progress on stderr) from the artifact feed into the
cache. If none can be had, test data is off and the run says why.
`--no-test-data` turns it off; `--test-data PATH.bak` names a backup.
Without test data, Microsoft's tests find an empty database and many fail on
missing setup data. Those failures are not trusted.

`al-bcdb` is the reader behind this: a Rust port of bcdb for BC `.bak` and
`.bacpac` files, with no SQL Server. It is also a command line
(`al-bcdb tables FILE`). See `docs/bcdb.md`.

## DotNet

The core needs no .NET. AL code that uses the `DotNet` type is served by an
optional .NET 8 helper process (ADR-014). Without it, tests that reach a
`DotNet` call are not run, with the reason stated. `--require-dotnet` turns
that into an error, for CI.

## Microsoft's test buckets

`alrunner` can run Microsoft's own Base Application test buckets. Most of these
tests are still refused, not run. On 2026-10-14, the 32 buckets together held
40300 tests: 1083 passed, 858 failed and 38359 were not run. Those runs had no
test data, so most of the failures are missing setup data. Test data fixes
part of that: on Tests-ERM it raises the passes from 241 to 487, with 5
failures. `docs/alrunner-buckets.md` has the per-bucket numbers, the recipe and
the failure analysis. Speed numbers of such a run cannot be compared with other
runners until many more of these tests run.

## Documentation

- `docs/architecture.md`: pipeline, crates, runtime model, budgets.
- `docs/codemap.md`: which module owns what, and where the entry points are.
- `docs/status.md`: current coverage, what is refused, roadmap, known issues.
- `docs/adr/`: architecture decisions. Seams change only through an ADR.
- `docs/alrunner-buckets.md`: how to run Microsoft's test buckets, and the measurements.
- `docs/bcdb.md`: the `.bak` and `.bacpac` reader.
- `docs/al-runner-v2-parity.md`: which features of BusinessCentral.AL.Runner exist here.
- `docs/owner-review.md`: decisions made without the owner, waiting for review.
- `docs/dotnet-examples.md` and `docs/dotnet-generic.md`: how Microsoft's AL uses `DotNet`, and the options.
- `docs/probes-needed.md`: behavior taken from documentation only, waiting for a probe on real BC.
- `docs/corpus-inventory.md`: what the corpus contains.
- `CLAUDE.md`: the rules every contributor and agent follows.
- `docs/agents/brief.md`: the working method for implementation agents (worktree, TDD, gates, memory caps, report).
- `docs/agents/remote-worker.md`: setup and task loop for a session on another machine that takes GitHub issues.
- `oracle/`: the real-BC results the pass rate is measured against.
- `metrics/<major>/passing.txt`: the cases that must keep passing, per BC major.
- `docs/history/`: the original spike brief and report.
