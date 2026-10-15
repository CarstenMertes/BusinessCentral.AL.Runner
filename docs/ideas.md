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
