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
