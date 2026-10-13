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

## Source positions and error call stacks (added 2026-10-13/14)

An additive change to the IR; no seam moved. Details are in `docs/architecture.md`
("Source positions").

- `Program::positions` and `Program::sources` hold source positions. `Stmt::Pos(id)`
  precedes every lowered statement. `Routine::pos` is the position of its `procedure` or
  `trigger` keyword.
- The VM tracks the routine and current position of every frame, and builds the AL call
  stack of an error as it unwinds (`al-vm/src/callstack.rs`).

### Decided without owner review (2026-10-13/14)

1. **Marker statements, not a wrapper around `Stmt`.** Options: (a) change `Stmt` into a struct
   with a position and a kind; (b) wrap every statement in `Stmt::At(id, Box<Stmt>)`;
   (c) a `Stmt::Pos(id)` marker before the statement. Chosen: (c). It adds no boxes, leaves
   every other `Stmt` match untouched (one new arm each), and keeps this change mergeable
   with the other agents' work in `lower.rs`. The cost is one extra list entry and one
   cheap VM step per statement (measured in the report that came with this change).
2. **No positions on call expressions.** Options: add a position field to `Expr::Call`,
   `Expr::InterfaceCall` and `Expr::Builtin`, or find a call through its statement. Chosen:
   the statement. A required field on those variants breaks every construction site other
   agents are adding. The field can be added later as an optional one.
3. **The call stack format and line counting follow AL Runner v2.** The corpus pins only
   the substrings listed in `docs/probes-needed.md`. v2 measured the full format on real BC
   (`line` counts from the `procedure` line; the quoted object name; the app tail).
   Options: refuse every stack read (no behaviour), or implement v2's format and record
   the assumption. Chosen: v2's format, for codeunits and tables only. Any other object
   type, field trigger or routine without source is refused through the unreadable marker.
   The probe that would settle it is listed in `docs/probes-needed.md`.
