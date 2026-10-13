# Architecture Decision Records

Seams between crates change only through an ADR. Agents fill in
implementations behind these seams; they do not move them.

To propose a change: copy `template.md` to the next number, status
`Proposed`, and get a human review before merging any code that depends on it.
A superseded ADR stays in place with status `Superseded by ADR-NNN`.

| ADR | Decision | Status |
|---|---|---|
| [001](001-engine-vs-session.md) | Immutable compiled program in an `Arc`; all mutable state in the Session | Accepted |
| [002](002-sync-execution-model.md) | Synchronous VM, one session per thread, `Rc`/`RefCell` inside | Accepted |
| [003](003-al-shaped-store-trait.md) | Store trait is AL-shaped: cursors, typed outcomes, transactions | Accepted |
| [004](004-snapshot-store.md) | Snapshots live on a separate `SnapshotStore` trait | Accepted |
| [005](005-company-and-access-context.md) | Company in every table identity; access context on every data op | Accepted |
| [006](006-core-purity.md) | No OS access in core crates; WASM-compatible | Accepted |
| [007](007-oracle-and-ratchet.md) | Oracle = corpus CI JUnit for BC 27.5; passing set is a ratchet | Accepted |
| [009](009-observable-equivalence.md) | The contract is observable behaviour; internals, performance, diagnostics and tooling may improve on BC | Accepted |
| [008](008-builtins-catalog.md) | Builtin signatures generated from the docs; implementations registered by overload id; value model in al-builtins | Accepted |
| [010](010-value-model.md) | One Value enum in al-builtins; value types copy, reference types share; records are handles | Accepted |
| [011](011-ir-and-execution.md) | Fully resolved typed IR; tree-walking interpreter first, bytecode only if budgets need it; Result-based control flow; BC test-runner semantics from the corpus | Accepted |
| [012](012-testpage-runtime.md) | Headless page objects in the IR and a TestPage client over page instances in the same session | Accepted |
