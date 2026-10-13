# Probes needed

Behaviour implemented from documentation only, because no corpus test pins
it yet. Each entry needs a probe test on a branch of the corpus repo (hard
rule 3, ADR-007); once its CI result is in, the entry moves into the code
comment as a corpus citation and is deleted here.

| Area | What is assumed | Source today | Code |
|---|---|---|---|
| Format | Decimal format 0/1 uses `,` thousands and `.` decimals (en-US), decimals trimmed of trailing zeros | doc table "Standard decimal formats" (US) | `al-builtins/src/format.rs` `format_decimal` |
| Format | Date format 0/1 is `MM/DD/YY`, format 2 `MMDDYY[C]D` (en-US) | doc table "Standard date formats" (US) | `format_date` |
| Format | Decimal format 2 trims trailing zeros (`1.50` → `1.5`) | doc `<Decimals>` field, no precision | `format_decimal` |
| Format | Boolean format 9 is `true`/`false` | doc table "Standard boolean formats" | `format` |
| Error | `Error(Text, ...)` sets `GetLastErrorCode()` to `Dialog` | Microsoft test libraries; corpus Assert `DialogTxt` label | `al-builtins/src/native.rs` `error_text` |
| Operators | `and` / `or` evaluate both operands (no short circuit) | C/AL tradition, not documented for AL | `al-vm` `eval` `Binary` |
| Operators | Text `=`/`<` compare ordinally and case-sensitively | none | `al-builtins/src/ops.rs` `compare` |
| asserterror | the message when the block raises nothing is `An error was expected inside an ASSERTERROR statement.` | BC message as commonly reported | `al-vm` `exec` `AssertError` |
| Code | assigning to Code upper-cases and trims leading/trailing spaces | doc "Code data type" | `ops::convert` |
| Record.Find | how `Find('=')` treats the record's active filters (two corpus tests pin opposite-looking cases) | corpus `OwnFilter_*` tests | `al-vm/src/records.rs`, Unsupported meanwhile |
| Record errors | `The record in table %1 already exists. Identification fields and values: %2` and `The %1 does not exist. Identification fields and values: %2` (corpus pins only the fragments) | BC messages as commonly seen | `al-vm/src/records.rs` `fail` |
| Temporary records | not affected by `asserterror` rollback | none | `al-vm` `AssertError` |
| Install | OnInstallAppPerDatabase runs before OnInstallAppPerCompany | doc "Writing extension install code" | `al-test-cli/src/runtime.rs` |
| SetFilter | text filter values on a Code field upper-case before matching (`'ap*'` = `'AP*'`); Boolean filter text accepts Yes/No/true/false | Code conversion rule; corpus uses `Format(false)` as a filter value | `al-builtins/src/filter.rs` |
| Events | `DeleteAll(true)` raises the delete events (Before, After) once per row, with the trigger, like `Delete(true)` per row; the corpus pins only the per-row OnDelete trigger | corpus `record/TestTriggerContracts.al:DeleteAll_WithTrigger_FiresForEachFilteredRecord` (trigger only); Truncate guard text "use DeleteAll" | `al-vm/src/records.rs` `DeleteAll` |
| Events | `DeleteAll(false)` on a table with delete event subscribers: whether the events fire per row. REFUSED (Unsupported) when rows would be deleted; nothing is assumed | corpus `record/TestRecordTruncateEventAndFilterGuards.al` implies it | `records.rs` `DeleteAll` |
| Events | `OnBeforeValidateEvent` sees the new value in `Rec` and the old row in `xRec` (the field is assigned first) | BaseApp practice; corpus logs the values but asserts only the After event | `records.rs` `Validate` |
| Events | `CurrFieldNo` passed to the validate events is 0 for a `Validate` called from code | corpus pins 0 only for the field's own OnValidate (`handlers/TestPageCurrFieldNo_Tests.al`) | `records.rs` `table_event` |
| Events | `RunTrigger` passed to the rename events is true (Rename always runs the trigger) | none | `records.rs` `Rename` |
| Events | Table events for temporary records: do they fire? REFUSED when a subscriber would be reached | the only corpus case is Base App (`record/TestTableEventAsyncSubscriberError.al`) | `records.rs` `table_event` |
| Events | Insert/Modify/Delete/Rename of a duplicate or missing key on a table with event subscribers: whether the Before event fires before the error. REFUSED | none | `records.rs` |
| Events | One `xRec` handle is shared by the Before event, the trigger and the After event of an operation (a subscriber's change to it reaches the trigger too); the corpus pins only subscriber-to-subscriber visibility | `record/TestTableEventByRefMutation.al` | `records.rs` |
| Events | A static (StaticAutomatic) subscriber codeunit that is not `SingleInstance` and keeps global variables: whether it gets a fresh instance per event. REFUSED | none | `al-vm/src/dispatch.rs` `static_instance` |
| Events | Subscriber order among several subscribers of one event is not pinned (program order is used); no test may depend on it | corpus `codeunit/SISCache.Codeunit.al` comment | `dispatch.rs` |
| Events | `BindSubscription` on an already bound instance as a statement raises an error whose text contains `already been bound`; the full text and the error code are not pinned | corpus `codeunit/TestEventManualBinding.al` Contract 8 (fragment only) | `dispatch.rs` `subscription` |
| Events | `BindSubscription(<non-Manual codeunit>)` as a statement (the Boolean form returns false): REFUSED; `UnbindSubscription` of a non-Manual codeunit returns false without raising (only "no error" is pinned) | `codeunit/TestEventContracts.al` Contract 5/6 | `dispatch.rs` `subscription` |
| Events | Subscribers to the Base App `Global Triggers` codeunit (`OnDatabaseInsert`, `OnGlobalRename`, ...) fire in BC around record writes; here the codeunit does not exist, so writes to the tables those subscribers name are REFUSED | `record/TestTriggerDispatchOrder.al` | `al-vm/src/events.rs` `global_triggers_apply` |
| Records | The `DB:RecordChanged` message for a stale Modify is the pinned sentence followed by `Identification fields and values: ...` (only the sentence and the key fragment are pinned) | `record/TestRecordStaleBufferWrite.al` | `records.rs` `Modify` |
| Records | When a field's OnValidate raises, only that field goes back to its old value (other fields the trigger changed stay) | `record/TestTriggerRollback.al:OnValidate_Throws_FieldValueRolledBack` pins the field only | `records.rs` `Validate` |
| asserterror | How far a plain `Error()` caught by asserterror rolls back writes made earlier in the same test when no `Commit()` ran in that test: `error-handling/TestAssertErrorRollback.al` (Initialize commits) rolls them back, `codeunit/TestScopeIsolationContracts.al:Test04...` and `session/TestTransactionContracts.al:Error_After_Insert_Before_Commit_RecordPersists` keep them. The runtime refuses (Unsupported) that case | contradictory corpus tests | `al-vm/src/lib.rs` `AssertError` |
| System fields | SystemCreatedAt/By and SystemModifiedAt/By are not written (no clock, no user security id); reading or filtering on them is refused at lowering | `record/TestBCSystemFieldContracts.al` | `al-sema/src/lower.rs` `readable_field_pos` |
