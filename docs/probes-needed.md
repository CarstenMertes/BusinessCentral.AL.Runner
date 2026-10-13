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
| RecordRef.Open | the text around the two pinned halves of the missing-table error: `You tried to invoke the Table object with the ID N from the object (caller not recorded). An object with that ID does not exist in the current application.` | corpus pins only the two fragments (recordref/TestRecordRefOpenMissingTable.al) | `al-vm/src/refs.rs` `rr_open` |
| RecordRef | `FieldCount` / `FieldIndex` skip the system fields and walk the fields in field-number order | none (the corpus only checks `> 0` / `>= 10`) | `refs.rs` `enumerable_fields` |
| RecordRef | `GetTable(Rec)` copies the Record's buffer, filters, key and direction into the ref; `SetTable(Rec)` copies the ref's into the Record. The corpus pins only that the other side is left alone (recordref/TestRecordRefGetSetTableContracts.al) | doc `RecordRef.GetTable` / `SetTable` | `refs.rs` |
| RecordRef | `Duplicate` is a fully independent ref (its own buffer and filters); a temporary ref is refused | doc | `refs.rs` |
| KeyRef / FieldRef | `KeyRef.Active` and `FieldRef.Active` are true for every key and field (no `Enabled = false` exists in the corpus; the lowerer does not read it) | corpus pins the true case only | `refs.rs` |
| FieldRef | `IsEnum` is false and `EnumValueCount` is 0 for a field that is neither Enum nor Option; `OptionCaption` without an `OptionCaption` property is the member names | none | `refs.rs` `enum_metadata` |
| Views | `GetView` is `SORTING(<key fields>) [ORDER(Descending)] [WHERE(<field>=FILTER(<text>),...)]` (`Field<n>` with `UseNames = false`); `SetView` replaces the key, the direction and the filters of the current filter group | the corpus feeds `SORTING(Entry No.) WHERE(Entry No.=FILTER(1..3))` to `SetView` and only round-trips `GetView` | `al-vm/src/views.rs` |
| Filters | `GetFilters` is `Caption: filter` pairs joined by `, ` in field order; a filter reads back as `a..b`, `a`, `<>a`, `>a`, `a\|b`, `a&b` (values needing quotes are refused) | the single-field forms are pinned by testfilter/ and record/TestCopyOntoTemporaryRecord.al | `views.rs` |
| FilterGroup | `FilterGroup(n)` returns the group in use before the call; `Reset` clears every group but leaves the current group number alone; `GetFilter`/`HasFilter`/`GetFilters`/`GetView` read the current group only | docs; the corpus pins that groups AND and that Reset clears all | `recmeta.rs` |
| SetRecFilter | with filters on non-key fields it is refused (whether they stay is unknown); key-field filters are replaced | doc | `recmeta.rs` |
| Copy | `Copy(From)` copies buffer, filters, group, key and direction; `Copy(From, true)` onto a temporary shares the source's table; a temporary source onto a normal record is refused | corpus pins temporary-target copy (record/TestCopyOntoTemporaryRecord.al) | `recmeta.rs` |
| Mark | `Mark(SetMark)` with a used result is refused (previous state or new state is unknown) | none | `recmeta.rs` |
| TestField | the mismatch text is `<caption> must be <value> in <table>: <key>. Currently it's <current>.`; the key is `Caption=value` pairs joined by `, `; the error code is `TestField` | Assert.al's documented "new format" comment; the empty-field text is pinned exactly (record/TestRecordTestFieldLookupPageId_Tests.al) | `recmeta.rs` `test_field` |
| FieldError | `FieldError(Field, Text)` reads `<caption> <text> in <table>: <key>.`; `FieldError(Field)` without text is refused | docs example; the corpus only checks the caption and the custom text appear | `recmeta.rs` |
| Validate | a blank value is never checked against the TableRelation; the relation is checked before the value is stored and before OnValidate; the message is `The field F of table T contains a value (V) that cannot be found in the related table (R).` | the fragment `cannot be found in the related table` is pinned; a refused value is not stored (record/TestCalcFormulaSystemFieldsTests.Codeunit.al:253) | `al-vm/src/relations.rs` |
| SystemId | the glue of the duplicate-SystemId error (`There is already a record in table T that has the same values in a unique index. The conflict is in the following fields: System ID='...'.`) and of the ModifyAll refusal (`ModifyAll cannot change the value of the SystemId field.`) | fragments pinned only (record/TestRecordSystemId.al) | `records.rs`, `recmeta.rs` |
| Insert | `Insert` without `InsertWithSystemId = true` always assigns a fresh SystemId, ignoring the buffer's | doc `Record.Insert` | `records.rs` `assign_system_id` |
| RecordId | `Format(RecordId)` is `<table name>: <key1>,<key2>` | the corpus only needs the key values in it | `al-builtins/src/format.rs` |
| Platform options | `FieldType` and `IsolationLevel` member lists and ordinals, `SecurityFilter` ordinals: names are what AL writes, ordinals are unverified; `Format(x, 0, 2)` would print them | none | `al-builtins/src/sysenum.rs` |
| Locking | `RecordLevelLocking()` is true; `ReadPermission()`/`WritePermission()` are true | `ReadConsistency` false and the permissions true are pinned (record/TestRecordLock.al:64-71, TestRecordTriggers.al:234-252); `RecordLevelLocking` is doc only | `recmeta.rs` |
| CurrentKey / GetPosition | `CurrentKey` is the key's field names joined by `,`; `GetPosition` is `Name=CONST(value)` pairs of the current key then the missing primary key fields (`Field<n>` with `UseNames = false`); `SetPosition` writes the key values into the buffer without reading the row | the corpus only round-trips them | `recmeta.rs` |
| Truncate | the subscriber guard refuses on any table subscriber (which events are subscribed is not modelled); a filter on a normal field is refused (does it limit the delete?); a temporary table is refused | guards for the delete-event subscriber, FlowField filter and try scope are pinned | `recmeta.rs` `truncate` |
| TableRelation | a Rename follows an `if` arm only for rows whose conditions hold and the `else` arm for every row; a condition on a FlowFilter field is not evaluated; a relation onto SystemRowVersion relates to the primary key | pinned by record/TestRenamePropagation*.al, TestRenameKeyRelation.al, fieldref/TestFieldRefRelationFieldClass.al, TestCalcFormulaSystemFieldsTests.Codeunit.al | `rename.rs`, `relations.rs` |
