# ADR-032: Typed ids across seams

- Status: Accepted (2026-10-15, decided by the orchestrator on the owner's delegation; see docs/owner-review.md)
- Date: 2026-10-15
- Related: ADR-008 (builtins by overload id), ADR-010 (value model), ADR-015 (version-tagged catalog), review
  `docs/reviews/2026-10-compile-ir-review.md` (R5, section 5.2)

## Context

Several seams carry strings where the set of values is closed and known at compile time:

| What | Where | Sites today |
|---|---|---:|
| Object kind | `al_ir::Object::kind: String`; `al-sema` has `ObjKind`, the IR drops it | 104 string compares in `al-vm`, `al-sema`, `al-testrun`, CLIs; invented kinds such as `"uncompiled report"` (`lower.rs:1515`) |
| Builtin id | `Expr::Builtin { id: String }`; three registries (11 static slices, 12 `native_for` derivers, `platform_native::lookup`) | 93 literal matches and 27 `starts_with` in `al-vm`; 9 in `al-sema` |
| Record and page operations | `record_op(method: &str, ..)`, `Ctx::record(method: &str)` | 57 callers, 285 match arms on method names |
| Library types | `RtType::Library(String)`, `Value::Library(String, ..)` | 26 name-guarded matches |
| Properties read at run time | `Object::properties: Vec<(String, String)>` | 128 `eq_ignore_ascii_case` in `al-vm` (`TableType`, `PageType`, `EventSubscriberInstance`, `FieldClass`, ...) |
| Company | `MemStore` tables keyed by `(String, u32)`; `AccessContext` clones `String`s per operation | every record operation |

With strings, the compiler cannot tell an agent where a new kind or builtin must be handled. Each feature adds more
sites. The strings also make the cache larger and its decode slower (ADR-022: strings are two thirds of the raw
bytes; decode is allocation work).

## Decision

1. **`ObjKind` moves to `al-ir`** (it is plain data) and `Object::kind` becomes `ObjKind`. `al-sema` re-exports it.
   Kinds the lowering invents (`uncompiled report`) become explicit fields (`Object::uncompiled: bool`), not
   kinds.
2. **`BuiltinId`**: a `u16` newtype with a generated table (`al-builtins/src/builtin_ids.rs`, generated from the
   catalog by `build.rs` or a checked-in generator with a test that it is current). `Expr::Builtin` carries a
   `BuiltinId`. The id string stays available (`BuiltinId::name()`) for refusal texts and the cache's
   human-readable dumps. The catalog of every supported BC version is in the table; availability per target stays
   in the catalog (ADR-015). One registry: `NATIVES: [Option<NativeFn>; N]` indexed by id; the VM's own builtins
   are entries in it.
3. **`RecordOp` and `PageOp` enums** replace the method-name strings of `Ctx::record`, `Ctx::page`, `record_op`
   and `page_op`.
4. **`LibraryKind` enum** replaces the `String` in `RtType::Library` and `Value::Library`.
5. **Typed schema facts** replace run-time property reads: lowering puts the properties the VM reads into typed
   fields (`Table::temporary`, `Table::crm`, `Page::page_type`, `Routine::subscriber_instance`, ...).
   `Object::properties` stays for the platform tables that list them as text.
6. **`CompanyId`**: the store interns company names; tables are keyed by `(CompanyId, u32)`.

## What changes at which seam

- `al-ir`: `Object::kind`, `Expr::Builtin::id`, `RtType::Library`, new typed schema fields. Cache format bump.
- `al-builtins`: `Value::Library`, `Ctx::record` and `Ctx::page` signatures, the registry (amends ADR-008: "keyed by
  overload id" stays true; the id is now an enum generated from the catalog).
- `al-data`: `Store` keeps its trait; `MemStore` internals and `AccessContext` change (company by id).

## Proof of zero behaviour change

- Each decision is a pure refactor. `cargo run --release -p al-test-cli --example compile_dump` before and after:
  the dumps are identical after mapping the typed fields back to their text (the dump prints `ObjKind` and
  `BuiltinId` by name).
- Corpus (every major) and Tests-ERM and Tests-SCM outcomes identical (`tools/compare_junit.py`).
- `tools/demand_vs_eager.sh`, `tools/cache_vs_uncached.sh`, `tools/baseline_rule_check.sh`: 0 differences.
- A test that every catalog id of every version has a `BuiltinId`, and every registered native has a catalog id.

## Migration (each step merges green; one enum per branch)

1. (Done on branch objkind-ir, 2026-10-16.) `ObjKind` in `al-ir` (largest fan-out; do it first while few branches are open).
2. (Done on branch builtin-id, 2026-10-16.) `BuiltinId` and the single registry. A checked-in generated table (`al-builtins/src/builtin_ids.rs`, `cargo run -p al-builtins --example gen_builtin_ids`, test `builtin_ids_current`) holds every catalog overload, registered native and invented id, sorted; one constant per id (`ids::RECORD_INSERT_2`) serves `match`. The cache stores the `u16` index; JSON dumps print the name. `native::by_id` is the array registry; the VM branches on `BuiltinId::class()` and the constants, with no id string in `al-vm`. Cache FORMAT 8.
3. `RecordOp`, `PageOp`.
4. `LibraryKind`.
5. Typed schema facts, one property at a time.
6. `CompanyId` in `MemStore`.

## Consequences

- Unblocks: ADR-033 (symbolic references use `ObjKind`), ADR-034 (pre-resolved builtins), ADR-035 (company ids).
- Blocks: nothing; but feature branches that add builtins or kinds conflict with steps 1 to 3. Do them while
  feature work is paused.

## Conformance

The catalog-coverage test (decision 2). `compile_dump` equality in the merge report. The corpus and bucket
outcome comparison.
