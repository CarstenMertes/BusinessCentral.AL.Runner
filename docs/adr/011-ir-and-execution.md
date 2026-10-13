# ADR-011: Typed IR and execution

- Status: Accepted (project owner, 2026-10-13)
- Date: 2026-10-13

## Context
The handoff pipeline is semantic model → typed IR → bytecode → link → VM,
and allows "tree-walk or simple register bytecode". The spike needs tests
passing early so the pass-rate curve means something; ADR-009 lets the
internal execution strategy be whatever is simplest and fast enough.

## Decision
1. **Typed IR** (`al-ir`): per routine, a tree of statements and expressions
   in which every name is already resolved (local slot, global slot, field
   id, procedure id, builtin overload id from ADR-008) and every node carries
   its `Ty`. Implicit conversions are explicit `Convert` nodes. The IR is
   plain data, `serde`-serialisable, with no syntax nodes inside.
2. **Lowering** (`al-sema` → `al-ir`) refuses anything it cannot express:
   an unimplemented builtin or construct is a compile diagnostic (hard rule 4),
   and the test codeunit that needs it is reported "not run" with that
   diagnostic as the reason.
3. **Link** turns object references into indices into the `Engine`'s
   object tables and builds the event-subscriber registry (ADR-001).
4. **Execution starts as a tree-walking interpreter over the IR** in `al-vm`.
   Bytecode is deferred until the performance budgets (median test ≤ 1 ms,
   full corpus ≤ 5 s) show it is needed; the IR is the stable seam either way.
5. **Control flow is a value, not a panic** (ADR-002): every evaluation
   returns `Result<Value, Signal>` where `Signal` is `Error(ErrorState)`,
   `Exit(Option<Value>)` or `Break`. `asserterror`, `Codeunit.Run` with a
   return value, and TryFunctions are the places that catch `Error`.
6. **Test running follows BC's test runner**, as observed through the
   corpus: one fresh `Session` per test codeunit, the per-test transaction
   and isolation behaviour, `OnRun` and handler functions. Where the corpus
   `codeunit/` and `error-handling/` tests pin this behaviour, they are the
   spec; where they do not, a probe decides.

## Consequences
- The first passing corpus tests need: lowering for the subset they use, the
  interpreter, a few builtins (Assert's dependencies), and the test runner.
  Everything else stays "not run" with a precise reason, which is the
  per-builtin backlog.
- Swapping in a bytecode backend later does not touch sema or the builtins.

## Conformance
`al-ir` round-trips through serde; the interpreter has unit tests per IR
node; the corpus pass rate is the integration test.
