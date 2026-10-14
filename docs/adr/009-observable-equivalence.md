# ADR-009: Observable equivalence, freedom everywhere else

- Status: Accepted (decided by the project owner, 2026-10-13)
- Date: 2026-10-13

## Context
The runtime is measured against real BC. That could be read as "reproduce
BC's implementation", which would import its accidental complexity and its
limits. The project owner's direction: as long as the observable outcome is
the same, the runtime may improve on the original AL language and BC stack.

## Decision
**The contract is observable behaviour, not implementation.**

*Observable* means anything AL code or a test can see:
- values returned and assigned, including formatting, rounding and conversion results;
- whether an error occurs, its text, code and the point where control resumes;
- database state as AL reads it (rows, keys, ordering, filters, FlowField
  results, AutoIncrement counters), including after commit and rollback;
- which triggers and event subscribers run, in what order, with what arguments;
- what the host sees (messages, confirms, pages opened, HTTP requests sent);
- whether a program compiles at all.

These must match real BC, settled by the oracle (ADR-007) and by probes
(hard rule 3).

**Everything else is ours to improve**, for example:
- internal architecture, data structures and algorithms (persistent maps,
  O(1) snapshots, a bytecode VM instead of generated C#);
- performance, startup time, memory use, parallel test execution;
- determinism and testability (injected clock, identity, randomness);
- diagnostics: clearer messages, extra warnings, better source locations;
- tooling: tracing, debugging, coverage, explain-why output.

## Limits
- An improvement must not change the observable outcome of any program BC
  accepts. In particular: no rejecting code BC compiles (a warning is fine,
  an error is not), no "fixing" BC semantics that AL code can observe, even
  where BC's behaviour looks like a bug. If BC behaviour seems wrong, record
  it as a probe test in the corpus; it stays the spec until BC changes.
- Hard rule 4 still applies: a builtin we have not implemented is a compile
  error. That is a temporary gap, tracked in the pass rate, not an improvement.
- Where an improvement is a visible design choice (for example a stricter
  optional mode), it is off by default and recorded in an ADR.

## Consequences
Reviews ask two questions of every change: does any observable outcome
differ from BC (then it is a bug, whatever its other merits), and if not,
is it simpler or better than the obvious copy of BC (then prefer it).

## Decided without owner review (2026-10-13/14)

Record fixes after the corpus bump (branch `fix-records`). Each item is also a row in `docs/probes-needed.md`.

- **Deferred duplicate Insert.** BC defers the failure of a duplicate-key Insert on a table without a Blob field. The probe pins that for persistent tables with an unused result. Decided: only AL-called Inserts on persistent tables without Blob and AutoIncrement fields defer. The page client, the XmlPort import, temporary records and AutoIncrement tables raise at once, because their own corpus tests pin that. Options: defer everywhere (breaks those tests), or never defer (fails the probe).
- **Refused tests roll back.** A test that stops with `Unsupported` leaves its writes in the store. Decided: the runner rolls them back, as it does for a failed test. Option: leave them (a later test then sees rows BC's run of the earlier test would also have left, but only partly).
- **Emit version.** `major*1000+25` for BC majors other than 27. Option: refuse for other majors.
- **xRec of table extension triggers** is the same handle as the events' and the table trigger's (equal to Rec for a Modify from code). No test pins the extension triggers.
- **Baseline removal.** `60013 CalcFields_TableExtensionFlowFieldOnStockkeepingUnit_WithNoMatchingRows_IsZero` passed because the sibling test before it was refused, so no row existed to delete. Once that sibling passes, the test deletes rows and reaches Base App subscribers that need `TaskScheduler.CanCreateTask`, which is not implemented or pinned. The case is removed from the three baselines as a pass by accident (hard rule 2). It returns when the builtin is pinned.
