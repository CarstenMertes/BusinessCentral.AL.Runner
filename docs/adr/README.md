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
| [007](007-oracle-and-ratchet.md) | Oracle = corpus CI JUnit for BC 27.5; passing set is a ratchet (one oracle and one ratchet per BC major since ADR-015) | Accepted |
| [009](009-observable-equivalence.md) | The contract is observable behaviour; internals, performance, diagnostics and tooling may improve on BC | Accepted |
| [008](008-builtins-catalog.md) | Builtin signatures generated from the docs; implementations registered by overload id; value model in al-builtins | Accepted |
| [010](010-value-model.md) | One Value enum in al-builtins; value types copy, reference types share; records are handles | Accepted |
| [011](011-ir-and-execution.md) | Fully resolved typed IR; tree-walking interpreter first, bytecode only if budgets need it; Result-based control flow; BC test-runner semantics from the corpus | Accepted |
| [012](012-testpage-runtime.md) | Headless page objects in the IR and a TestPage client over page instances in the same session | Accepted |
| [013](013-external-objects.md) | Dependency apps compiled from Microsoft's own artifacts (source, cached outside the repo); native providers only for platform objects with no AL body; version selection from app.json as in AL Runner; AL Runner's identity values and install baseline | Accepted |
| [014](014-dotnet-provider-seam.md) | `DotNet` calls: a trait in `al-host`; a .NET 8 sidecar process per Session loads real assemblies; a runtime registry in front serves server classes that need a service tier; refused without .NET | Accepted |
| [015](015-version-independent-runtime.md) | One codebase for BC 27 and every later major: a `Target` value, version-tagged builtin catalog, named behaviour gates, one oracle and one ratchet per version | Proposed |
| [016](016-release-al-runner-v3.md) | Release: the runtime ships as AL Runner v3 with its own user command line (`alrunner`), a plain GitHub Actions release matrix, an optional .NET helper asset, WebAssembly hosts, and readiness gates; v2 feature parity tracked in `docs/al-runner-v2-parity.md` | Proposed |
| [017](017-builtins-round-2-defaults.md) | Defaults for error collection, partial records, media and the call depth ceiling, chosen without owner review | Accepted |
| [018](018-fixes-2-defaults.md) | Defaults chosen in the second round of corpus fixes (background worker transaction, LogMessage, SessionSettings handler, unevaluable InitValue), without owner review | Accepted |
| [019](019-platform-table-providers.md) | Platform tables: a registry of providers keyed by table id, a `VirtualTables` seam in the store, poisoned rows and unpinned columns refused, stored platform tables seeded with the session's facts | Accepted (without owner review) |
| [020](020-bcdb-test-data.md) | `al-bcdb` reads BC data from a `.bak` or `.bacpac` with no OS access; a table provider in `al-host` loads CRONUS into the store on first touch (`alrunner test --test-data`) | Accepted, built |
| [021](021-table-globals-per-record-variable.md) | A table's (and table extension's) global variables live as long as the record variable: `RecordState` owns a lazily created `Instance` per object, `Clear` and `Reset` reset them, copies start fresh, `xRec` has its own persistent set | Accepted (owner, 2026-10-14; details without owner review) |
| [022](022-compile-cache.md) | Compile cache: on by default; layers (owner, 2026-10-15): Microsoft's apps are layer 1, every other dependency app its own layer keyed with the key below, so a change in one's own app never rebuilds Base Application; entries per layer (workspace apps ordered last; object index, event index, member tables, per-object zstd IR blocks with a shared dictionary, lazy decode, blocks re-lowered when the workspace extends their objects); key = apps, versions, source hashes, target, runtime build; local only; CI caches the files and builds them in a separate step | Accepted |
| [023](023-page-extension-base-access.md) | Page and report extension code that names a base object's `protected var` or procedure: one instance per extension with a link to the base instance, `Place::BaseGlobal` and `Expr::BaseInstance` in the IR; built behind probe gates | Accepted |
| [024](024-dotnet-runtime-typed-values.md) | `DotNet` values whose type is known only at run time: settled at the point of use (operand, builtin argument, `foreach`), `CopyStream` to and from a .NET `Stream` through two sidecar ops, no assembly reflection at compile time; probes in the OnPrem corpus app | Accepted (owner, 2026-10-14), build deferred |
| [025](025-multiple-reportextensions-per-run.md) | Several reportextensions that change one report run: `Report::extensions` and `Run::exts` as lists, object id order, placement next to the anchor, cross-app order refused, cache facts and format 2 | Accepted (owner, 2026-10-14) |
| [026](026-incremental-compile.md) | Incremental compile: measured split of compile time (app under test about 2 s, a changed dependency set 9 to 13 s); recommends no per-file or per-object incremental compile; parallel `resolve_references` and layered dependency entries built 2026-10-15 (owner), NP Retail measured | Proposed |
| [027](027-scope.md) | Scope: what v3 runs as BC, replaces faithfully, refuses by design (email, HTTP, rendering, background sessions, licensing, compiled-only apps), and defers; open owner decisions | Accepted |
