//! The registry of behaviors the runtime implements on an unprobed assumption.
//!
//! Owner rule (CLAUDE.md hard rule 3, exception of 2026-10-15): behavior a Microsoft test needs may be
//! implemented before its corpus probe exists, but every test that passes through such behavior is a
//! FLAGGED pass: it relies on something no real-BC measurement has confirmed. Each such code path calls
//! [`assume`] with its entry of [`Assumption`] when it runs; the runner collects the entries a test hit and
//! reports the pass as flagged (console summary, JUnit property, JSON, `--explain`, `--top-assumptions`).
//!
//! One entry per assumption: its stable id, the behavior assumed, the Microsoft test(s) that need it and the
//! probe wanted. `docs/probes-needed.md` holds a table generated from this registry (a test checks it; run
//! the test with `UPDATE_ASSUMPTIONS_DOC=1` to rewrite it). When a probe pins an assumption, delete its entry:
//! the compiler then names every place that still marks it.
//!
//! A test (`every_entry_is_marked_somewhere`) checks that each entry is used by at least one code path; an
//! unregistered marker does not compile.

use std::cell::Cell;

macro_rules! registry {
    ($( $(#[$m:meta])* $variant:ident => {
        id: $id:expr,
        assumed: $assumed:expr,
        tests: $tests:expr,
        probe: $probe:expr,
        $(note: $note:expr,)?
    } ),* $(,)?) => {
        /// A behavior implemented on an assumption no probe has confirmed.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        #[repr(u8)]
        pub enum Assumption { $( $(#[$m])* $variant ),* }

        impl Assumption {
            /// Every entry, in registry order.
            pub const ALL: &'static [Assumption] = &[ $( Assumption::$variant ),* ];

            /// The entry's text.
            pub fn info(self) -> Info {
                match self {
                    $( Assumption::$variant => Info {
                        id: $id,
                        assumed: $assumed,
                        tests: $tests,
                        probe: $probe,
                        note: registry!(@note $($note)?),
                    } ),*
                }
            }
        }
    };
    (@note) => { None };
    (@note $n:expr) => { Some($n) };
}

/// What the registry says about one assumption.
#[derive(Debug, Clone, Copy)]
pub struct Info {
    /// The stable id: lower case words with hyphens.
    pub id: &'static str,
    /// The behavior assumed, in one line.
    pub assumed: &'static str,
    /// The Microsoft test(s) that need it.
    pub tests: &'static str,
    /// The probe wanted.
    pub probe: &'static str,
    /// An open point about the entry (a probe that contradicts it, say).
    pub note: Option<&'static str>,
}

registry! {
    TransferFieldsEnumOption => {
        id: "transfer-fields-enum-option",
        assumed: "TransferFields moves the ordinal between an Enum field and an Option field of the same field number; an Option ordinal the Enum lacks is refused.",
        tests: "Sales Order Entity Buffer next to Sales Header (Shipping Advice); 149 Tests-SCM tests.",
        probe: "Dst.TransferFields(Src) for an Enum source and an Option target and back, with an ordinal the Enum lacks.",
    },
    FieldOfProcedureResult => {
        id: "field-of-procedure-result",
        assumed: "A field read off what a procedure returns (GetLocation(Code).\"Bin Mandatory\") is the field of the returned copy.",
        tests: "Warehouse Receipt Header.BinCodeMandatory; 50 Tests-SCM tests.",
        probe: "A procedure p(): Record T and p().\"Field\" as an rvalue.",
    },
    RecordGetRecordId => {
        id: "record-get-record-id",
        assumed: "Record.Get(RecordId) of a row that does not exist fails like Get(key) with the same error text; a RecordId of another table is refused. (A row that exists is pinned by record/TestSetAutoCalcFieldsOnGet.Codeunit.al.)",
        tests: "Item Category Management (ItemCategory.Get(RecId)); 29 Tests-SCM tests.",
        probe: "Get(RecordId) on a missing row (result used and unused) and with a RecordId of another table.",
    },
    RecordSetFilterEmpty => {
        id: "record-setfilter-empty",
        assumed: "Record.SetFilter(Field, '') removes the field's filter in the current filter group, as a Query column does.",
        tests: "BOM Structure (Item.SetFilter(\"No.\", ItemFilter) with an empty filter); 49 Tests-SCM tests.",
        probe: "SetFilter(F, '') after SetRange(F, 1) and after SetFilter(F, '>1'), then GetFilters, in filter group 0 and 2.",
    },
    FindEqualOnAnotherKey => {
        id: "find-equal-on-another-key",
        assumed: "Find with `=` among several directions (`=><`) on a record whose current key is not the primary key finds the row itself when the record's primary key names a row that passes the filters and the fields of the current key still hold the row's values; only then is the neighbour searched.",
        tests: "Req. Wksh.-Make Order (Code: ReqLine.Find('=><') on the Requisition Line the Carry Out Action Msg. report read by its own sorting); Tests-SCM UnitCostOnPurchLineWithCarryOutActionMsg and the tests that carry out action messages.",
        probe: "SetCurrentKey(SecondaryKey), set the key fields and the primary key to an existing row, Find('=>'), Find('=<') and Find('=><'); then change one key field and repeat.",
    },
    ClearAllKeepsTemporaryRows => {
        id: "clear-all-keeps-temporary-rows",
        assumed: "ClearAll() resets the fields and filters of a global temporary record but keeps its rows, as Clear(Rec) does.",
        tests: "Purch.-Post ClearAllVariables (RunWithCheck relies on TempWhseRcptHeader, filled by SetWhseRcptHeader before); Tests-SCM tests that post a warehouse receipt of a purchase order.",
        probe: "A global temporary record with a row and a filter, ClearAll(), then Count and GetFilters; the same in a codeunit run by Codeunit.Run.",
    },
    PageOnInsertRecordFalseAfterCodeInsert => {
        id: "page-oninsertrecord-false-after-code-insert",
        assumed: "When a page's OnInsertRecord inserts the row itself and answers false, the page goes on with that row as a stored one: the next edit of the row runs OnModifyRecord instead of OnInsertRecord again.",
        tests: "Item Tracking Lines (InsertRecord inserts into the page's temporary source, ValidateAndInsert answers false); Tests-Manufacturing PostProductionJournalUsingLotItemTracking and the item tracking tests that set a lot and a quantity.",
        probe: "A page with OnInsertRecord { Rec.Insert(); exit(false) } and OnModifyRecord { Rec.Modify(); exit(false) }: OpenNew, set the key, set a second field, OK; read the row and count the trigger calls.",
    },
    DateAddWholeDecimal => {
        id: "date-add-whole-decimal",
        assumed: "Date + Decimal and Date - Decimal with a whole number add or subtract days like an Integer; a fraction is refused.",
        tests: "Phys. Invt. Count.-Management.CalcPeriod (StartDate + Days with Days: Decimal); 15 Tests-SCM tests.",
        probe: "D + 2.5, D - 2.5.",
    },
    NextWholeDecimal => {
        id: "next-whole-decimal",
        assumed: "Record.Next(Decimal) with a whole number steps like the Integer; a fraction is refused.",
        tests: "Phys. Invt. Count.-Management.CalcPeriod (Calendar.Next(12 / CountFrequency)).",
        probe: "R.Next(1.5) and R.Next(-1.5).",
    },
    OptionAsIntegerArgument => {
        id: "option-as-integer-argument",
        assumed: "A text builtin that takes an Integer accepts an Option and uses its ordinal, as an assignment to an Integer does.",
        tests: "Calculate Standard Cost.PrepareAssemblyCalculation (SelectStr(Target, Text)); 14 Tests-SCM tests.",
        probe: "SelectStr, CopyStr, PadStr and StrPos with an Option value.",
    },
    TableRelationFilterCondition => {
        id: "table-relation-filter-condition",
        assumed: "A TableRelation arm with a Field = filter(x) condition applies when the field's value passes the filter text (filter('') the blank value, filter(<> '') any other); the first arm that holds wins.",
        tests: "Bin Content / Warehouse Journal Line \"Bin Code\", Item \"Put-away Unit of Measure Code\"; 307 Tests-SCM tests.",
        probe: "TableRelation = if (Zone = filter('')) Bin.Code where(...) else if (Zone = filter(<> '')) Bin.Code where(...): Validate of a bin of each zone and FieldRef.Relation.",
    },
    TableRelationQuotes => {
        id: "table-relation-quotes",
        assumed: "A filter(...) text of a TableRelation where() clause quotes an identifier with \" and is read like a SetFilter text after the quotes are rewritten to '.",
        tests: "Item where(Type = filter(Inventory | \"Non-Inventory\")) (Sales Line No.); tests in Tests-ERM.",
        probe: "TableRelation ... where(F = filter(A | \"B C\")) and Validate with a value of each kind.",
    },
    StaleBufferNewerThanRow => {
        id: "stale-buffer-newer-than-row",
        assumed: "A buffer whose row version is newer than the stored row (the transaction was rolled back to the last Commit after the buffer's own Modify) is saved without DB:RecordChanged; only a stored row newer than the buffer is stale.",
        tests: "SMB Service Item TestItem_Reserve (and four siblings).",
        probe: "Modify, Commit, Modify, asserterror, Modify through the same buffer.",
    },
    TextDigitsToInteger => {
        id: "text-digits-to-integer",
        assumed: "A Text or Code of digits (optional sign, surrounding spaces ignored) converts into an Integer or BigInteger where a Variant meets a number, such as the key of Get; a text with anything else stays refused.",
        tests: "Incoming Document.FindFromIncomingDocumentEntryNo (`Get(Format(FieldRef.Value))`): about 400 Tests-ERM tests behind the Incoming Doc. Attach. FactBox.",
        probe: "Rec.Get(Format(5)) on an Integer key; Get('5'); Get('x'); Get('') and Get(' 5 ') on an Integer and a BigInteger key.",
    },
    CustomLayoutMerger => {
        id: "custom-layout-merger",
        assumed: "A SaveAs of a report whose selected rendering layout has Type = Custom raises the platform's CustomDocumentMergerEx with ObjectID, a ReportAction named after the format (SaveAsPdf, SaveAsWord, SaveAsExcel, SaveAsHtml), an EMPTY ObjectPayload, XmlData holding the Xml dataset SaveAs writes, LayoutData holding the layout file (read as UTF-8) and DocumentStream on the stream SaveAs was given (UTF-8); a subscriber that sets Success/IsHandled makes SaveAs answer true. A SaveAs no subscriber handles is refused.",
        tests: "handlers/TestReportLayoutByNameMergerCapture.al pins the subscriber's signature and the layout text; Pageworks (about 50 tests whose first refusal is SaveAs as Pdf).",
        probe: "A custom layout report saved as Pdf, Word, Excel and Html with a Base Application subscriber that records every argument: the ReportAction ordinals, the keys of ObjectPayload (layoutmimetype, layoutname, ...), the form of XmlData, the encoding of the streams, and SaveAs with no subscriber.",
    },
    CookieSetterResult => {
        id: "cookie-setter-result",
        assumed: "Cookie.Name(NewName) and Cookie.Value(NewValue), with the result used, answer the new text.",
        tests: "platform/lt-xmlport-session-navapp-cookie/P94Cookie.al assigns the result to a Text it never reads (SetName_DoesNotTouchOtherMembers, Copy_AndClear).",
        probe: "Format of the result of C.Name('n') and C.Value('v') on a cookie that had other values before.",
    },
    PrimitiveToTextIsFormat => {
        id: "primitive-totext-is-format",
        assumed: "Date.ToText() and Boolean.ToText() give Format(value) with the default format (en-US).",
        tests: "Customer Mgt.CalcLastPaymentInfo via the Customer Statistics FactBox: 49 Tests-ERM tests.",
        probe: "D.ToText() against Format(D) for a Date and a blank Date; B.ToText() for true and false.",
    },
    FormatTextCodeWidthPads => {
        id: "format-text-code-width-pads",
        assumed: "In a Format picture, `<Month Text,N>` and `<Weekday Text,N>` give the name cut to N characters (pinned for N = 3) and, when it is shorter than N, padded with trailing spaces to N.",
        tests: "Accounting Period names made with `<Month Text,10>` (Tests-ERM DefaultAccountingPeriodUT and 37 more).",
        probe: "Format(DMY2Date(5,3,2024), 0, '<Month Text,10>') with its length and a visible delimiter; the same for <Weekday Text,10>.",
    },
    PageExpressionControlSetValue => {
        id: "page-expression-control-setvalue",
        assumed: "TestPage.SetValue on a field control whose source is an assignable path (a record in an array, `Totals[1].\"Inv. Discount Amount\"`) assigns the path, then runs the control's OnValidate; the value is converted to the type of the value the path holds now. An entry the client would refuse is refused with the variable-control texts.",
        tests: "Purchase and Sales Order Statistics (`InvDiscountAmount_General`): ConfirmationOnInvDiscUpdateInPurchOrderWithInvoicedLine and about 55 more Tests-ERM tests.",
        probe: "A page field(Ctl; Arr[1].Amount) with an OnValidate; TestPage.Ctl.SetValue(5) then the global and the OnValidate effect; SetValue('x') on it.",
    },
    DateToTextIsFormat => {
        id: "date-to-text-is-format",
        assumed: "A Date converted to a Text or Code (a Variant from FieldRef.Value into a Text parameter) is its default Format text, so an Evaluate of it reads the same date back.",
        tests: "Incoming Document.FindByDocumentNoAndPostingDate(.., PostingDateFieldRef.Value) behind the Incoming Doc. Attach. FactBox: about 470 Tests-ERM tests.",
        probe: "T := V where V is a Variant holding a Date: compare T with Format(D); Evaluate(D2, T); the same for a blank Date.",
    },
    GuidToText => {
        id: "guid-to-text",
        assumed: "A Guid assigned to a Text or Code is its text form {XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX} in capitals, as Format gives it.",
        tests: "Config. Package Management.SendTelemetryOnDeleteConfigPackage (Dimensions.Add('PackageCode', Rec.SystemId)); 15 Rapid Start tests.",
        probe: "T := G with Text[40], Text[10], Code[40]; Dict.Add('k', G); Evaluate(T, G).",
    },
    GuidAsTextArgument => {
        id: "guid-as-text-argument",
        assumed: "A Guid passed where a function wants a Text is its text form {XXXXXXXX-...}, as in an assignment.",
        tests: "Match General Jnl Lines UT (CopyStr(CreateGuid(), 1, 50)); 61 Tests-General Journal tests.",
        probe: "CopyStr(G, 1, 50), StrLen(G), UpperCase(G).",
    },
    ProcedureAssignIsCall => {
        id: "procedure-assign-is-call",
        assumed: "`Proc := value` for a procedure with one value parameter is the call `Proc(value)`.",
        tests: "Service Line (`SetHideWarrantyWarning := true` in OnValidate of Fault Reason Code): T204_PostedServiceInvoiceContainsPriceCalcMethod and 178 more Tests-ERM tests.",
        probe: "A codeunit procedure P(V: Boolean) and `P := true;`: does it compile and run P? The same with a var parameter, two parameters, a return value.",
    },
    DateFormulaSpacesBetweenTerms => {
        id: "date-formula-spaces-between-terms",
        assumed: "Spaces in a date formula are ignored, also between the terms: `<CM + 1D>` is `<CM+1D>` (the corpus pins spaces next to the brackets only).",
        tests: "Tests-SCM Item.TestField(\"Next Counting Start Date\", CalcDate('<CM + 1D>', WorkDate())), Base Application's `<CM + 1D>` and 16 more Tests-ERM tests.",
        probe: "CalcDate('<CM + 1D>', D), Evaluate(DF, '<1D + 1W>') and Format(DF), '<1 D>', '<- 1D>'.",
    },
    FeatureKeyEnabledModify => {
        id: "feature-key-enabled-modify",
        assumed: "Modify of a Feature Key with a changed Enabled (the only writable column) is kept for the rest of the run: later reads answer the new value, a rollback undoes it with the other writes.",
        tests: "Tests-ERM Initialize (`FeatureKey.Enabled := FeatureKey.Enabled::None; FeatureKey.Modify()` for ReminderTermsCommunicationTexts): 243 tests.",
        probe: "Get a Feature Key, change Enabled, Modify; Get again and read Enabled; FindSet and Count; the same inside a test with TestIsolation, then in the next test.",
    },
    FeatureKeyUnchangedModify => {
        id: "feature-key-unchanged-modify",
        assumed: "Modify on a Feature Key row that was read and not changed (Enabled set to the value it has) succeeds and changes nothing.",
        tests: "Tests-ERM Initialize (`FeatureKey.Enabled := FeatureKey.Enabled::None; FeatureKey.Modify()` for ReminderTermsCommunicationTexts): 243 tests.",
        probe: "FindFirst, Modify() with nothing changed; Enabled := Enabled then Modify; Enabled changed then Modify, then Get again and read Enabled; after a rollback.",
    },
    TypedModalHandlerSkipsPageTriggers => {
        id: "typed-modal-handler-skips-page-triggers",
        assumed: "A [ModalPageHandler] typed as the Page itself stands in for the page: none of the page's triggers (OnInit, OnOpenPage, OnQueryClosePage, ...) run for that call.",
        tests: "Tests-ERM `PaymentToleranceWarning` handler (`PaymentToleranceWarning.InitializeOption(OptionValue)`, page 591, whose OnOpenPage and OnQueryClosePage would overwrite the result) and about 47 more tests.",
        probe: "A page with Trace logging in OnInit, OnOpenPage, OnQueryClosePage and OnClosePage, run with RunModal under a ModalPageHandler typed as the Page that logs and sets Response.",
    },
    QueryMethodOverFlowField => {
        id: "query-method-over-flowfield",
        assumed: "A query column over a FlowField with Method Sum, Average, Min or Max aggregates the value calculated for each joined row (with the flow filters of its dataitem), as a column without a method shows it.",
        tests: "Query \"Cust. Ledg. Entry Remain. Amt.\" (Sum of the FlowField \"Remaining Amt. (LCY)\"): T120_NotifyAboutMissingSalesSetupOnLineDiscountValidation and about 425 more Tests-ERM tests.",
        probe: "A query with column(Total; FlowFieldOfTheTable) { Method = Sum; } over three rows: the total; the same with Min, Max, Average, ReverseSign and a filter() element feeding the flow filter.",
    },
    RecordRefGetTableOfRecordRef => {
        id: "recordref-gettable-of-recordref",
        assumed: "RecordRef.GetTable(OtherRecordRef) makes the ref a copy of the other ref's record: table, buffer, filters and current row, not shared afterwards.",
        tests: "Tests-ERM SalesInvoiceTryPreview and the other posting preview tests (Gen. Jnl.-Post Preview): about 30 tests.",
        probe: "B.GetTable(A) with A open on a table with a filter and a found row; change A, read B; with A not open; with B open on another table.",
    },
    RequestPageCaptionSet => {
        id: "request-page-caption-set",
        assumed: "`RequestOptionsPage.Caption := Text` in the code of a report changes nothing a test can see: a request page handler's `TestRequestPage.Caption` is not changed by it.",
        tests: "Financial Report (`RequestOptionsPage.Caption := FinancialReport.Description`): about 86 Tests-ERM tests.",
        probe: "A report whose request page OnOpenPage sets RequestOptionsPage.Caption; a RequestPageHandler reads TestRequestPage.Caption.",
    },
    FilterEmptyTextDateFormula => {
        id: "filter-empty-text-dateformula",
        assumed: "In a filter on a DateFormula field the empty text is the empty formula, so a filter of not-equal and the empty text selects the rows that have a formula.",
        tests: "Library - ERM GetDiscountPaymentTerm (PaymentTerms.SetFilter(Due Date Calculation, not-equal and the empty text)): about 170 Tests-ERM tests.",
        probe: "Payment Terms rows with and without Due Date Calculation; SetFilter on it with the empty text, not-equal and the empty text, then Count.",
    },
    UpdateRefreshOnFindRecord => {
        id: "update-refresh-onfindrecord",
        assumed: "CurrPage.Update on a page with OnFindRecord and OnAfterGetCurrRecord asks OnFindRecord for the current row with Which `=`, then reads the row as after a move: OnAfterGetRecord, OnAfterGetCurrRecord and the parts' reads.",
        tests: "Discount Setup Notifications (T130_NotifyAboutMissingSalesSetupOnInvDiscountValidation, a Sales document page) and about 87 more Tests-ERM tests.",
        probe: "A card with OnFindRecord and OnAfterGetCurrRecord that log, an action that calls CurrPage.Update: the Which string and the trigger order; the same on a list, and after an edit not yet saved.",
    },
    PageRecProcedureSeesPageXrec => {
        id: "page-rec-procedure-sees-page-xrec",
        assumed: "A table procedure the page's code calls on Rec (Rec.InitType()) sees the page's xRec as its xRec, not a blank record.",
        tests: "Test Item Lookup (TestSalesInvoiceItemLookupInDescription and four siblings): Sales Invoice Subform.OnNewRecord calls Rec.InitType, which copies xRec.Type.",
        probe: "A table with a procedure returning xRec.\"No.\"; a page whose OnAfterGetCurrRecord returns it through Rec.Proc() after moving between two rows; the same in OnNewRecord.",
    },
    UpdateRefreshListOnFindRecord => {
        id: "update-refresh-list-onfindrecord",
        assumed: "CurrPage.Update on a list with OnFindRecord (with or without OnNextRecord) reads the rows as the pinned table of 3 rows with the cursor on the middle one does (misc/W2FindRecordUpdate.al), for any number of rows and any cursor position below a window.",
        tests: "ERM Sales Subform (InvoiceAddingLinesUpdatesTotals) and about 120 more Tests-ERM tests whose sales and purchase subforms refresh.",
        probe: "The same list with 1, 2, 5 and 25 rows and the cursor on the first, last and a middle row; a list with OnFindRecord only; a list part of a card.",
    },
    CaptionReadBeforeThePageRuns => {
        id: "caption-read-before-the-page-runs",
        assumed: "CurrPage.Caption read in a procedure the caller invokes on a Page variable before Run or RunModal answers the page's Caption property, and raises nothing.",
        tests: "Alloc. Account Sales E2E Tests (TestOverrideGLAllocation) and 20 more Tests-ERM tests that open Edit Dimension Set Entries through Dimension Management.",
        probe: "A page with Caption = 'X' and a procedure returning CurrPage.Caption, called on a Page variable before RunModal and after it closed; also with a Caption assigned in OnOpenPage.",
    },
    PartOfAPageThatNeverRan => {
        id: "part-of-a-page-that-never-ran",
        assumed: "CurrPage.<Part>.Page in a procedure called on a Page variable that was never run reaches a part page that exists, is initialised and is not open (as a hidden part does).",
        tests: "O365 Simplify UI Sales Invoice (CreateInvoiceForCustomerWithCreditLimit) and the other Tests-SMB and Tests-ERM tests that send the credit limit notification.",
        probe: "A page with a part and a procedure that calls a procedure of the part page and returns what it set; call it on a Page variable before Run and read the result.",
    },
    SetTableViewKeepsSetRecordPosition => {
        id: "set-table-view-keeps-set-record-position",
        assumed: "On a Page variable, SetTableView(Rec) after SetRecord(Rec) takes the filters of the view and keeps the record SetRecord handed over as the position the page opens on, so OnOpenPage reads it as Rec.",
        tests: "VendEntry-Apply Posted Entries / CustEntry-Apply Posted Entries (SetRecord then SetTableView, then Apply Vendor Entries OnOpenPage reads Rec.\"Vendor No.\"): about 39 Tests-ERM tests.",
        probe: "A page variable with SetRecord(A) then SetTableView(B) where B has a filter and another position: the Rec OnOpenPage reads, the rows shown, and the same in the other order.",
    },
    FormatCurrencyPrefix => {
        id: "format-currency-prefix",
        assumed: "In a Format string the code `<C,CODE>` (the currency prefix of Base Application's Amount Auto Format) is accepted and writes nothing.",
        tests: "Format(Amount, 0, AutoFormat.ResolveAutoFormat(AmountFormat, 'GBP')) in the Remittance Advice and Finance Charge Memo reports: about 28 Tests-ERM tests (FinanceChargeMemoAndApply).",
        probe: "Format(123.45, 0, '<C,GBP><Precision,2:2><Standard Format,0>') and with the code after the others; Format(5, 0, '<C,GBP><Integer>'); '<C,>' and '<C>'.",
    },
    PageRowDeleteEventNotRaised => {
        id: "page-row-delete-event-not-raised",
        assumed: "A page whose table has a subscriber to OnDeleteRecordEvent opens; the test client has no operation that deletes the page's row, so the event is never raised.",
        tests: "Base Application subscribes on the General Journal and other journals; 88 Tests-General Journal tests.",
        probe: "Invoke the standard Delete action on a page row of a table with such a subscriber: is the subscriber run.",
    },
    NewRowEqualFiltersSeveralGroups => {
        id: "new-row-equal-filters-several-groups",
        assumed: "A new page row takes the value of a field that has equality filters in several filter groups when they all name the same value; different values are not stamped.",
        tests: "Payment Journal and Incom. Doc. Attach. FactBox tests of Tests-General Journal (Cash Receipt Journal).",
        probe: "Open a page on a record with SetRange(F, 'A') in group 0 and group 2, insert a row, read F.",
    },
    GetRangeAcrossFilterGroups => {
        id: "get-range-across-filter-groups",
        assumed: "GetRangeMin/Max see a filter on the field set in another filter group when the current group has none. With no filter at all it stays refused.",
        tests: "GenJnlManagement.OpenJnl after TemplateSelection (Purchase Journal OnOpenPage); 34 Tests-General Journal tests.",
        probe: "FilterGroup(2); SetRange(F,'T'); FilterGroup(0); GetRangeMax(F).",
    },
    InvisiblePartForPageCode => {
        id: "invisible-part-for-page-code",
        assumed: "CurrPage.<Part>.Page in the page's own code reaches a part whose Visible came out false, whose Visible is the literal false or whose ApplicationArea is not enabled (its page is created on the first reach, initialised and not opened).",
        tests: "Cash Receipt Journal OnAfterGetCurrRecord (WorkflowStatusBatch, Visible = variable); 7 Tests-General Journal tests; Sales Credit Memo and other document pages (IncomingDocAttachFactBox), about 230 Tests-ERM tests.",
        probe: "A page with part(P; ...) { Visible = Flag } where Flag is false at open and OnAfterGetCurrRecord calls CurrPage.P.Page.Method().",
    },
    DatabaseSidNumbers => {
        id: "database-sid-numbers",
        assumed: "Database.SID answers S-1-5-21-a-b-c-rid with a fixed domain part and a stable hash of the name as the rid.",
        tests: "Library - Document Approvals.GetUser; 214 Tests-Dimension tests.",
        probe: "Nothing in AL can compare the numbers with BC's; the shape is pinned (PXDatabaseSid.al).",
    },
    MixedDataItemLink => {
        id: "mixed-dataitem-link",
        assumed: "A query DataItemLink whose parts name different enclosing data items joins each part to its item.",
        tests: "Item Analysis View Source / Analysis View Source queries; hundreds of Tests-SCM tests.",
        probe: "A three-level query where the leaf links to the grandparent and the parent in one DataItemLink.",
    },
    SaveValuesWithExtensions => {
        id: "savevalues-with-extensions",
        assumed: "A page extension that adds only controls bound to Rec fields, labels, parts or actions changes nothing about what a SaveValues page stores.",
        tests: "Pages with SaveValues and page extensions in the buckets.",
        probe: "A SaveValues page extended by a control bound to a variable and by one bound to a field.",
    },
    SaveValuesCallerValueWins => {
        id: "savevalues-caller-value-wins",
        assumed: "On a report's request page with SaveValues, every variable keeps its value, over the one an earlier confirmed run stored, when the caller ran a procedure of the report variable (SetParameters) before the run.",
        tests: "Microsoft's Get Demand To Reserve with its Assembly extension, run by Reservation Worksheet tests.",
        probe: "A report with SaveValues whose first run confirms the default and whose second run sets the variable before RunModal; read the variable in OnPreReport.",
    },
    TempRecordArrayShares => {
        id: "temporary-record-array-shares-table",
        assumed: "The elements of an array of temporary records of one table share one temporary table (each element has its own filters and position).",
        tests: "Microsoft's Tests-SCM-Planning (Inventory Profile Offsetting keeps demand and supply in InventoryProfile: array[2] of Record \"Inventory Profile\" temporary).",
        probe: "An array[2] of a temporary record: insert through element 1, count and read through element 2.",
    },
    AbsOfDuration => {
        id: "abs-of-duration-is-decimal",
        assumed: "Abs of a Duration takes the Decimal overload: the Duration counts as its milliseconds and the answer is a Decimal.",
        tests: "Microsoft's production order and routing code (Abs of a DateTime difference): Tests-SCM-Manufacturing and Tests-SCM-Costing, about 230 tests.",
        probe: "D: Duration := -5000; X := Abs(D) into a Decimal and into a Duration; Format both; Abs(D) > 4999.",
    },
    DateToDateTime => {
        id: "date-to-datetime-is-midnight",
        assumed: "A Date assigned or converted to a DateTime is that day at midnight; the blank Date is the blank DateTime.",
        tests: "Microsoft's production order and routing code (Starting Date-Time := Starting Date): Tests-SCM-Manufacturing, about 20 tests.",
        probe: "DT := D for a defined and for a blank Date; Format(DT, 0, 9); DT := ClosingDate(D).",
    },
    DurationArithmeticWithNumber => {
        id: "duration-arithmetic-with-number",
        assumed: "A whole number added to or subtracted from a Duration (or a Duration from a whole number) is a Duration, as milliseconds.",
        tests: "Microsoft's routing code (a day in milliseconds added to a Duration): Tests-SCM-Manufacturing, about 20 tests.",
        probe: "D: Duration; I: Integer; Format(D + I), Format(I + D), Format(D - I), Format(I - D).",
    },
    SetViewSortingWithoutKey => {
        id: "set-view-sorting-without-a-key",
        assumed: "SetView of a SORTING(...) that no key starts with orders the rows by those fields, then by the primary key, as SetCurrentKey of such fields does.",
        tests: "Microsoft's tests that set a view written by GetView after an ad hoc SetCurrentKey (Posting Preview and Item Journal tests): Tests-SCM and Tests-SCM-Manufacturing, about 110 tests.",
        probe: "SetCurrentKey on fields no key starts with, GetView, a second record SetView(that view): compare the order and CurrentKey().",
    },
    DeleteAllSkipsRowsDeletedMeanwhile => {
        id: "delete-all-skips-rows-deleted-meanwhile",
        assumed: "DeleteAll(true) with triggers goes row by row and does not visit a row that an earlier row's OnDelete (or an event) deleted.",
        tests: "Requisition Line OnDelete (deletes the lines of a higher level that follow) under Order Planning Mgt. PrepareRequisitionRecord: Tests-SCM-Manufacturing B42538_UserIDOnReqLine and Order Planning tests.",
        probe: "A table whose OnDelete deletes the next row; DeleteAll(true) over three rows; count the rows and the OnDelete calls.",
    },
    RelationWhereOnTargetFlowFilter => {
        id: "relation-where-on-a-flowfilter-of-the-target",
        assumed: "A where() condition of a TableRelation on a FlowFilter field of the related table does not select rows when the relation is validated.",
        tests: "Item Journal Line \"Bin Code\" (Bin.Code where(\"Item Filter\" = field(\"Item No.\"), \"Variant Filter\" = ...)): Tests-SCM and Tests-SCM-Manufacturing, about 10 tests.",
        probe: "A table relation whose where() names a FlowFilter of the target; Validate a value that exists; Validate one that does not.",
    },
    QueryClosePageFallsThroughAsTrue => {
        id: "query-close-page-falls-through-as-true",
        assumed: "An OnQueryClosePage trigger with a Boolean return that reaches its end without giving a value lets the page close (its named return value starts as Yes).",
        tests: "Item Tracking Lines OnQueryClosePage (its Result stays untouched when the availability check has no warning): about 1,100 tests of Tests-SCM, Tests-SCM-Manufacturing and Tests-SCM-Costing that close the page from a handler.",
        probe: "A page with trigger OnQueryClosePage(CloseAction: Action) Result: Boolean that does not assign Result on one path; close it with OK from a handler; read the answer of RunModal.",
    },
    ImplicitCategoryGroup => {
        id: "implicit-category-group",
        assumed: "addlast/addfirst on the implicit group of a legacy promoted category (Category_Category4) adds into the promoted area of the page.",
        tests: "Email Address Lookup extension; 54 Tests-Misc tests.",
        probe: "An extension adding to Category_Category4 of a page whose action says Promoted = true; PromotedCategory = Category4.",
    },
    ChainedArrayIndex => {
        id: "chained-array-index",
        assumed: "A[x][y] on a multi-dimensional array is A[x, y].",
        tests: "Backup Storage test library (array[20, 1000] of RecordRef); 43 Tests-Misc tests.",
        probe: "The chained form on a two-dimensional array, read and write.",
    },
    InBindsTighterThanAnd => {
        id: "in-binds-tighter-than-and",
        assumed: "`a and b in [..]` (also `or`, `xor`) without parentheses is `a and (b in [..])`, not `(a and b) in [..]`.",
        tests: "Base Application's Purchase Header, field Buy-from Vendor No. (Self-Billing Invoice); every Tests-SCM and Tests-ERM test that validates it.",
        probe: "A Boolean variable and an enum value joined with and, then in a set, with and without parentheses; a Boolean result for each.",
    },
    FilterTextReadAsEvaluate => {
        id: "filter-text-read-as-evaluate",
        assumed: "A text value of a filter on a Date or Guid field (SetFilter, SetRange with text) reads as Evaluate reads the text, in the runtime's fixed region (en-US short date `01/17/27`, a Guid with braces). A formula such as 1M, a Time or DateTime text and an empty text stay refused.",
        tests: "about 700 tests of the Tests-SCM buckets filter dates with text built by Format.",
        probe: "SetFilter on a Date field with Format(D), with '..' + Format(D), and with '' and '1M'; the same on a Guid field with Format(G); then GetFilter and Count.",
    },
    ReportThisPassedOn => {
        id: "report-this-passed-on",
        assumed: "`this` in a report can be passed on (to a var parameter of the report's type, in an event for example) and is the running report: its procedures, and those of the report extensions, work on it.",
        tests: "Whse.-Source - Create Document raises OnAfterPostReport(..., this); about 960 tests of the Tests-SCM buckets.",
        probe: "A report that passes this to an event in OnPostReport; a subscriber calls a procedure of the report and one of a reportextension on the parameter.",
    },
    PageRunRecordAndSourceTableView => {
        id: "page-run-record-and-source-table-view",
        assumed: "Page.Run(Page, Rec) on a page that declares SourceTableView: the page shows the rows of the record that the view also admits (the record's filters, then the view in filter group 2).",
        tests: "Assemble-to-Order Link ShowAsmOrders runs the Assembly Orders page on a temporary record; about 30 tests of the Tests-SCM-Assembly bucket.",
        probe: "A page with SourceTableView = where(Kind = const(B)) run on a temporary record holding A and B rows: the rows a handler sees; and on a record with a filter of its own.",
    },
    BooleanToTextAsFormat => {
        id: "boolean-to-text-as-format",
        assumed: "A Boolean given where a Text or Code is expected (Record.Get(Boolean) on a Code key) converts as Format writes it: Yes or No, upper case for a Code.",
        tests: "Item Tracking Code Get(Serial) in the SCM kitting tests; about 90 tests of the Tests-SCM-Assembly bucket.",
        probe: "Get(true) on a table with a Code[10] key holding the rows 'Yes' and 'YES' and '1'; assign a Boolean to a Text and a Code variable.",
    },
    FormatCodeSpelling => {
        id: "format-code-spelling",
        assumed: "The codes of a Format string are read in any case (`<sign><integer>`) and with blanks around the name and after the comma (`<precision, 2:2><standard format,0>`).",
        tests: "Microsoft's test libraries (`<sign><integer>`, about 60 tests of the Tests-SCM buckets) and Base Application's percent texts.",
        probe: "Format of an Integer, a Decimal and a Date with the same codes in capitals, small letters and with a blank after the comma; compare with the canonical spelling.",
    },
    TableExtensionBaseVars => {
        id: "table-extension-base-vars",
        assumed: "A table extension reads and writes the `protected var` variables of its base table, on the same per-record globals the base table's own code uses (a procedure of the extension sees what a trigger of the base set).",
        tests: "tableextension 906 \"Asm. Requisition Line\" (Assembly) uses Requisition Line's Item, TempPlanningErrorLog, PlanningResiliency and MissingFieldValueErr; about 600 tests of the Tests-SCM-Planning bucket.",
        probe: "A table with a protected var, a table extension procedure that reads and writes it, and a base trigger that reads it back; on one record variable and on a second one; with a Record and a Codeunit protected var.",
    },
    DataItemsEmptyElement => {
        id: "dataitems-empty-element",
        assumed: "A report dataset saved with no row at all writes its DataItems element in the XmlWriter's short form <DataItems />. (Only the prefix <DataItems is pinned: handlers/PWReportSaveAsColumns.al:Labels_NoRowAtAll.)",
        tests: "Report SaveAs datasets of reports with no rows in Tests-ERM.",
        probe: "Confirmed by a corpus bump 9 probe (empty DataItems); delete when the bump is merged.",
        note: "Probably pinned by corpus bump 9 (\"Empty DataItems <DataItems /> confirmed\"): check and delete.",
    },
    EnumOptionIntegerOrdinal => {
        id: "enum-option-integer-ordinal",
        assumed: "An Enum value converts into an Option or an Integer, and a BigInteger into an Option or an Enum, by ordinal; an ordinal beyond the members is kept. (Integer into Option, Option into Integer and Option into Enum are pinned by types/PROptionInteger.al and types/PEOptionEnumBigText.al.)",
        tests: "Sales Line UpdatePlanned (the FlowField filter of Reserved Quantity: Option column against an Enum field) in about 7,000 Tests-ERM and Tests-SCM tests; Library - Variable Storage DequeueInteger of an enqueued enum (ERM TestMultipleGenJnlLines).",
        probe: "Opt := Variant(EnumValue), Int := Variant(EnumValue), a FlowField filter and a SetRange that compare an Option column with an Enum field, with an ordinal the Option lacks. Corpus-topic issue 67.",
    },
    ForOptionVariable => {
        id: "for-option-variable",
        assumed: "A for loop whose variable is an Option or Enum field or variable runs once per ordinal, holds the member in each round and is read back before it counts up; Enum bounds count as their ordinal. (An Integer loop with an Option bound is pinned by types/PROptionInteger.al.)",
        tests: "Sales Price Calc. Mgt. FindSalesLineDisc (for \"Sales Type\" := Customer to Campaign), reached by Job Consumption Basic and ERM Dimension Sales tests.",
        probe: "for R.Opt := R.Opt::A to R.Opt::C over an Option field and an Option variable; the value after the loop; a body that changes the variable.",
    },
    RecordIdInMessage => {
        id: "record-id-in-message",
        assumed: "A RecordId as a %1 of an error, or in TestField and FieldError text, reads as Format(RecordId) does (Table: key1,key2). (Format(RecordId) is pinned by record/TestProbeRecordRefContracts.al:472-502.)",
        tests: "Gen. Jnl.-Post Line PostVend (ERM Gen. Lines for Vendor, over 1,000 Tests-ERM and Tests-General Journal tests).",
        probe: "Error('%1', Rec.RecordId), StrSubstNo and TestField on a RecordId field, with a text key that needs quotes.",
    },
    DateTimeDateTimeMethods => {
        id: "datetime-date-time-methods",
        assumed: "The instance methods DateTime.Date() and DateTime.Time() give what DT2Date and DT2Time give, the undefined DateTime included. (DT2Date and DT2Time are pinned by text/TestProbeTextDateFormatContracts.al:Dates_DateTime_DT2DateTime.)",
        tests: "About 975 Tests-ERM, Tests-SCM and Tests-General Journal tests (Library - Random and Library - Utility date and time helpers).",
        probe: "Dt.Date() and Dt.Time() for a set DateTime and for 0DT, next to DT2Date(Dt) and DT2Time(Dt).",
    },
    LoadFieldsNonStored => {
        id: "load-fields-non-stored",
        assumed: "SetLoadFields and AddLoadFields accept a field that is not stored (a FlowField) and add nothing to the load request for it. (The documentation says a runtime error or a false result; Microsoft's own code passes a FlowField and runs on real BC.)",
        tests: "Sales Line SetLoadFieldsForInvDiscoundCalculation (\"Reserved Quantity\"): about 2,350 Tests-ERM and Tests-SCM tests.",
        probe: "SetLoadFields(FlowField) and SetLoadFields(Normal, FlowField) then FindFirst and AreFieldsLoaded of the normal field; AddLoadFields the same; the result of SetLoadFields used as a value.",
    },
    SetTableTemporaryIntoPermanent => {
        id: "settable-temporary-into-permanent",
        assumed: "RecordRef.SetTable of a RecordRef on a temporary record into a record variable that is not declared temporary makes that variable temporary and share the temporary rows. (Permanent into permanent and temporary into temporary are pinned by recordref/TestRecordRefGetSetTableContracts.al.)",
        tests: "Mfg. Dimension Management OnCheckDimValuePostingOnAfterCreateDimTableIDs: SetTable(PurchaseLine) of the temporary Purchase Line of a posting check; about 1,450 Tests-ERM and Tests-SCM tests.",
        probe: "Tmp.Insert; RR.GetTable(Tmp); RR.SetTable(Perm); Perm.IsTemporary, Perm.Get, Perm.Insert and Perm.Count afterwards; the same for a permanent RecordRef into a temporary variable.",
    },
    HttpRelativeRequestPath => {
        id: "http-relative-request-path",
        assumed: "HttpClient.Get/Post/Put/Delete/Patch with a path that is not an absolute uri joins it to the client's base address like new Uri(base, path): a rooted path replaces the base path, any other joins the base path's directory.",
        tests: "Microsoft and NP Retail code that sets SetBaseAddress and then Get('/orders').",
        probe: "SetBaseAddress('https://example.invalid/api/v1/') then Get('orders'), Get('/orders'), Get('../x'), Get('?q=1') with a handler printing Request.Path.",
    },
    HttpContentTypeOfStream => {
        id: "http-content-type-of-stream",
        assumed: "HttpContent.WriteFrom(InStream) gives the content the Content-Type application/octet-stream, and a content nobody wrote to has text/plain; charset=utf-8. (WriteFrom(Text) gives text/plain; charset=utf-8: misc/W4XmlJsonHttp.al:WriteFromText_HeadersAndRoundTrip; a fresh content has a Content-Type: network/lt-http/HttpContent.al:ContentTypeHeaderAfterWrite.)",
        tests: "Code that posts a stream and then reads the content headers.",
        probe: "Content.WriteFrom(InS); Content.GetHeaders(H); H.GetValues('Content-Type', V) on a fresh content and after WriteFrom(InStream).",
    },
    HttpContentWriteResetsHeaders => {
        id: "http-content-write-resets-headers",
        assumed: "HttpContent.WriteFrom and Clear give the content a new header collection that holds only the Content-Type: headers added before are gone, and an HttpHeaders handed out before no longer shows the content's headers. (network/lt-http/HttpContent.al:ClearContent pins Clear; the re-fetch of the headers after WriteFrom in ContentTypeHeaderAfterWrite suggests the same.)",
        tests: "Code that calls GetHeaders and adds headers before WriteFrom.",
        probe: "Content.GetHeaders(H); H.Add('X-A','1'); Content.WriteFrom('x'); then H.Contains('X-A') and a new GetHeaders(H2).Contains('X-A').",
    },
    HttpHeaderRules => {
        id: "http-header-rules",
        assumed: "HttpHeaders.Add checks a header the way the probes measured: the name is a token (empty: Argument, otherwise Format), the value has no line break, a content header is refused on request, response and client headers, and Accept, Authorization, Date and Content-Type values are checked; every other header .NET types (Cache-Control, Range, Via, ...) is accepted as written, and the content headers other than Content-Type and Content-Length follow the same rule as those two.",
        tests: "Every Microsoft or NP Retail test that builds a request with headers (Authorization, Accept, Content-Type, custom X- headers).",
        probe: "network/lt-http/HttpHeaders.al covers the measured part; wanted: Cache-Control, Range, If-Match, Content-Disposition, Content-Encoding and Expires on request and content headers.",
    },
    HttpHandlerEveryVersion => {
        id: "http-handler-every-version",
        assumed: "An [HttpClientHandler] answers HttpClient calls on every BC major and environment. (The probe PZHttpHandler.al measured it on the newest major; on the two older ones a request was not given to the handler, and NP Retail notes that handlers intercept only on an on-prem tier. ADR-027 owner decision 1: handler-served on all.) The response a handler leaves keeps 200 OK for what it does not set, a handler that answers false or nothing intercepts, a call answers true whatever the status, and a handler that answers true is treated as a failed send (http-true-is-unresolvable).",
        tests: "NP Retail tests with [HttpClientHandler] mocks (Entria, Adyen, EFT, Azure Key Vault, Ticket Module): about 2,000 tests; 319 in the Microsoft buckets (CrossIntercompany Connector.Send).",
        probe: "PZHttpHandler.al on the two older majors as an on-prem service tier; a handler that sets only the status 404 (reason phrase?); a handler error.",
    },
    HttpTrueIsUnresolvable => {
        id: "http-true-is-unresolvable",
        assumed: "An [HttpClientHandler] that answers true makes the request fail as one to an unresolvable host does (the call answers false, the response keeps status 0), whatever the host. (Pinned for example.invalid by session/PZHttpHandler.al:HandlerAnsweringTrue_SendsTheRealRequest; the runtime never sends, so a host that BC could resolve is treated the same.)",
        tests: "NP Retail and Microsoft tests whose handler falls through with true and then expects a failed call or a real answer.",
        probe: "A handler answering true for a resolvable host on an environment without a network: the Boolean, the status and GetLastErrorText.",
    },
    FilterPlaceholderInQuotes => {
        id: "filter-placeholder-in-quotes",
        assumed: "In SetFilter, a %n inside a quoted part of the filter text is replaced by the argument like any other placeholder (the text '''%1''|''%2''' with two blank texts is the filter ''|'').",
        tests: "Gen. Jnl.-Show CT Entries (Credit Transfer Register UT.TestShowCTEntriesForGenJnlLine and the Payment Export tests); Base Application filters written '''%1'''.",
        probe: "SetFilter(TextField, '''%1''|''%2''', '', 'A') on rows with blank and A, then GetFilter and Count; with an Integer and a Code argument.",
    },
    PageEventSender => {
        id: "page-event-sender",
        assumed: "The sender of an event a page publishes with IncludeSender is the page itself (a Page variable on the running instance), not the page's Rec.",
        tests: "Item Tracking Lines OnSetSourceSpecOnCollectTrackingData with the subscriber in Asm. Item Tracking Mgt. (var sender: Page \"Item Tracking Lines\"); about 800 Tests-ERM and Tests-SCM tests that open item tracking.",
        probe: "A page that publishes [IntegrationEvent(true, false)], a subscriber with (var sender: Page X) that calls a procedure of the page and reads its Caption, from TestPage.OpenView and from a Page variable.",
    },
    DurationToNumber => {
        id: "duration-to-number",
        assumed: "A Duration converts into an Integer, a BigInteger or a Decimal as its number of milliseconds; a value beyond Integer is refused. (A whole number into a Duration is pinned by record/TestCalcFormulaValidationTests.Codeunit.al:42.)",
        tests: "Calendar Management CalcTimeDelta (Result := EndingTime - StartingTime into an Integer): about 750 Tests-ERM, Tests-SCM-Service and Tests-Job tests.",
        probe: "I := D; B := D; Dec := D for a Duration D, also negative and beyond Integer, and I := EndTime - StartTime.",
    },
    DataItemViewAndTableView => {
        id: "data-item-view-and-table-view",
        assumed: "A report data item whose DataItemTableView filters, run with a SetTableView (or a record given to the static Run), shows the rows that pass both: the filters of the view are added to the item's own, which live in the report's filter group. (A DataItemTableView that only sorts keeps the SetTableView filters: handlers/TestReportRuntimeContracts.al:393-410.)",
        tests: "Whse.-Source - Create Document and Production Order callers (Warehouse Request, Item data items): about 800 Tests-SCM and Tests-ERM tests.",
        probe: "A data item with DataItemTableView = where(F = filter(1..2)) run after SetTableView with F in 2..3, with a filter on another field, and the item's GetFilters in OnPreDataItem.",
    },
    ForBoolean => {
        id: "for-boolean",
        assumed: "A for loop over a Boolean variable (for B := false to true do) runs for false and then for true, and downto from true to false; the variable holds the Boolean in each round; its value after the loop is not read.",
        tests: "Library codeunits with `for Stepwise := false to true do` (about 410 Tests-ERM and Tests-SCM tests).",
        probe: "for B := false to true do counting the passes, the value of B inside and after the loop, and for B := true downto false.",
    },
    TimeAddWholeDecimal => {
        id: "time-add-whole-decimal",
        assumed: "A Time plus or minus a whole-number Decimal moves like an Integer. (A Time with an Integer, a Duration and a fractional Decimal is pinned by types/PUTimeArithmetic.al.)",
        tests: "Service Item Line CalculateResponseDateTime through Calendar Management: about 820 Tests-ERM, Tests-SCM-Service and Tests-Job tests.",
        probe: "T + 1000.0 and T - 1000.0 for a Decimal variable holding a whole number, and 0.",
    },
    PartLinkNotValidatedForUnsavedHost => {
        id: "part-link-not-validated-for-unsaved-host",
        assumed: "The blank line of a part whose host record is a new, unsaved row takes the SubPageLink values without validating them (the table relation of a link field would not find the host row yet); it is stamped and validated again when the host is saved.",
        tests: "Allocation Account (ListPlus page with parts linked on \"No.\"), OpenNew then \"No.\".SetValue: Alloc. Account Sales and Purchase E2E tests, about 34.",
        probe: "A card with a part linked on the card's key, DelayedInsert false: OpenNew, SetValue of the key, then read the part's draft line and its link field; SetValue of a second field and read again.",
    },
    PageRunModalRecordTakesSelection => {
        id: "page-run-modal-record-takes-selection",
        assumed: "Page.RunModal(Id, Rec) that answers LookupOK leaves in Rec the row the page was closed on, and not its own filters or key.",
        tests: "Gen. Journal Management TemplateSelection (PAGE.RunModal(0, GenJnlTemplate) = ACTION::LookupOK, then GenJnlTemplate.Name): ERM Apply Sales/Receivables and about 9 more Tests-ERM tests.",
        probe: "Page.RunModal(Page::X, Rec) with a handler that does GoToKey and OK; read Rec; the same with Cancel; with a page variable's RunModal and GetRecord.",
    },
    FilterChangeLandsOnNewLine => {
        id: "filter-change-lands-on-new-line",
        assumed: "When a TestPage filter (or key) change leaves an editable list without a row, the page stands on its blank line, with the single-valued filters on it (a page opened with OpenNew starts a new row again); typing into it inserts the row.",
        tests: "Item Vendor Catalog (OpenNew, Filter.SetFilter(\"Item No.\"), then SetValue of the vendor): ERM Item Reference Purchase and about 12 more Tests-ERM tests.",
        probe: "An editable list opened with OpenNew and with OpenEdit over an empty filtered result: SetValue of a non-key field and the stored row's key; Next() and the row triggers that run.",
    },
    DurationAddDuration => {
        id: "duration-add-duration",
        assumed: "A Duration plus or minus a Duration is a Duration of the summed or subtracted milliseconds; an overflow is refused. (Duration times a whole number, DateTime and Time with a Duration are pinned by types/TestDurationContracts.al and types/PUTimeArithmetic.al.)",
        tests: "Match Bank Payments (the telemetry timers of MatchEntries) through Tests-Bank ERM Bank Reconciliation and ERM Cash Manager: about 268 tests.",
        probe: "D1 + D2, D1 - D2 and a negative result for two Duration variables, Format(Result, 0, 9), and the overflow at the BigInteger edge.",
    },
    CalcSumsFlowFieldFilter => {
        id: "calcsums-flowfield-filter",
        assumed: "CalcSums on a record with a filter on a FlowField sums the stored field over the rows whose calculated FlowField value passes the filter. (CalcSums over stored-field filters is pinned by record/TestRecordFlowField.al:109-160.)",
        tests: "Bank Acc. Recon. Test.GetTotalOutstanding (SetRange/SetFilter on the Check Ledger Entries FlowField of the Bank Account Ledger Entry), reached by posting a bank reconciliation: about 200 Tests-Bank tests.",
        probe: "SetFilter(FlowField, '<>0') then CalcSums(StoredField), with rows whose FlowField is zero and non-zero, and a SetRange(FlowField, 0).",
    },
    DeleteAllMarkedOnly => {
        id: "delete-all-marked-only",
        assumed: "Record.DeleteAll with MarkedOnly(true) deletes the marked rows that pass the filters and no other row, like Find, Next, Count and ModifyAll work on the marked set; with triggers and events as a DeleteAll with a filter.",
        tests: "Post Inventory Cost to G/L (PostValueEntryToGL.MarkedOnly(true); DeleteAll() in OnPostDataItem, then CapValueEntryLoop Gets the capacity entries left): SCM Kitting D3 Part 2 and SCM Costing Batch, and every test that posts inventory cost with the report.",
        probe: "Mark two of three rows, MarkedOnly(true), DeleteAll: which rows remain; the same with a filter that excludes a marked row, and DeleteAll(true) with an OnDelete trigger.",
    },
    CodeDigitsCompareAsNumbers => {
        id: "code-digits-compare-as-numbers",
        assumed: "In <, <=, > and >= two Code values that are digits only and have no leading zero are ordered as numbers (the shorter is the smaller); anything else keeps the text order. Equality and the order of keys and filters are unchanged.",
        tests: "No. Series NoIsWithinValidRange (CurrentNo > EndingNo with the Cronus series P-CR+, 109001 to 1010999): Purch. Correct Cr. Memo, O365 Cancel Purch Cr Memo Dim., ERM Sales/Purch. Correct. Docs and every test that posts or corrects through that series. Pinned for one pair: '10' < '9' is No.",
        probe: "Code values '9' and '10', '09' and '9', '9A' and '10A', '100' and '99', '0' and '00', digits with a sign: the four operators and equality; and a SetRange over a Code field holding '9' and '10'.",
    },
    ReportRenderDiscarded => {
        id: "report-render-discarded",
        assumed: "A rendering save (SaveAsPdf, SaveAsExcel, SaveAsWord, SaveAsHtml, SaveAs to Pdf, Excel, Word or Html) of a report on the allow-list (ADR-040) runs the report as the Xml path does, answers true and writes no output; AL that looks at the output is refused.",
        tests: "Post Inventory Cost to G/L through Library - Costing and Library - Assembly PostInvtCostToGL (SCM Kitting D3 Part 1 and 2, SCM WIP Costing Production I and II, SCM Costing Batch); Check.SaveAsPdf (ERM Cash Manager, Match Bank Rec. Scenarios).",
        probe: "A report on the list saved to a temp file and to a stream on a Windows tier: the answer, the order of the triggers, File.Exists and TempBlob.Length afterwards.",
    },
    HiddenPartUpdateIsNoop => {
        id: "hidden-part-update-is-noop",
        assumed: "CurrPage.Update (also with false) in the code of a part page that the host's code created without opening it (a literal Visible = false) does nothing and raises no error.",
        tests: "Sales Invoice, Purchase Invoice and the other document pages call CurrPage.ApprovalFactBox.PAGE.UpdateApprovalEntriesFromSourceRecord(Rec.RecordId) in OnAfterGetCurrRecord, which ends in CurrPage.Update(false): about 900 Tests-ERM, Tests-SCM and Tests-Misc tests.",
        probe: "A host with part(P; ...) { Visible = false } whose OnAfterGetCurrRecord calls CurrPage.P.Page.Ping(), where Ping calls CurrPage.Update(false) and CurrPage.Update(true) and then reads Rec.",
    },
}

thread_local! {
    static HITS: Cell<u128> = const { Cell::new(0) };
}

/// Notes that the code path of `a` ran. Cheap: one thread-local read and write. The runner collects the
/// marks of each test with [`take_hits`].
#[inline]
pub fn assume(a: Assumption) {
    HITS.with(|h| h.set(h.get() | (1u128 << (a as u32))));
}

/// [`assume`] that answers true, for a match guard that decides to take the assumed path.
#[inline]
pub fn hit(a: Assumption) -> bool {
    assume(a);
    true
}

/// Forgets the marks of this thread (a test is about to start).
pub fn clear_hits() {
    HITS.with(|h| h.set(0));
}

/// The marks of this thread since the last [`clear_hits`] or `take_hits`, in registry order; clears them.
pub fn take_hits() -> Vec<Assumption> {
    let bits = HITS.with(|h| h.replace(0));
    Assumption::ALL
        .iter()
        .copied()
        .filter(|a| bits & (1u128 << (*a as u32)) != 0)
        .collect()
}

/// The entry with this id.
pub fn by_id(id: &str) -> Option<Assumption> {
    Assumption::ALL.iter().copied().find(|a| a.info().id == id)
}

/// The registry as the table `docs/probes-needed.md` holds between its markers.
pub fn markdown_table() -> String {
    let mut s = String::from("| Id | Assumed | Microsoft tests that need it | Probe wanted |\n|---|---|---|---|\n");
    for a in Assumption::ALL {
        let i = a.info();
        let cell = |t: &str| t.replace('|', "\\|");
        let note = i.note.map(|n| format!(" **{}**", cell(n))).unwrap_or_default();
        s.push_str(&format!(
            "| `{}` | {}{} | {} | {} |\n",
            i.id,
            cell(i.assumed),
            note,
            cell(i.tests),
            cell(i.probe)
        ));
    }
    s
}

/// The registry for `--list-assumptions`; `counts` gives, per id, (flagged passes, other tests that hit it)
/// of the last run.
pub fn listing(counts: &dyn Fn(&str) -> Option<(usize, usize)>) -> String {
    let mut s = String::new();
    for a in Assumption::ALL {
        let i = a.info();
        let hits = match counts(i.id) {
            Some((flagged, other)) => format!("{flagged} flagged passes, {other} other tests"),
            None => "not hit in the last run".to_string(),
        };
        s.push_str(&format!(
            "{}  ({hits})\n    assumed: {}\n    needed by: {}\n    probe wanted: {}\n",
            i.id, i.assumed, i.tests, i.probe
        ));
        if let Some(n) = i.note {
            s.push_str(&format!("    NOTE: {n}\n"));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_collected_per_thread_and_cleared() {
        clear_hits();
        assume(Assumption::GuidToText);
        assume(Assumption::GuidToText);
        assume(Assumption::TransferFieldsEnumOption);
        assert_eq!(
            take_hits(),
            vec![Assumption::TransferFieldsEnumOption, Assumption::GuidToText]
        );
        assert!(take_hits().is_empty());
        assume(Assumption::GuidToText);
        std::thread::spawn(|| assert!(take_hits().is_empty())).join().unwrap();
        clear_hits();
    }

    #[test]
    fn ids_are_unique_stable_words() {
        let mut ids: Vec<&str> = Assumption::ALL.iter().map(|a| a.info().id).collect();
        for id in &ids {
            assert!(
                !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{id}"
            );
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), Assumption::ALL.len());
        assert!(Assumption::ALL.len() <= 128, "the marks are a u128");
        for a in Assumption::ALL {
            assert_eq!(by_id(a.info().id), Some(*a));
        }
    }

    /// Every `*.rs` file under `crates/*/src`.
    fn sources() -> Vec<(std::path::PathBuf, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if p.is_dir() {
                    if matches!(name, "target" | "tests" | ".git" | "corpus") {
                        continue;
                    }
                    stack.push(p);
                } else if name.ends_with(".rs") && p.to_string_lossy().contains("/src/") {
                    if let Ok(t) = std::fs::read_to_string(&p) {
                        out.push((p, t));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn every_entry_is_marked_somewhere() {
        let files = sources();
        let mut unused = Vec::new();
        for a in Assumption::ALL {
            let needle = format!("Assumption::{a:?}");
            let used = files
                .iter()
                .filter(|(p, _)| !p.ends_with("assumptions.rs"))
                .any(|(_, t)| t.contains(&needle));
            if !used {
                unused.push(a.info().id);
            }
        }
        assert!(
            unused.is_empty(),
            "registered but never marked by a code path (delete the entry or add the marker): {unused:?}"
        );
    }

    const BEGIN: &str = "<!-- assumptions:begin (generated from crates/al-builtins/src/assumptions.rs; UPDATE_ASSUMPTIONS_DOC=1 cargo test -p al-builtins assumptions rewrites it) -->";
    const END: &str = "<!-- assumptions:end -->";

    #[test]
    fn the_table_in_probes_needed_is_generated_from_the_registry() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/probes-needed.md");
        let text = std::fs::read_to_string(&path).expect("docs/probes-needed.md");
        let want = format!("{BEGIN}\n{}{END}", markdown_table());
        let (Some(a), Some(b)) = (text.find("<!-- assumptions:begin"), text.find(END)) else {
            panic!("docs/probes-needed.md has no assumptions block: add the two marker lines");
        };
        let have = &text[a..b + END.len()];
        if have != want {
            if std::env::var_os("UPDATE_ASSUMPTIONS_DOC").is_some() {
                let new = format!("{}{want}{}", &text[..a], &text[b + END.len()..]);
                std::fs::write(&path, new).expect("write docs/probes-needed.md");
            } else {
                panic!(
                    "the assumptions table of docs/probes-needed.md is out of date: run UPDATE_ASSUMPTIONS_DOC=1 cargo test -p al-builtins assumptions"
                );
            }
        }
    }
}
