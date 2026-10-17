# Where the demo backup differs from the database Microsoft's tests run on

The test data of a run is the Cronus backup of the BC artifact (ADR-020). Microsoft's tests were written and gated on a
test database that is set up further. Where a test needs a setup value the backup lacks, the run fails or is refused for
a reason that is not a runtime defect. This file lists the differences that are proven, with the evidence, so that a
failure is not chased in the runtime twice. Each row was found by reading the backup (BC 27.5.46862.48827,
`BusinessCentral-W1.bak`) and confirmed by an experiment: the same tests, run with the value set by an install codeunit
of a scratch app (`alrunner test <test app> <scratch app>`), pass.

| Table, field | In the backup | A test needs | Effect (main 0b414672 or 23cc1e8f, full run) |
|---|---|---|---|
| Sales & Receivables Setup, `Posted Prepmt. Inv. Nos.`, `Posted Prepmt. Cr. Memo Nos.` | empty | a series | "Specify the code for the number series that will be used to assign numbers to posted sales prepayment invoices" and "You cannot get a No. Series Line with empty No. Series Code." in the prepayment codeunits |
| Purchases & Payables Setup, the same two fields | empty | a series | the same, for purchase prepayments |
| Purchases & Payables Setup, `Return Shipment on Credit Memo` | No | Yes (`PurchaseHeader."Return Shipment No. Series"` is only set then) | `PeekNextNo('')`: "You cannot get a No. Series Line with empty No. Series Code." in ERM Purchase Credit Memo and its neighbours |
| Sales & Receivables Setup, `Return Receipt on Credit Memo` | No | Yes | the same, for sales |
| Whse. Worksheet Template, Whse. Worksheet Name | no rows (0 pages) | a template of type Pick | "There is no Whse. Worksheet Template within the filter. Filters: Type: Pick": 55 failures in the full run of main 23cc1e8f (Service Order Warehouse Orange 33, SCM Pick Worksheet 14) |
| Warehouse Journal Template and Batch, Warehouse Employee, Bin Content | no rows (0 pages) | a template of type Item | "There is no Warehouse Journal Template within the filter. Filters: Type: Item": 9 failures (SCM Assembly Item Tracking, SCM Whse.-Asm. To Stock) |
| Item Journal Batch | one row (ITEM/DEFAULT) | a batch for the OUTPUT template | "There is no Item Journal Batch within the filter. Filters: Journal Template Name: OUTPUT" (SCM Order Tracking) |
| Service Cost, Service Zone | no rows (0 pages) | a cost with an account | "There is no Service Cost within the filter. Filters: Account No.: <>'', Service Zone Code: ''": 18 failures (Service - Price Management, Service Line Update Validation) |
| General Posting Setup | 14 rows; the product groups `FREIGHT` and `MISC` exist, the rows (`DOMESTIC`, `FREIGHT`) and (`DOMESTIC`, `MISC`) do not | those rows | "The General Posting Setup does not exist ... Gen. Prod. Posting Group='FREIGHT'" (6.0SP1 - VAT 1 to 1, 35) and `'MISC'` (ERM Invoice Discount And VAT, ERM VAT Serv. Charge, SCM Copy Document Mgt. and others) |
| VAT Posting Setup | 13 rows; (`DOMESTIC`, blank product group) is missing | that row | "The VAT Posting Setup does not exist ... VAT Prod. Posting Group=''" in ERM Prepayment III and Service Warehouse Integration (2 failures; not proven further) |
| Cost Type, Cost Journal Template, Cost Journal Batch, G/L Budget Name | no rows (0 pages) | at least one | "There are no records within the filters specified for table Cost Type", "The Cost Journal Template table is empty", "The G/L Budget Name table is empty": about 55 failures (ERM Cost Acc. Allocations, ERM CA Cost Journal, ERM KPI Web Service) |
| Value Entry | 828 rows, every one with `Cost Posted to G/L` equal to `Cost Amount (Actual)` (none unposted) | unposted entries when SCM Costing Batch's lazy `Initialize` runs `PostInvtCostToGL` (the MessageHandler expects "value entries have been posted to the general ledger") | "Assert.ExpectedMessage failed ... Actual: There is nothing to post to the general ledger" in the first test; `isInitialized` is never set, so every later test of the codeunit repeats `Initialize` and fails at "Unhandled UI: RequestPage 795" or "The following UI handlers were not executed: MessageHandler". 42 of the 68 failures in Tests-SCM-Costing (scm-costing-6 run). Not proven by a scratch app |
| General Ledger Setup, `Additional Reporting Currency` | `EUR` (LCY is GBP) | empty | SCM Inventory Costing II `PostPurchaseInvoiceWithChargeAssignment`: "Cost per Unit must be equal to '0' in Value Entry ... Current value is '1'" (`ItemJnlPostLine.RecalculateCostPerUnit` keeps the rounded cost per unit when the Source Currency Code differs from the ARC), and 10 tests after it in the codeunit. Found by instrumenting Item Jnl.-Post Line |
| Inventory Posting Setup (RESALE and the other groups), Interim and WIP accounts | blank | an account | "WIP Account is missing in Inventory Posting Setup" and "Inventory Account (Interim) is missing" in SCM Manufacturing and Costing (33 + 26 + 11 follow-ons in Tests-SCM-Manufacturing and Tests-SCM-Costing); the failing row is the one `FindLast` picks, so it depends on the order of the rows |

## Proof

The scratch app was one install codeunit that sets the fields above (`Posted Prepmt. Inv. Nos.` to `S-INV+`/`P-INV+`,
`Posted Prepmt. Cr. Memo Nos.` to `S-CR+`/`P-CR+`, both credit memo flags to true, `P-CR+` ending and warning number to
`109999` and `109995`; the last change is no longer needed, see below). Same binary, same tests:

| Codeunit | Without the scratch app | With it |
|---|---|---|
| Tests-Prepayment (all, 567 tests) | 86 passed, 102 failed, 379 not run | 414 passed, 17 failed, 136 not run |
| 138037 O365 Cancel Purch Cr Memo Dim. | 0 passed, 1 failed, 6 not run | 7 passed |
| 137028 Purch. Correct Cr. Memo | 1 passed, 1 failed, 27 not run | 24 passed, 1 failed, 4 not run |
| 134398 ERM Sales/Purch. Correct. Docs | 26 passed, 1 failed, 29 not run | 52 passed, 4 not run |

Most of the not run tests are the codeunit rule that a test after a failed one is not run when it draws random numbers,
so one setup value blocks a whole codeunit.

## What this means for the runtime

Nothing in the runtime is wrong here: the AL code of the Base Application runs as written. Making these tests run is a
choice about the database, not a fix: the runner would have to prepare the demo company the way Microsoft's test
database is prepared. That changes the data a test sees, so it is an owner decision (see docs/owner-review.md); the scratch app
is a one-file way to try it.

## The `P-CR+` range was a runtime defect, not data

No. Series Line `P-CR+` starts at 109001 and ends at 1010999. `NoIsWithinValidRange` takes Code[20] parameters and checks
`CurrentNo > EndingNo`. The runtime ordered two Code values as text, so `'109001' > '1010999'` was true and the series
read as used up ("no unused posted credit memo numbers").

The corpus pins Code apart from Text: text/lt-text-functions/TextCompare.Codeunit.al `CodeComparison` (lines 205-246)
shows `'10' < '9'` is No for Code (observation `tenlt`). The fix orders two Codes of plain digits (no leading zero) as
numbers in the `<`, `<=`, `>`, `>=` operators. Leading zeros, signs and mixed codes are not pinned and stay as before.
It is registered as the assumption `code-digits-compare-as-numbers`. Test: `crates/al-vm/tests/language/code_compare.rs`.
Results without any scratch app are in the hand-back; the prepayment and return-flag rows above remain data findings.

## Tables the backup holds no rows of (found with the empty-table text of FindFirst, FindLast and FindSet)

`al-bcdb read <bak> --table <T> --company CRONUS` returns no rows for these, and the tests below call `FindFirst` or `FindSet`
without creating a row first, so they fail with "The <table> table is empty." (DB:TableIsEmpty, pinned by
record/lt-filters-keys-finds/P68C_EmptyFinds.al). The backup holds 5 customers and no Service Item.

| Table | Tests that fail on it (Tests-Misc, Tests-SCM, Tests-ERM) |
|---|---|
| Service Item | Test Planning/Req.Wksh (PlanForServiceDemandNetChange, PlanAfterNeededByDateNetChange, MultipleDemandsSameDayNetChange) |
| Warehouse Entry | SCM Warehouse Unit Tests, 27 tests |
| Cost Journal Template | ERM CA Cost Journal, 16 tests |
| Cost Type, Responsibility Center | ERM Cost Accounting - Tables and Pages, ERM Sales Quotes (SalesQuoteCreation) |

Not confirmed by a scratch app that seeds the rows; the other rows above are.

Cost Accounting - Pages, date filter: TestCostBudgetPerCostCenterMatrixValidateFiltersForAmountTypeNetChange and 3 siblings read
`MatrixForm.FILTER.GetFilter("Date Filter")`. The matrix part sets that filter on its Rec in `SetRecordFilters`, which only runs for a row
(OnAfterGetRecord). The backup holds no Cost Type rows, so no row is read and the filter stays empty. Not confirmed with seeded rows.
## Group 1 of ms-fails-2 was runtime, and mostly fixed by earlier merges

"Location Code GU... cannot be found in Location" (95), "Bin Code ... Bin" (about 190), "Item Tracking Code SN-PRON" (24) and
"Warehouse handling is required" (about 107) were values the test had just created. They are gone from the failures after
the merges that followed the full run of main 0b414672 (3273 failures then, 924 on main 23cc1e8f). The
remaining part is a runtime defect fixed on branch ms-fails-2: a `where()` condition of a TableRelation that names the
validated field itself (`Item Variant.Code where("Item No." = field("Item No."), Code = field("Variant Code"), ...)` of Prod.
Order Line, Assembly Header and Assembly Line) read the old, blank value of the field, so it contradicted the key and the
variant "cannot be found". It reads the value being validated now (assumption `relation-where-reads-the-validated-value`,
test `crates/al-vm/tests/store/relation_where_enum_filter.rs`).

How the data rows above were proven: `bcdb tables BusinessCentral-W1.bak` lists the page count of every table of the
company; the tables above have 0 pages, and a scratch test that counts the rows through the runtime agrees.
