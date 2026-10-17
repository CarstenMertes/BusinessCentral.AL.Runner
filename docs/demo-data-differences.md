# Where the demo backup differs from the database Microsoft's tests run on

The test data of a run is the Cronus backup of the BC artifact (ADR-020). Microsoft's tests were written and gated on a
test database that is set up further. Where a test needs a setup value the backup lacks, the run fails or is refused for
a reason that is not a runtime defect. This file lists the differences that are proven, with the evidence, so that a
failure is not chased in the runtime twice. Each row was found by reading the backup (BC 27.5.46862.48827,
`BusinessCentral-W1.bak`) and confirmed by an experiment: the same tests, run with the value set by an install codeunit
of a scratch app (`alrunner test <test app> <scratch app>`), pass.

| Table, field | In the backup | A test needs | Effect (main 0b414672, full run) |
|---|---|---|---|
| Sales & Receivables Setup, `Posted Prepmt. Inv. Nos.`, `Posted Prepmt. Cr. Memo Nos.` | empty | a series | "Specify the code for the number series that will be used to assign numbers to posted sales prepayment invoices" and "You cannot get a No. Series Line with empty No. Series Code." in the prepayment codeunits |
| Purchases & Payables Setup, the same two fields | empty | a series | the same, for purchase prepayments |
| Purchases & Payables Setup, `Return Shipment on Credit Memo` | No | Yes (`PurchaseHeader."Return Shipment No. Series"` is only set then) | `PeekNextNo('')`: "You cannot get a No. Series Line with empty No. Series Code." in ERM Purchase Credit Memo and its neighbours |
| Sales & Receivables Setup, `Return Receipt on Credit Memo` | No | Yes | the same, for sales |

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
