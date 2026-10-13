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
| `al-host` | Capability traits for the host (ADR-006): `Clock` and `Identity` with AL Runner's defaults (ADR-013); the session reads them for `Today`, `UserId`, the system field stamps and the like. The command line injects the system clock (`al-test-cli/src/host.rs`). Also `DotNetHost` / `DotNetSession` and the default `NoDotNet` (ADR-014) |
| `al-test-cli` | Corpus discovery, oracle reading, the BC test-runner semantics, reports (text, Markdown, JSON, JUnit), the ratchet; `SidecarHost` (`dotnet.rs`), the one place that starts a process (the .NET sidecar) |

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
- **DotNet (ADR-014).** A `dotnet { assembly(..) { type(..; Alias) } }` block declares
  aliases; `X: DotNet Alias` lowers to `RtType::DotNet` and member uses to `Expr::DotNet`
  (constructor, call, property get and set), whose results have no static type and are coerced
  exactly where they are stored. In `al-vm/src/dotnet/` a call goes to the registry of
  runtime-provided classes (`registry.rs`, empty until probes pin members), then through the
  policy (`policy.txt`, section 8 of the ADR), then to the host (`al-host::dotnet`), else it is
  refused. `al-test-cli/src/dotnet.rs` implements the host with a .NET 8 helper process
  (`tools/dotnet-sidecar`), one per Session, started at the first call that reaches it. A
  .NET exception is `Signal::Unsupported` with its CLR type and message until a probe pins
  BC's text. A machine without .NET refuses with the `dotnet-unavailable` marker; the ratchet
  skips such baseline cases locally and `--require-dotnet` (CI) makes them an error.

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
