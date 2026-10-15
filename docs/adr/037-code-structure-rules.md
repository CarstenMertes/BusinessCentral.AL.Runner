# ADR-037: Code structure rules

- Status: Accepted (2026-10-15, decided by the orchestrator on the owner's delegation; see docs/owner-review.md)
- Date: 2026-10-15
- Related: ADR-001 (Engine and Session), review `docs/reviews/2026-10-compile-ir-review.md` (section 5, R9)

## Context

Many agents work in parallel. The structure makes their changes collide and drift:

- Large files with many owners: `al-sema/src/lower.rs` 7 479 lines (one `impl` of about 3 100 lines, 1 680 lines of
  inline tests), `al-vm/src/pages.rs` 4 703, `al-testrun/src/runtime.rs` 4 573 (1 720 lines of inline tests),
  `al-sema/src/check.rs` 3 607, `al-builtins/src/xml.rs` 3 542, `al-vm/src/lib.rs` 2 267 with a `Session` of 83
  fields.
- Duplicates: three where-condition parsers (`calcformula.rs`, `relation.rs`, `pages.rs`), view text parsed at run time
  in two places (`al-vm/views.rs`, `al-vm/pages/request.rs`), 5 copies of `split_top`, 7 of `unquote` (while
  `al_syntax::ast::unquote_ident` exists), 3 month and day name tables, 6 type-name renderers, two JUnit writers, a
  `coverage.rs` in three crates with different meanings.
- Tests: 52 test binaries in `al-vm/tests`, 44 with their own compile helper (`fn run` copied 39 times); no shared
  test kit.
- Layering: `al-test-cli` calls `al_sema::check` directly.
- Version gates: 7 of 16 are never referenced; two places read `bc.major` directly.

## Decision

1. **File size.** Non-test code above 2 000 lines in one file is split before it grows further. `check.sh` warns
   above 2 000 and fails above 3 000 lines of non-test code (owner decision D7: warn only, or fail). Existing files
   get an exception list that only shrinks.
2. **Inline tests** above 300 lines move to `tests/` or to a `tests.rs` child module.
3. **Splits**:
   - `al-sema/src/lower.rs` -> `lower/{mod,shared,stmt,expr,call,convert,pages,reports,xmlports,dotnet,assemble}.rs`.
   - `al-sema/src/check.rs` -> `check/{mod,posmap,infer,calls,fit}.rs` (before ADR-036 replaces parts of it).
   - `al-vm/src/lib.rs`: `Session` holds sub-states (`TxState`, `UiState`, `EventState`, `CoverageState`,
     `ReportState`, `PageClients`, `DotNetState`); the interpreter moves to `interp.rs`; the `Ctx` implementation
     to `ctx.rs`.
   - `al-vm/src/pages.rs` -> `pages/{open,navigate,controls,close,triggers}.rs`.
   - `al-testrun/src/runtime.rs` -> `runtime/{mod,compile,cache,run,isolation}.rs`.
4. **One implementation per grammar and helper:** one where-condition parser in `al-sema` used by CalcFormula,
   TableRelation, page links and query filters; view text parsed at lowering into a typed view; one `unquote`, one
   `split_top`, one month and day table, one type-name renderer (`al-builtins/describe.rs`).
5. **A test kit crate** `al-testkit` (dev-dependency only): compile an app from inline AL, run a codeunit, read
   outcomes. `al-vm/tests` uses it and is merged into a few binaries by area.
6. **Layering:** the CLIs reach `al-sema` only through `al-testrun`. One JUnit writer in `al-testrun`.
7. **Gates:** unused gates are removed or used; `bc.major` is read only in `al-version`.

## What changes at which seam

No public seam changes. Each split is a move inside a crate. `al-testkit` is a new dev-only crate. The JUnit writer
moves to `al-testrun`.

## Proof of zero behaviour change

- Moves: `cargo run --release -p al-test-cli --example compile_dump` identical before and after; corpus outcomes
  identical.
- Parser merges: unit tests of every old parser kept and run against the shared one; `compile_dump` identical on the
  corpus and the buckets (the parsed conditions end up in the IR).
- Test kit: the number of tests before and after is the same (`cargo test -- --list | wc -l`).

## Migration (each step merges green)

Each split is one short branch, done when no open branch touches the file (the plan schedules them). Order:
test kit, `unquote` and helpers, `lower.rs`, `check.rs`, `Session` sub-states, `pages.rs`, `runtime.rs`, the
where-condition parser, view text at lowering.

## Consequences

- Fewer merge conflicts and smaller reviews for every later change; test builds link fewer binaries.
- Blocks: the splits of `lower.rs` and `check.rs` must land before ADR-033 step 1 and ADR-036, or they collide.
- Cost: 6 to 10 agent-days.

## Conformance

The size rule in `check.sh`; `codemap.rs` (the code map test) keeps the module list current.
