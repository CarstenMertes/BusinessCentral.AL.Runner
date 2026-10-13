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
| Handlers | The full text of the "Unhandled UI" error. Corpus pins `Unhandled UI`, the handler type (`Message`, `Confirm`, `ModalPage`) and, for Confirm, the question; the only full line is a comment quoting a CI log (`Unhandled UI: Message  <text>`, two spaces). Implemented as `Unhandled UI: <Kind>  <text>`; for StrMenu the text is the options string. Its error code is empty | corpus handlers/TestConfirmNoHandler.al, pageextensiontrigger/TestModifiedControlValidateTriggers_Tests.al:162 | `al-vm/src/ui.rs` `dispatch_ui` |
| Handlers | The text after `The following UI handlers were not executed` (implemented as `: ` plus the handler function names), and whether the check also runs when the test already failed (implemented: the test's own error wins) | corpus handlers/TestPageQueryCloseMessageHandler_Tests.al:51 (prefix only) | `al-test-cli/src/runtime.rs` `run_codeunit` |
| Handlers | An error raised inside a handler function comes out of the `Message`/`Confirm` call as an ordinary, catchable error | none (Assert-based handlers in handlers/TestMessageHandler.al only pass) | `al-vm/src/ui.rs` |
| Handlers | A UI call raised from inside a handler function, and a UI call outside a test method (OnRun, install code): both refused as Unsupported | none | `al-vm/src/ui.rs` `dispatch_ui` |
| Handlers | Two distinct handler functions of one kind in one `[HandlerFunctions]` list: refused (BC may reject it at compile time or bind the first) | none | `al-test-cli/src/runtime.rs` `resolve_handlers` |
| Confirm | An omitted `Default` is false | doc dialog-confirm-method | `al-builtins/src/native.rs` `confirm` |
| StrMenu | An omitted `DefaultNumber` is 1, an omitted `Instruction` is empty, and the handler's `Choice` starts at the default | doc dialog-strmenu-method; corpus network/TestHttpClient.al:36 sets `Choice` unconditionally | `native.rs` `str_menu` |
| TestPage | `GetValidationError(0)` on the page raises a catchable error; the text is not pinned (the test only needs it non-empty) | corpus handlers/TestPageValidationError_Tests.al:Claim 9 | `al-vm/src/pages.rs` `test_page_op` |
| TestPage | a control that is not on the runtime page raises `The field <name> is not found on the page.`; BC's text carries a compiler-assigned control id (`The field with ID = N ...`) we cannot derive | corpus handlers/TestPageFieldVisibleGroup_Tests.al (substring only) | `control_eliminated` |
| TestPage | a Guid control shows lower-case hex in braces (the corpus only has digits) | .NET `Guid.ToString("B")` | `al-builtins/src/pagefmt.rs` `guid_text` |
| TestPage | `BlankNumbers` members other than `BlankNeg` / `BlankZeroAndPos` (`BlankNegAndZero`, `BlankZero`, `BlankPos`) blank what their names say | doc property page | `pagefmt::blanked` |
| TestPage | a negative Duration spells as `-` plus the words | none | `pagefmt::duration_text` |
| TestPage | GoToKey / GoToRecord scan forward from the FIRST row and populate every row they pass (OnAfterGetCurrRecord too); the corpus pins only that the scan passes a failing row and restores the cursor on a miss | corpus autoformat/TestAutoFormatExpressionTiming.al, handlers/TestPageGoToRecordNonKeyRefresh_Tests.al | `page_go_to_key` |
| TestPage | a repeater populates a window of 20 rows (pinned between 20 and 24) and re-populates a window starting at a row outside it | corpus autoformat/TestAutoFormatExpressionTiming.al:5-12 | `WINDOW` |
| TestPage | OnInsertRecord receives BelowxRec = false when the insert is the client's own | none | `insert_draft` |
| TestPage | SetValue of the value a field already holds runs no OnValidate trigger | corpus handlers/TestPageSameValueSetValue_Tests.al pins only that OnModify does not run | `control_set_value` |
| TestPage | a page that showed no row shows one when page code left Rec on a stored row that the view admits and that was not stored before the action; the same for any action | corpus testpart/BnrBlankPart.al NoRow_Action_* | `show_row_page_code_positioned` |

