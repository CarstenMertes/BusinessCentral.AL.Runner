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
