# Architecture

How the runtime is built today. The binding decisions are the ADRs in
`docs/adr/`; this page is the map.

## Pipeline

```
AL source
  → lexer → preprocessor (#if, token level)              al-syntax
  → lossless CST (rowan green trees, error tolerant)     al-syntax
  → workspace: apps, objects, namespaces, extensions     al-sema   (parallel per file)
  → member tables, type check, name/overload resolution  al-sema
  → lowering to a fully resolved typed IR                al-sema → al-ir
  → tree-walking interpreter over the IR                 al-vm
  → in-memory store (persistent maps)                    al-data
```

Anything the pipeline cannot handle is refused. Lowering records a reason per
routine, and the runner reports a test as **not run** if anything it can reach
statically is refused. What can only be discovered while running raises
`Signal::Unsupported`, which is also reported as not run. Nothing unsupported
runs with a guessed behaviour.

There is no bytecode stage. ADR-011 keeps the tree-walker until a budget
needs more.

## Crates

| Crate | Responsibility |
|---|---|
| `al-syntax` | Lexer, `#if` preprocessor, error-tolerant lossless parser, typed AST views, fast green-tree builder |
| `al-sema` | Workspace of apps and objects with namespace resolution; members (fields, keys, CalcFormula, TableRelation, page controls); type checker; lowering to IR |
| `al-builtins` | Builtin catalog, generated from the corpus surface file plus evidence-backed overrides, keyed by overload id (`Record.Insert#2`); the value model; natives for text, format/evaluate, filters, JSON, XML, streams, collections, refs |
| `al-ir` | Typed IR: program, objects, routines, places, expressions, table and page schemas |
| `al-vm` | Engine (immutable, `Arc`) and Session (all mutable state). Records, triggers, events and subscribers, FlowFields, AutoIncrement, RecordRef/FieldRef, relations and rename propagation, UI dialogs and handlers, the headless page client |
| `al-data` | The `Store` trait (ADR-003), `MemStore` on `im` persistent maps, and its contract suite |
| `al-host` | Capability traits for the host (placeholder; ADR-006) |
| `al-test-cli` | Corpus discovery, oracle reading, the BC test-runner semantics, reports (text, Markdown, JSON, JUnit), the ratchet |

## Runtime model

- **Engine and Session (ADR-001, 002).** The compiled program is immutable and
  shared. A Session holds the store, temporary stores, single-instance
  codeunits, event bindings, the UI handler scope and the page clients. It is
  synchronous and uses `Rc`/`RefCell` inside.
- **Values (ADR-010).** One `Value` enum. Value types copy. List, Dictionary,
  Json, Xml and stream types share. A Record is a handle onto a field buffer,
  filters, key and position; `:=` copies fields.
- **Store (ADR-003, 004).** AL-shaped operations with typed outcomes.
  Snapshots are root pointers, so commit, rollback and per-test reset are O(1).
- **Test runner semantics** (pinned by the corpus):
  - One session and one fresh store per test codeunit; install codeunits run first.
  - A passing test commits at the test boundary.
  - `asserterror` rolls back to the last commit.
  - `[HandlerFunctions]` are bound per test method; "Unhandled UI" and unexecuted handlers fail the test.
- **Pages (ADR-012).** Pages lower to a control tree, and their triggers are
  ordinary routines. A headless client in `al-vm/src/pages.rs` plays the BC
  test client: open/close, navigation, SetValue and validation, actions, and
  modal and page handlers.
- **XmlPorts.** An xmlport lowers to an object whose first globals are its
  element variables (text and table elements, in schema order), two hidden
  stream slots, then its declared variables; the schema is `Object::xmlport`.
  A variable is a codeunit-like instance. `Import` and `Export` run in
  `al-vm/src/xmlports.rs` over the document tree of `al-builtins/src/xmlport.rs`.
- **Reports.** A report lowers to an object whose first globals are its data item
  records (source order), then a hidden request-page flag, then its declared
  variables; the dataset is `Object::report`. A variable is a codeunit-like
  instance and its first use runs `OnInitReport`. `Run`, `RunModal`, `Execute`
  and `SetTableView` run in `al-vm/src/reports.rs`: processing only, with no
  request page, layout or output (those are refused).
- **Culture.** Fixed en-US, which matches the oracle.

## Source positions

Lowering records where statements and routines sit in the source. Coverage, call
stacks, value capture, loop tracking and a debugger all start from this table.

**The data** (`al-ir`):

- `Program::positions` is a table of `SrcPos { file, line, col, end_line, object, routine }`.
  A position is named by its index, a `u32`. Lines and columns count from 1; the
  column is a byte column. Lines are the author's lines: the compiler's added page
  expression routines do not move them.
- `Program::sources[pos.file]` gives `{ app, path }`. The path is relative to the app.
  `pos.object` and `pos.routine` index `Program::objects` and `Object::routines`, so a
  position names the routine or trigger it is in without anything else. The app is
  `Object::app`.
- `Routine::pos` is the position of the `procedure` or `trigger` keyword (after
  attributes and modifiers). It is `NO_POS` for a routine the compiler made up.
- `Stmt::Pos(id)` is a marker statement. It sits directly before every statement it
  describes, in the same list, nested statements included. Blocks (`begin ... end`) have
  none, because lowering flattens them. A statement that fails to lower leaves no marker.
- Call expressions have no position of their own. A call is found through the statement
  that contains it, so a call on a later line of a multi-line statement reports the
  statement's first line. Giving `Expr::Call` and `Expr::Builtin` an optional position
  later is additive; nothing needs it yet.

**From an executing statement to its position:**

1. `Session::call_stack` (`al-vm/src/lib.rs`) holds one `CallFrame { routine, pos }` per
   routine on the call stack, parallel to `Session::frames` (which holds the object).
2. Running `Stmt::Pos(id)` sets `pos` of the top frame. After a call returns, the caller's
   frame still holds its own statement. `while` and `repeat` set it back to the loop's own
   position after each pass, so the loop condition is attributed to the loop line.
3. `program.pos(frame.pos)` gives the `SrcPos`; `program.sources[pos.file]` gives the file.
   `program.pos(routine.pos)` gives the routine header.
4. The code that wants to see every statement as it starts (coverage, value capture,
   a debugger) hooks the `S::Pos` arm of `Session::exec`. The hit is "position `id` ran".
   Nothing else has to change.

**Where the positions go wrong, so nobody trusts them too far:** the table has no entry
for page expression routines (`pos` is `NO_POS`, the call stack marks such a frame
unreadable), and the table is only built when lowering is asked for positions
(`lower` does; `lower_with_positions(.., false)` does not, which keeps unit tests of
lowered bodies free of markers).

**Error call stacks** (`al-vm/src/callstack.rs`). An error does not carry its stack from
the point where it is raised. Each routine it leaves adds one line (innermost first),
and the routine that catches it (`asserterror`, a TryFunction call, a guarded
`Codeunit.Run`) adds the last. The line format, and which parts the corpus pins, are in
that file's header and in `docs/probes-needed.md`.

## Budgets (tests, run in CI in release)

| Budget | Limit | Today |
|---|---|---|
| Cold compile of the corpus (`al-test-cli/tests/perf_budget.rs`) | ≤ 1 s | ~0.5 s |
| Serial parse throughput (`al-syntax/tests/perf_parse_throughput.rs`) | ≥ 50 MB/s | ~65–75 MB/s |

`cargo run --release -p al-test-cli --example phases` prints timings per
phase. `--example compile_dump` dumps every compile output so a performance
change can be shown to change nothing.

## Dependencies outside the workspace

Today only the corpus apps are compiled. Objects from Microsoft apps (System
Application, Base Application, platform system tables) are refused as
"outside the workspace".

ADR-013 (Proposed) is the plan for them: compile Microsoft's own AL source,
fetched from the BC artifact feed into a local cache, and implement natively
only the platform objects that have no AL body.
