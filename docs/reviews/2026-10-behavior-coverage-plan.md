# Behavior coverage map: design note (Accepted; built, with the amendment that catalogs are runtime-known union Microsoft-used)

Task: extend `--coverage-map` beyond builtin overloads (1074 of 1729 pinned) so the corpus writer gets a gap list for the rest of the language. Status: design only, nothing built.

## Decision needed

Approve the six dimensions below, the output as a second file pair (`docs/corpus-behavior-coverage.md` + `.json`, same generator pattern, same ignored golden test), and "pinned" as static reach plus a syntactic or resolved match. No runtime run needed.

## Definition of pinned

A catalog item is pinned when a routine of a corpus test codeunit reaches it. Reach is the demand-driven call graph of the compile (as `AL_REACH_REPORT` in `al-testrun/src/reach.rs` computes it), seeded from test routines of the Workspace apps, restricted to routines in Workspace apps. Static, no run, deterministic. Each pinned item names one witness (file:line, first in file order, as the existing map does).

Why reach and not "appears in a file": a fixture helper no test calls pins nothing. Known limit: reach is not execution. A branch the test never takes counts as reached. Report it as "reached", not "observed". The oracle (real BC outcome per test) is the reason reached counts at all: the test's recorded outcome depends on the item.

Stronger variant, optional later: a dimension is "observed" if the test also passes on the runtime with a coverage counter. Out of scope now.

## Dimensions

| # | Dimension | Catalog source (real, not invented) | Size | Pinned test |
|---|---|---|---|---|
| 1 | Triggers per object kind | Trigger names the parser accepts in `al-syntax` grammar (`objects.rs`) crossed with `al_ir::ObjKind` (16 kinds); names the runtime dispatches, grepped from `al-vm`/`al-sema` string tables (about 60 `On*` names) | est. 100-150 (kind, trigger) pairs; exact count in step 2 | A trigger body in a Workspace object, reached, or a runtime-dispatched trigger fired by a reached builtin (Insert reaches OnInsert) |
| 2 | Behavioral object properties per kind | Property names `al-sema` reads (`scan.rs`, `members.rs`, `relation.rs`, `calcformula.rs`, `lower/tables.rs`, `pages.rs`, `reports.rs`, `queries.rs`, `xmlports.rs`). Allow-list lives in the generator, each entry checked by a test to still be read in al-sema | est. 60-90 (kind, property, value-class) | Property set in a Workspace object that is reached (any routine of the object reached, or the object is the target of a reached table/page operation) |
| 3 | Platform and system events | Platform `System.app` publisher codeunits/objects in the corpus dependency apps (`.alpackages`, via the existing `deps::load`): event publishers with `[IntegrationEvent]`/`[BusinessEvent]`, Global Triggers procedures, plus the record-event family (`OnBefore/AfterInsert/Modify/Delete/RenameEvent`, `OnBefore/AfterValidateEvent`). Table-event family is generated per table kind, so counted per event name, not per table | est. 40-80 names | An `[EventSubscriber]` in a Workspace app binding the event, and the subscriber reached or the publisher raise site reached |
| 4 | Statements and expression kinds | `SyntaxKind` variants in `al-syntax/src/syntax_kind.rs` (the Stmt and Expr groups, about 25) refined by operator token (binary, prefix, set `in`, range, ternary) and by statement shape (case with ranges, for downto, foreach over each collection type, assert error) | est. 60-80 | Syntax node in a reached routine body of a Workspace app. Needs a walk of the green tree per reached routine; sema span maps give the routine range |
| 5 | Runtime error sites | Every `raise(msg, code)` / `AlError {..}` construction in `al-builtins`, `al-vm`, `al-data` (147 sites by grep today; extracted with a source scan, keyed by file and normalized message template). Compile-error codes via `alcodes.rs` `IMPLEMENTED` list already exist and are a separate, cheaper subsection | est. 150-200 | Two levels. (a) Text pinned: a corpus test asserts the message/code (`asserterror` plus `GetLastErrorText`/`GetLastErrorCode` or `ExpectedError`) and the string occurs in the corpus; matched by template. (b) Site reached: static reach cannot tell which error a call raises, so (b) is only "a reached builtin that can raise it", from a hand-kept map. Proposal: ship (a) only |
| 6 | Types and conversions | Types from the builtin catalog's type list plus primitive types from `al-sema/src/types.rs`; matrix = (source type, target type) for assignment, and for the `Evaluate`/`Format`/option-enum conversions | est. 30 types, 900 cells, most invalid | A reached assignment, argument pass or `Evaluate`/`Format` whose checked source and target types are the pair. Needs the checker's expression types (`ExprTypes`, bound-0 plan); today only call resolutions are kept. Blocked until lowering fills `expr_tys` |

## Out of scope

- Execution counts, timing, branch coverage inside a routine.
- Dependency apps (Microsoft) as sources of pinned: only Workspace apps count, as today.
- Page control and report layout details beyond the property table in dimension 2.
- Value-level behavior (rounding modes, locale): that is the builtin dimension's job.
- Anything in the runtime-owned rework files: this work only reads.

## Build order (step 2)

1. Shared reach: a read-only function giving the set of reached Workspace routines. If `al-testrun::reach` has no public accessor, add one small additive `pub fn reached_routines` (the only change outside al-test-cli, additive, in `reach.rs`; not a rework-lane file).
2. Dimension 1, then 2, 3 (cheap, mostly name matching), then 4, 5(a). Dimension 6 waits for `expr_tys`.
3. Output: new `behavior_coverage.rs` in al-test-cli, flag `--behavior-coverage-map`, golden test `corpus_behavior_coverage_is_current` (ignored, release), red/green unit tests for the classification on tiny inline AL.
4. Report per dimension: totals, table of unpinned items grouped by kind, so a probe topic maps to a section.

## Risks

- Catalog drift: an allow-list in dimension 2 can go stale. A test fails when al-sema stops reading a listed property.
- Over-counting from reach (see above). Mitigation: label "reached".
- Dimension 5 string matching is heuristic; templates with `%1` placeholders are normalized before matching.
