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
| CalcFields | the "must have the same type" error lays out its lines as `The following fields must have the same type:` blank line, then `Field:`, `Table:`, `Type:` lines separated by newlines (the corpus pins each line as a fragment, not the separators) | corpus `CFM Validation Tests` | `al-vm/src/flowfields.rs` `check_calc_field` |
| CalcFields | the recursion error reads `A FlowField formula depends on its own value. This can be caused by recursive function calls.` (the corpus pins only the second sentence) | corpus `CFV Tests` | `flowfields.rs` `calc_value` |
| CalcFields | the table named in `The X field in the Y table must be a FlowField.` / `You must define a CalcFormula ...` is the table caption (equal to the name for every corpus table) | none | `flowfields.rs` `table_caption` |
| CalcFields | the error code (`GetLastErrorCode`) of the CalcFields errors is empty | none | `flowfields.rs` `error` |
| CalcFields | a FlowField on a temporary record: the corpus only has a source table without rows (both "read the persistent source" and "stay uncalculated" give 0); with rows it is refused | corpus `SetAutoCalcFields_TempRecord_DoesNotCalculate` | `flowfields.rs` `calc_fields` |
| CalcFields | `lookup()` over several matching rows reads the first by some key BC picks; only rows that agree are accepted | corpus lookups have one match | `flowfields.rs` `calc_value_inner` |
| CalcFields | an Integer/BigInteger/Duration `average()` with a fraction of one half or more (truncate or round?) is refused; below one half both agree and is pinned | corpus `Average Quantity Int` (5.25 gives 5, 0.25 gives 0) | `flowfields.rs` `to_number` |
| CalcFields | `Reset` / `Clear` on a record with `SetAutoCalcFields` active: whether the auto-calc list survives is refused, not guessed | none | `al-vm/src/records.rs` |
| CalcFormula | two conditions on one source field other than `field()` + `field(filter())` (the pinned "totaling replaces the link" shape) are refused | corpus `CFF Tests` | `al-sema/src/lower.rs` `lower_calc` |
| CalcFormula | `-` before `min` / `max` / `lookup` is refused (only sum, count, average and exist are pinned) | corpus `CFS Header` | `flowfields.rs` |
| System fields | SystemCreatedAt/By and SystemModifiedAt/By are not stamped on writes (no clock or user in the core, ADR-006), so routines that read them are refused | corpus `CFSF Tests`, `TestBCSystemFieldContracts` | `lower.rs` |
| Test runner | a test that returns normally commits at the test boundary (default TransactionModel); a failing test is rolled back | corpus `TestWriteTxTestBoundary.al:26`, `CFV Tests` re-seed comments | `al-test-cli/src/runtime.rs` |
| Truncate | a filter on a stored field that excludes rows: does Truncate delete only the filtered rows? (the corpus filter keeps the only row, so both readings agree and are accepted; otherwise refused) | corpus `Truncate_WithFilterOnANormalField_Succeeds` | `al-vm/src/autoinc.rs` `truncate` |
| Truncate | the order of BC's seven guards and the messages of the five unpinned ones (temporary record, system table, Media fields, ...); a temporary record is refused | corpus `Test Record Truncate Guards` | `autoinc.rs` `truncate` |
| Unique index | the refusal message continues after `... in a unique index` (the corpus pins that prefix); its error code is empty; a duplicate primary key is reported before a unique-key violation when both apply; Modify and Rename on such tables are refused | corpus `AutoInc_UniqueKeyRefusedInsert_UnderAsserterror_*` | `records.rs` `fail_unique`, `violates_unique_key` |
| AutoIncrement | the Rename refusal lays out `Field:` and `Table:` on separate lines after the sentence (the corpus pins each as a fragment) | corpus `AutoInc_Rename_ThatChangesTheAutoIncrementValue_IsRefused` | `autoinc.rs` `check_rename_auto_increment` |
| Handlers | The full text of the "Unhandled UI" error. Corpus pins `Unhandled UI`, the handler type (`Message`, `Confirm`, `ModalPage`) and, for Confirm, the question; the only full line is a comment quoting a CI log (`Unhandled UI: Message  <text>`, two spaces). Implemented as `Unhandled UI: <Kind>  <text>`; for StrMenu the text is the options string. Its error code is empty | corpus handlers/TestConfirmNoHandler.al, pageextensiontrigger/TestModifiedControlValidateTriggers_Tests.al:162 | `al-vm/src/ui.rs` `dispatch_ui` |
| Handlers | The text after `The following UI handlers were not executed` (implemented as `: ` plus the handler function names), and whether the check also runs when the test already failed (implemented: the test's own error wins) | corpus handlers/TestPageQueryCloseMessageHandler_Tests.al:51 (prefix only) | `al-test-cli/src/runtime.rs` `run_codeunit` |
| Handlers | An error raised inside a handler function comes out of the `Message`/`Confirm` call as an ordinary, catchable error | none (Assert-based handlers in handlers/TestMessageHandler.al only pass) | `al-vm/src/ui.rs` |
| Handlers | A UI call raised from inside a handler function, and a UI call outside a test method (OnRun, install code): both refused as Unsupported | none | `al-vm/src/ui.rs` `dispatch_ui` |
| Handlers | Two distinct handler functions of one kind in one `[HandlerFunctions]` list: refused (BC may reject it at compile time or bind the first) | none | `al-test-cli/src/runtime.rs` `resolve_handlers` |
| Confirm | An omitted `Default` is false | doc dialog-confirm-method | `al-builtins/src/native.rs` `confirm` |
| StrMenu | An omitted `DefaultNumber` is 1, an omitted `Instruction` is empty, and the handler's `Choice` starts at the default | doc dialog-strmenu-method; corpus network/TestHttpClient.al:36 sets `Choice` unconditionally | `native.rs` `str_menu` |
