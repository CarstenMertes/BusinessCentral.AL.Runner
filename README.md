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

## Why it is built on the language, not on BC's internals

AL Runner v3 reproduces what AL code can observe: values, errors, data, and the order of
triggers and events. That surface is the public contract of the language. Microsoft cannot
change it without a breaking change for every partner, so it changes slowly and visibly,
behind versions and obsolete markers.

AL Runner v2 ran Microsoft's compiled code on top of BC's internals: the C# runtime, its
metadata and session classes, and patches into them. Microsoft can change those in any
hotfix or release without notice, so a v2 fix can stop working with the next update, and
v2 needed one engine per BC minor version. v3 is one binary for all supported versions.

Where the observable behavior does differ between BC versions (an error text, a system
table column), the corpus measures it on every major and pins it per version, so a change
in a new BC release shows up as a failing corpus case, not as a mystery. The one part that
still loads Microsoft's internals is the optional .NET helper for `DotNet` code, and it is
kept small on purpose.

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

## Try the alpha

`alrunner` is in alpha (`3.0.0-alpha.1`). It does not match AL Runner v2 yet.
Many tests are refused, and results may change between builds. Use it to try
it out and to report what is wrong. Do not gate a pipeline on it.

**Which build?**

- **edge**: the newest build of the `release` branch. One pre-release named `edge`,
  replaced on each build. Pick this to test the latest fixes.
- **alpha**: a numbered checkpoint (`3.0.0-alpha.N`). Pick this for a stable
  point you can name in a bug report.

**Install** (Linux x64, Windows x64, macOS arm64). The scripts check the SHA-256 of
the download. Replace `latest-alpha` with `edge` or a tag such as `v3.0.0-alpha.1`.

```
curl -fsSL https://github.com/StefanMaron/al-runtime-rs/releases/download/edge/install.sh | sh -s -- latest-alpha   # Linux, macOS
```

On Windows, download `install.ps1` from the `edge` release and run
`./install.ps1 -Version latest-alpha` in PowerShell. (GitHub's "latest" link
skips pre-releases, so the `edge` release is the stable place for the scripts.)
You can also download an archive from the releases page, check it against
`SHA256SUMS`, and unpack it. Each archive holds the binary, `LICENSE` and a short
`README.txt`. `gh attestation verify FILE --repo StefanMaron/al-runtime-rs`
checks the build provenance.

A numbered alpha is a draft until the owner publishes it.

**The binaries are not signed.** Your system may warn you.

- macOS: Gatekeeper blocks the file. Run `xattr -d com.apple.quarantine ./alrunner`
  (the install script does this for you).
- Windows: SmartScreen shows "Windows protected your PC". Click "More info", then
  "Run anyway". Or run `Unblock-File .\alrunner.exe` in PowerShell.

**Run**

```
alrunner --version
alrunner test ./MyApp ./MyApp.Test
```

Pass both the app and its test app. Then your app is compiled fresh on each run,
and Microsoft's apps stay in the cache. The first run that needs Microsoft's apps
downloads them from Microsoft's artifact feed and builds the cache. Later runs
load it.

- **Test data** (the CRONUS demo company) is on by default when a run uses
  Microsoft's apps. The first run downloads about 520 MB. `--no-test-data`
  turns it off.
- **"Not run"** means `alrunner` refused the test and said why, for example a
  feature it does not support yet. It never guesses. The test did not pass and
  did not fail. `--verbose` lists the reason for each test.
- **Exit codes**: 0 all selected tests passed, 1 a test failed, 2 bad invocation
  or the run could not start, 6 the selection matched no test, 7 some tests were
  not run (`--allow-not-run` makes that 0).

**Known limits**

- Tests that use the `DotNet` type need the .NET 8 SDK. The binary carries the
  helper sources and builds the helper into the cache on first use. Without the
  SDK these tests are not run, with a reason.
- Many features are missing. See `docs/status.md` and `docs/al-runner-v2-parity.md`.
- No watch mode, no server mode, no coverage yet.
- Only three targets are built: Linux x64, Windows x64 and macOS arm64.

**Report problems** as a GitHub issue, using the "Alpha feedback" template.
A wrong result is the most useful report: a test that passes or fails here and
behaves differently on real Business Central.

## What AL Runner v3 does not do

Some AL features are refused on purpose. A test that reaches one is reported as
"not run", with the reason. It is never guessed.

| Area | What v3 does | What to do instead |
|---|---|---|
| Email, outbound HTTP, web service endpoints | Not run. No network, no server | Put the call behind an AL interface and fake it in the test |
| Cloud storage, Key Vault, external data connections | Not run | Same |
| File upload and download dialogs | Not run (needs a browser) | Test with streams |
| Report rendering to PDF, Word or Excel | Not run. The XML dataset works | Assert on the dataset |
| Background jobs, `StartSession` | Tasks are created and never run. A second session is not started | Call the code the task would run |
| Licensing, permission enforcement | Everything is granted. Denial paths cannot be tested | Use a real service tier |
| Apps that ship without AL source | Not run. The message names the app | Provide an `.app` with source |
| Server mode, watch mode, debugger, coverage | Deferred | Not available yet |

The full list, with reasons and open questions, is
[ADR-027](docs/adr/027-scope.md) (Proposed).

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

- `docs/releasing.md`: how an edge build, a numbered alpha and a hotfix are released.
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

## License

al-runtime-rs is licensed under the Business Source License 1.1 (`LICENSE`). You may use it for free,
including commercial work: developing, testing and building Business Central extensions for yourself or
for your customers, on your own machines and in build pipelines. You may not sell it, or offer it or a
product or service built substantially on it to others for a fee. Each version becomes available under
the Apache License 2.0 four years after it is published. Versions published before this change remain
under the MIT License. For other licensing arrangements, contact the author.
