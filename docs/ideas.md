# Ideas: developer features an own runtime makes possible

Written 2026-10-15 by the orchestrator, without the owner's own list (he asked for an
unbiased view). Nothing here is decided or planned. Each idea says what it does, why the
stock BC toolchain cannot do it, and a rough size (S: days, M: one to two weeks, L: more).

What makes these possible: v3 compiles and runs AL itself, in one process, deterministically,
in milliseconds, with the whole program, every statement and every database write in view.

## Understanding a failure

1. **Failure context, not just a message.** On a failed test, print the local variables of
   every frame at the error, the last 20 statements executed, and the record buffers involved
   (field values, filters, key). BC gives a message and a call stack. Size: M.
2. **Database diff per test.** After each test (or at the failure), list the rows the test
   inserted, modified and deleted, with before and after values, per table. "What did my
   posting routine actually write?" answered without a debugger. Size: M.
3. **Event trace.** For one test or one statement: every event raised, every subscriber that
   ran, in order, with its app and how long it took. Answers "who changed my field?" in large
   multi-app projects. Size: S to M.
4. **Replay a failing test.** Runs are deterministic, so a failing test can be rerun to any
   statement and stepped backwards (record once, then move through the trace). A
   time-travel debugger, which BC's debugger cannot offer. Size: L.

## Test quality

5. **Mutation testing.** Change the app under test in small ways (flip a comparison, drop a
   Modify, swap a field) and rerun its tests; a mutation no test notices is an untested
   behavior. Only practical when a full test run takes seconds. Size: M.
6. **Order and isolation checks.** Run test codeunits in random order and in each isolation
   mode, and report tests that pass only after another test ran (hidden shared state), or
   that leave data behind. Size: S.
7. **Exact coverage per test.** Line and branch coverage of the app, per test, from the
   interpreter, with no instrumentation cost. Plus "code no test reaches" for the app. Size: S
   (the runtime already counts routines).
8. **Controlled world.** Run as if today is a given date, with a fixed random seed, as a given
   user, in a given company or work date, from the command line, without changing test code.
   Month-end and year-end bugs become testable. Size: S.

## Performance and correctness warnings BC only shows in production

9. **Database cost profile.** Per test and per procedure: how many reads, writes, CalcFields
   and FlowField computations, and where. Flags N+1 patterns (a CalcFields or Get inside a
   loop over the same set) with the line. Size: M.
10. **Partial-record warnings.** Report every read of a field outside `SetLoadFields`, which
    on real BC is an extra SQL round trip (a "JIT load"), with the line that caused it. Only a
    runtime that tracks loaded fields per buffer can see this. Size: S.
11. **Transaction and lock warnings.** Report a `Commit` inside a routine called from posting,
    a write without `LockTable` in code that reads then writes the same row, and long write
    transactions. Size: M.
12. **Permission check mode.** Run the tests with only the app's permission sets (and the
    user's) and report each table access the sets do not grant. Missing permissions are a
    common AppSource failure found late. Size: M.

## Versions and upgrades

13. **Cross-version run.** One command runs the tests against BC 27, 28 and 29 and lists the
    tests whose outcome differs. Know what breaks before upgrading. Size: S (the runtime
    already compiles each major).
14. **Upgrade code testing.** Restore the demo data of an older version, install the previous
    app version, then run the new version's upgrade codeunits and the tests. Size: L.
15. **Obsolete API report.** List every use of objects and members marked obsolete (pending or
    removed) in the next BC version, from the real dependency sources. Size: S.

## Speed of the inner loop

16. **Instant evaluation.** `alrunner eval` runs an AL snippet or one procedure against the
    compiled app and the demo data in well under a second, like a REPL. Size: M.
17. **Snapshot testing.** Compare a report dataset, a JSON or XML output, or a table's content
    with an approved file, and show the diff on change. Size: S.
18. **Fast setup reuse.** Snapshot the database after a shared setup and fork it per test,
    so expensive setup (posting master data) runs once. Size: M.

## My pick

If I could choose three: database diff per test (2), database cost profile with N+1 and
JIT-load warnings (9, 10), and the controlled world (8). They need little new machinery, every
AL developer hits these questions weekly, and none of them is possible with the stock tools.
Mutation testing (5) is the strongest "only we can do this" feature once the runtime is broad
enough that most app tests run.

## Owner feedback (2026-10-15)

Wanted, high value:
- **Database diff per test** (2) and **database cost profile** (9, with 10): very good ideas.
- **Seed** (part of 8): v2 prints the random seed of every run, so a failure seen once in a
  pipeline can be reproduced locally with `--seed`. v3 needs the same (ADR-016 gate P also lists
  `--seed`). The rest of "controlled world" (date, user) is optional.
- **Exact coverage per test** (7): important, mainly to rerun only the tests that exercise what
  changed since the last run ("rerun relevant tests", which v2 has as a performance mode).
- **Upgrade code testing** (14): a good one.

New ideas from the owner:
- **Value tracing.** Point at a field (or variable) and get, out of the box, the trace of where
  its value came from and where it went through the code, without writing a test for it.
  The interpreter sees every assignment, so a data-flow trace per run is possible.
- **Interactive debugging with changing variables in memory**, which other languages have and
  AL does not. The interpreter owns all state, so a debugger can stop, inspect and set values.
- **Country matrix.** More than versions, partners must run their tests across many
  localizations (around 5 versions times 50 countries). The runtime could run one compiled test
  app against each country's Base Application and demo data, and report the outcome matrix
  (only the cells that differ).
- **Tests independent of CRONUS data.** Record which rows a test actually read from the demo
  data (the "givens" it needs) and generate AL setup code that creates exactly those rows, so the
  test runs without a CRONUS database and stops depending on its structure. The runtime already
  loads rows on first touch, so it knows each test's data footprint.

Lower priority or unclear:
- Random order and isolation checks (6): backlog, not urgent.
- Cross-version runs (13): the country matrix matters more.
- REPL (16), snapshot tests (17), setup reuse (18): explained to the owner, not yet rated.

## Live testing as you type (owner discussion 2026-10-15)

- **What:** while a developer types in VS Code (ALchemist or any editor), the affected tests run
  after each short pause (about 300 ms) and the results appear inline: pass or fail per test, the
  failing assert, and with value capture the values per line.
- **How:** `alrunner serve` keeps the compiled Microsoft and dependency apps in memory; the editor
  sends the unsaved file; v3 recompiles only what changed and runs only the tests whose coverage
  reaches the change.
- **Speed today:** one filtered test 0.1 to 0.2 s total, the Pageworks suite about 1 s warm, NP
  Retail (2,300 files) compiles in 2.3 s. Small and mid-size apps are live already; large apps need
  the incremental compile (ADR-029: a change in the code of codeunits and tables compiles only the
  changed objects; measured: Tests-ERM request 5.5 s to 2.2 s, corpus app 1.3 s to 0.4 s).
- **Needs, in order:** serve with warm state (tool-integration lane), per-test coverage (idea 7),
  incremental compile of the workspace (ADR-029, built for codeunits and tables), value capture (ALchemist).
- **Agents:** the same warm loop serves AI agents: edit a file, get the affected test results in a
  fraction of a second instead of a cold run.

## Vision: faster than BC at raw computation (owner, 2026-10-16)

Not a goal for now, a direction for later. BC's runtime grew over 30 years (AL to C# to .NET IL to JIT, with
SQL behind every record operation). v3 has no database in the loop, so every test, Base Application's included,
is bound by computation alone: every gain in the interpreter and the record store reaches every test directly.
The owner's vision is that v3 overtakes BC not only end to end (already true) but also on pure computation.

Where v3 stands (2026-10-16, Pageworks): codeunits that ran fully take 13.7 s against BC's 13.3 s; pure compute
is still 2 to 3 times slower than BC's JIT-compiled code (QR encoder 2.2x, font embedding 3.2x).

The path, in order, each step checked by the dual run (ADR-034):
1. Typed instructions: AL is statically typed, so the bytecode compiler can emit "add two Integers" instead of a
   generic add that checks types at run time.
2. Inline caches: an instruction remembers its resolved call target, field position or builtin, so a repeated
   lookup costs nothing.
3. Superinstructions: frequent instruction pairs (compare and jump, load field and compare) become one.
4. Ahead-of-time native code with Cranelift (a Rust code generator, about 10x faster to compile than LLVM):
   machine code built once per compile-cache layer (Microsoft's apps once per BC build, like the cache today),
   stored per platform; the workspace stays on bytecode or gets a quick Cranelift compile. No JIT warm-up at test
   time. Needs a new ADR (ADR-034 says "no JIT"); costs: a second back end to keep equal (the dual run is the
   check), per-platform cache files, and debugger/capture/coverage hooks in native code.
5. The record store as computation: with no SQL, key comparison, row copies and filter matching are the store's
   cost; ADR-035 steps 2 and 3 (shared rows, encoded keys), deferred today, come back when they are the top item.

### Native code with Cranelift: how it would fit (owner, 2026-10-16)

- Owner's model: compile to machine code wherever possible, on the user's machine, and cache it. Microsoft's apps
  and other dependencies once per build (ahead of time, stored with the compile cache layer); the workspace
  apps too if the compile cost allows.
- Rough estimates, to be measured by a spike before any ADR: Cranelift compiles a medium function in roughly
  50-200 microseconds. A workspace app with 2,000 routines: about 0.1-0.4 s on one core, less in parallel, so
  compiling the user's own apps on every change is affordable, and the incremental compile (ADR-029/033)
  limits it to the changed objects. Base Application (on the order of 100,000+ routines): roughly 10-30 s on one
  core, a few seconds on 8 cores, once per BC build, cached per platform.
- Tiering keeps cost where it pays: run bytecode first and compile a routine to native code only after it ran
  often (true JIT for the hot part), or compile ahead of time only the routines a test run actually reached
  (most of Base Application never runs in a given suite).
- The spike to answer it: compile the corpus and Pageworks' hot codeunits (QR, fonts) with Cranelift; measure
  compile time per routine, cache size, and run time against bytecode and BC.

## Build pin for the compile cache (owner, 2026-10-16)

Microsoft publishes several hotfix builds a day, and each new build means a new compile-cache layer for its
apps. Today: `--bc-version <four numbers>` pins an exact build for one run, and with a major or major.minor a
build already in the local artifact cache wins over a newer published one. Missing:
- A settings file (in the project, e.g. next to app.json, and per user) that pins the build per BC major, so
  every run and every developer of a project reuses one cache.
- A policy instead of a fixed build: "newest at most once a day / once a week", or "a named channel", so the
  cache moves on a schedule the developer or the pipeline chooses.
- An explicit refresh command, and a line in each run saying which build was used and why.
- Pipelines: with the pin in the repository, a CI cache keyed on the pinned build stays valid until the pin
  moves (for example weekly), instead of a rebuild per hotfix.

## Parallel tests inside a codeunit under TestIsolation = Function (owner discussion, 2026-10-16)

Today the unit of parallelism is the test codeunit: each runs on its own session and thread, so many small test
codeunits use a many-core machine well, and the slowest codeunit sets the minimum wall time. Under
TestIsolation = Function the database is rolled back after each test, but the tests of a codeunit still run in
order on one thread. They cannot simply run in parallel: BC keeps the codeunit's global variables alive across
its tests (Microsoft's `Initialize`/`IsInitialized` pattern relies on it), so fresh copies per test would be
observable. Idea: run the tests of a Function-isolated codeunit in parallel, each on its own copy of the start
state, when a static check shows no global written by one test is read by another (or the author declares it);
otherwise keep the order. Advice for test design meanwhile: many small test codeunits, and expensive tests
(heavy setup, pure computation) in codeunits of their own.

## Differential exploration against real BC, after the known gaps are closed (owner, 2026-10-16)

The corpus is filled from three known sources: how the language is documented and works, Microsoft's test
suites, and partner test suites. Once those are exhausted (every coverage map and refusal list closed), what is
left is behavior no known test exercises: unusual combinations, edge values, orders of events across
extensions, interactions of features. Idea for that phase: agents generate such combinations on purpose
("try to break it"), run each one on a real BC container and on v3, and compare. Every difference is a new
corpus case, found even though no existing test ever exercises it. The corpus writer's tooling (local
container, compile legs, dupfinder) and the oracle comparison already exist; what is new is the generator:
grammar- and catalog-driven programs over the pinned building blocks (types, triggers, events, record
operations), kept small so a difference points at one cause, and deduplicated before they become cases.

## Microsoft's test suite as one command for any developer (owner, 2026-10-17)

The owner had this idea for the v2 runner. Nothing here is decided or planned.

1. **The recipe ships with the runner.** The runner knows how Microsoft runs its own test
   apps: which test apps, in which company, with which test type and bucket, and which tests
   Microsoft disables (BCApps `build/projects/Test Apps <country>` settings and
   `src/DisabledTests`). How much of that is embedded and how much is read from BCApps or the
   artifacts is open. One command, for example `alrunner test --microsoft-suite ./MyApp`,
   then installs the developer's app next to Microsoft's apps and runs the whole Microsoft
   suite against it. Any developer sees which Microsoft tests their customization breaks,
   without a container or a pipeline. The same run is how we report parity with Microsoft.
2. **Only the tests that cover what I changed.** The runner records, per Microsoft test, the
   routines and event publishers it reaches (it already has static reach and can record
   dynamic reach). A customization that subscribes to one event in the sales posting routine
   then runs only the Microsoft tests whose path raises that publisher, and the same for a
   table or page extension (tests that touch the extended object) or a replaced procedure.
   With a full run in a few minutes this matters less, but it makes the loop seconds long
   and tells the developer which tests are relevant at all. The coverage map per test could
   be built once per BC version and shipped with the compile cache.
