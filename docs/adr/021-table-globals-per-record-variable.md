# ADR-021: Table globals per record variable

- Status: Accepted (owner, 2026-10-14: the principle in decision 1). The detailed rules in decisions 2 to 8 come from probes on real BC 27.5 and are listed under "Decided without owner review".
- Date: 2026-10-14
- Changes: the shape of `RecordState` (ADR-010 decision 4) and how a table procedure or trigger gets its `Instance`. Additive; no seam is removed.

## Context

A table's `var` section declares global variables. Until now every call of a table procedure or
trigger on a record built a new `Instance` of those globals, so state one call set was gone at
the next call. In BC the globals live as long as the record variable. Found on Microsoft's
Tests-ERM with test data: Base App "Price List Line" sets the global `PriceAsset` in the
"Asset Type" `OnValidate` and reads it in the "Amount Type" `OnValidate`.
`T043_AmountTypeMustBeDiscountForItemDiscGroup` (Price List Line UT and Price Worksheet Line UT)
failed because the check that reads `PriceAsset` was skipped.

The probes (corpus branch `al-runtime-rs`, head `eb46da5`, `tests/al-language/tableglobals/`, 58
tests, all green on BC 27.5; the corpus submodule does not hold them yet) pin what each operation
does to the globals.

## Decision

1. **The globals belong to the record variable (the `RecordState`), not to a call.** Every call
   of a procedure or trigger of the table, or of a table extension, on that variable runs on the
   same globals. A second variable of the same table has its own. A table extension has its own
   globals, apart from the table's, per record variable.
2. **Representation.** `RecordState` gets a field `globals: TableGlobals` (al-builtins). It holds
   one `Rc<RefCell<Instance>>` per object that declares globals (the table, each extension),
   created on the first call, and a second `TableGlobals` for `xRec`. Cloning a `TableGlobals`
   gives an empty one: a copy of a record state starts with fresh globals. Code that wants to
   keep the globals of a state (`:=`, `Copy`, `TransferFields`) copies fields only, as
   ADR-010 already says.
3. **No `Rc` cycles.** The `Instance` of a table holds `Rec` and `xRec` as global slots that point
   back to the record. They are filled for the duration of a call and put back to what they held
   before it (`Null` outside any call), also when the call fails. So the state owns its globals,
   and the globals do not own the state at rest. A nested call on the same record (a procedure
   calling `Rec.Other()`) saves and restores the slots, so it never clears what the outer call
   still reads. A global record variable of the table's own table (`Child: Record "Same"`)
   creates its own `TableGlobals` on its first call, so nothing is built recursively.
4. **Lifetime rules** (measured, listed in the Conformance section by probe):
   - Kept across: procedure calls, field triggers, `OnInsert`/`OnModify`/`OnDelete` of
     `Insert(true)`, `Modify(true)` and `Delete(true)` on the variable (they run on its instance),
     `Get`, `Find*`, `Next`, filters, marks, `Init`, `Validate`, `ChangeCompany`, `Insert`,
     `Modify`, `Delete`.
   - Reset (set to initial values): `Clear(Rec)` and `Rec.Reset()`. Nothing else.
   - Not moved by `A := B`, `A.Copy(B)`, `A.TransferFields(B)`, `Rec := Other` inside the table,
     `RecordRef.GetTable`/`SetTable`, Variant round trips and returning a record: they copy field
     values and leave both sides' globals alone.
   - A record passed by value is a copy with fresh globals. Passed by var it is the same record.
     `Codeunit.Run(.., Rec)` with `TableNo` runs on the caller's record.
5. **`xRec`** is a second `TableGlobals` owned by the record variable. It survives `Reset` and
   `Clear` of `Rec`, and it is one instance for `OnValidate`, `OnModify`, `OnDelete`, `OnRename`
   and the event subscribers of that variable. An `xRec` built for a call (the old row image)
   takes its globals from the variable.
6. **Bulk operations.** `ModifyAll(.., true)` and `DeleteAll(true)` run `OnModify`/`OnDelete` of
   all rows on one fresh instance that is not the caller's. `ModifyAll` runs no field validation.
7. **Event subscribers** receive the caller's record (`var Rec`) and its `xRec`, so their reads and
   changes of the globals show in the trigger and in the caller.
8. **Pages.** The page's `Rec` keeps its globals across actions and rows. A field validation
   through the page runs the table's `OnValidate` on fresh globals, not the page record's, and
   leaves the page record with fresh globals afterwards. `Insert` and `Modify` that follow it
   see fresh globals as well. The client clears the page record's globals before and after each
   validation.
9. **Instances of other objects are unchanged**: codeunit, page, report, query and xmlport
   instances keep their own lifetime.

## Consequences

- Table code that keeps state in globals now works: Base App's pricing, sales and posting tables.
- `RecordState` grows by one small field; a call on a record no longer builds an `Instance`
  (the global defaults, including nested `RecordState`s) every time, so record-heavy code gets
  cheaper where tables declare globals.
- A struct copy of `RecordState` (`..r.clone()`) now means "same fields, fresh globals". Code
  that overwrites a whole state must keep `globals` of the target on purpose.
- Table procedures and triggers have no `Rc` back-reference at rest, so a record variable is
  freed when its last handle goes away.

## Conformance

Unit tests in `crates/al-vm/tests/table_globals.rs`, one per rule above, with their own fixtures.
Each cites the probe test by corpus branch and head until the corpus bump brings the probes into
the submodule: corpus `tests/al-language/tableglobals/TestTableGlobalsRecordVariable.al` (rules 1
to 7) and `TestTableGlobalsHolders.al` (codeunit-held records, pages). Real-world check: Tests-ERM
with `--test-data` (docs/alrunner-buckets.md), including the two `T043` tests.

## Decided without owner review (2026-10-14)

Everything in decisions 2 to 9. They come from probes, not from guesses, with these exceptions
that are assumptions or choices:

- The page client clears the page record's globals before and after a validation. Measured: the
  trigger sees fresh globals, and the page record is fresh afterwards, `OnInsert` and `OnModify`
  after it see fresh globals. A page `Insert` or `Modify` with no validation before it
  (a draft row inserted by `New()` and `Close()`) runs on the page record's own globals. Not
  pinned (`docs/probes-needed.md`).
- `xRec` in a plain procedure call (not from a trigger) is still a copy of `Rec`'s fields taken at the
  call, with the variable's `xRec` globals. Its field values there are not pinned.
- Records inside `array of Record`, and `RecordRef.Open` instances, get the same per-state
  globals by construction; only `RecordRef` validate-then-insert was probed.
