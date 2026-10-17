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
    CodeCoverageRefreshIsANoOp => {
        id: "code-coverage-refresh-is-a-no-op",
        assumed: "CodeCoverageRefresh() changes nothing: the Code Coverage table is served from the live recording, so a read after CodeCoverageLog(false, ...) already holds the hits.",
        tests: "System Application's Code Coverage Mgt.Refresh, called by Microsoft's code coverage helpers; 9 Tests-SCM-Costing tests.",
        probe: "CodeCoverageLog(true, false), run code, CodeCoverageLog(false, false); count the Code Coverage rows before and after CodeCoverageRefresh().",
    },
    NumberToTextIsFormat => {
        id: "number-to-text-is-format",
        assumed: "An Integer, BigInteger, Decimal or Enum value (a Variant from FieldRef.Value) assigned to a record's Text or Code field is its default Format text (an enum value its member name), as Format gives it.",
        tests: "Workflow Table Relation Value.CreateNew (Value := FieldRef.Value()); 114 Tests-Workflow tests.",
        probe: "A Variant holding an Integer, a BigInteger (2645 and 1234567), a Decimal and an enum value assigned to a Text[50] and a Code[50] field and variable and a Text parameter; compare with Format.",
    },
    QueryClosedAfterLastRow => {
        id: "query-closed-after-last-row",
        assumed: "A query whose Read has returned false after its last row is closed: SetRange and SetFilter take a new filter and Open runs it again.",
        tests: "Workflow Step Instance.SetTableRelationValues (one query variable, filtered and opened once per table relation); 125 Tests-Workflow tests.",
        probe: "Open a query, Read to false, SetRange on a column, Open and Read again; the same without the SetRange; Open twice after the last row.",
    },
    XmlportExportsFlowfieldCalculated => {
        id: "xmlport-exports-flowfield-calculated",
        assumed: "An XmlPort field element or attribute bound to a FlowField exports the calculated value, as if CalcFields had run on the row; an import sets the in-memory value only.",
        tests: "Import / Export Workflow (\"Workflow Step Argument\".\"Response Option Group\", a Lookup FlowField); 76 Tests-Workflow tests.",
        probe: "An export-only XmlPort with a fieldelement over a Sum FlowField and over a Lookup FlowField of a row with children: the exported text with and without a CalcFields on the row before; an import of the same element.",
    },
    RecordRefIntoRecord => {
        id: "recordref-into-record",
        assumed: "A RecordRef (held by a Variant) assigned or passed to a Record of the same table gives a copy of the record the RecordRef is on, filters included; another table is refused.",
        tests: "Approvals Mgmt.InformUserOnStatusChange (ShowSalesApprovalStatus(Variant)); 73 Tests-Workflow tests.",
        probe: "V := RecRef (a RecordRef on a filtered row); Rec := V and a Record parameter p(V): the fields, GetFilters, position and a later Modify; the same with a RecordRef of another table.",
    },
    FieldRefCalcFieldOfABlob => {
        id: "fieldref-calcfield-of-a-blob",
        assumed: "FieldRef.CalcField on a Blob field is accepted and loads the blob, as Record.CalcFields does (record/TestCalcFieldsFieldClass.Codeunit.al); on another stored field it stays refused.",
        tests: "Temp Blob Impl.FromFieldRef (FieldRef.CalcField on each Blob it reads); 14 Tests-SCM-Manufacturing tests (Demand Forecast Card).",
        probe: "A FieldRef on a Blob field of a stored row read without CalcFields: CalcField() then Value and HasValue; the same on a Code field (the error text) and on a FlowField.",
    },
    NextWholeDecimal => {
        id: "next-whole-decimal",
        assumed: "Record.Next(Decimal) with a whole number steps like the Integer; a fraction is refused.",
        tests: "Phys. Invt. Count.-Management.CalcPeriod (Calendar.Next(12 / CountFrequency)).",
        probe: "R.Next(1.5) and R.Next(-1.5).",
    },
    TableRelationQuotes => {
        id: "table-relation-quotes",
        assumed: "A filter(...) text of a TableRelation where() clause quotes an identifier with \" and is read like a SetFilter text after the quotes are rewritten to '.",
        tests: "Item where(Type = filter(Inventory | \"Non-Inventory\")) (Sales Line No.); tests in Tests-ERM.",
        probe: "TableRelation ... where(F = filter(A | \"B C\")) and Validate with a value of each kind.",
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
    GuidAsTextArgument => {
        id: "guid-as-text-argument",
        assumed: "A Guid passed where a function wants a Text is its text form {XXXXXXXX-...}, as in an assignment.",
        tests: "Match General Jnl Lines UT (CopyStr(CreateGuid(), 1, 50)); 61 Tests-General Journal tests.",
        probe: "CopyStr(G, 1, 50), StrLen(G), UpperCase(G).",
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
    QueryColumnRecordId => {
        id: "query-column-record-id",
        assumed: "A query column over a RecordId field returns the stored RecordId, groups and orders by it like the field, and cannot carry a Method, a ColumnFilter or ReverseSign (refused).",
        tests: "Base Application's \"Workflow Instance\" query (column Record_ID), opened by Workflow Step Instance.SetTableRelationValues and Approvals Mgmt.; 142 Tests-Workflow tests.",
        probe: "A query with a column over a RecordId field of a table with rows: Open, Read, Format of the column, grouping and OrderBy on it, SetFilter on the column, SaveAsCsv.",
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
    CaptionAssignedBeforeThePageRuns => {
        id: "caption-assigned-before-the-page-runs",
        assumed: "`PageVariable.Caption := x` before the variable's first Run or RunModal raises nothing, and the page shows x when it opens (its own OnOpenPage may assign another).",
        tests: "SCM Transfers (UndoTransShpt_SNTracking and about 20 more Tests-SCM tests): Item Tracking Data Collection.SelectMultipleTrackingNo sets the caption of Item Tracking Summary before it runs.",
        probe: "A page variable with Caption := 'X' before RunModal; read TestPage.Caption in the handler; the same with OnOpenPage assigning CurrPage.Caption.",
    },
    SetValueWholeNumberIntoTextControl => {
        id: "setvalue-whole-number-into-text-control",
        assumed: "TestPage SetValue of an Integer, BigInteger or whole Decimal (below 1000 in size) on a Text or Code control enters the number's digits.",
        tests: "SCM RTAM Item Tracking-II (RegisterWhseJournalLineWithSNWarehouseTracking): WhseItemTrackingPageHandler sets Serial No. to a Decimal.",
        probe: "SetValue(14), SetValue(14.0), SetValue(1234) and SetValue(-3) on a Text[30] and a Code[30] control bound to a field and to a variable.",
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
    DatabaseSidNumbers => {
        id: "database-sid-numbers",
        assumed: "Database.SID answers S-1-5-21-a-b-c-rid with a fixed domain part and a stable hash of the name as the rid.",
        tests: "Library - Document Approvals.GetUser; 214 Tests-Dimension tests.",
        probe: "Nothing in AL can compare the numbers with BC's; the shape is pinned (PXDatabaseSid.al).",
    },
    SaveValuesWithExtensions => {
        id: "savevalues-with-extensions",
        assumed: "A page extension that adds only controls bound to Rec fields, labels, parts or actions changes nothing about what a SaveValues page stores.",
        tests: "Pages with SaveValues and page extensions in the buckets.",
        probe: "A SaveValues page extended by a control bound to a variable and by one bound to a field.",
    },
    DateToDateTime => {
        id: "date-to-datetime-is-midnight",
        assumed: "A Date assigned or converted to a DateTime is that day at midnight; the blank Date is the blank DateTime.",
        tests: "Microsoft's production order and routing code (Starting Date-Time := Starting Date): Tests-SCM-Manufacturing, about 20 tests.",
        probe: "DT := D for a defined and for a blank Date; Format(DT, 0, 9); DT := ClosingDate(D).",
    },
    RelationWhereReadsValidatedValue => {
        id: "relation-where-reads-the-validated-value",
        assumed: "A where() condition of a TableRelation that names the validated field itself (Code = field(\"Variant Code\")) reads the value being validated, not the value the field holds before the assignment.",
        tests: "Prod. Order Line, Assembly Header and Assembly Line \"Variant Code\" (Item Variant.Code where(\"Item No.\" = field(\"Item No.\"), Code = field(\"Variant Code\"), ...)): Tests-SCM and Tests-SCM-Assembly, about 60 tests, among them SCM Order Tracking OrderTrackingOnSalesWithVariant.",
        probe: "A table relation whose where() has Code = field(<the field itself>); Validate a value that exists; read the field and the error.",
    },
    FilterTextReadAsEvaluate => {
        id: "filter-text-read-as-evaluate",
        assumed: "A text value of a filter on a Date field (SetFilter, SetRange with text) reads as Evaluate reads the text, in the runtime's fixed region (en-US short date `01/17/27`, closing date `C01/17/27`). A formula such as 1M, a Time or DateTime text and an empty text stay refused. (A Guid filter text is pinned by types/lt-in-precedence-date-guid-filters/P143Guid_Filters.al.)",
        tests: "about 700 tests of the Tests-SCM buckets filter dates with text built by Format.",
        probe: "SetFilter on a Date field with Format(D), with '..' + Format(D), and with '' and '1M'; then GetFilter and Count.",
    },
    BooleanToTextAsFormat => {
        id: "boolean-to-text-as-format",
        assumed: "A Boolean given where a Text or Code is expected (Record.Get(Boolean) on a Code key) converts as Format writes it: Yes or No, upper case for a Code.",
        tests: "Item Tracking Code Get(Serial) in the SCM kitting tests; about 90 tests of the Tests-SCM-Assembly bucket.",
        probe: "Get(true) on a table with a Code[10] key holding the rows 'Yes' and 'YES' and '1'; assign a Boolean to a Text and a Code variable.",
    },
    FormatCodeSpelling => {
        id: "format-code-spelling",
        assumed: "In a Format string `Filler Char` stands for `Filler Character`. (Any case of the other code names, and blanks after the comma, are pinned by types/lt-format-codes-textbuilder-pagerun-labels/P144FormatCodes.al.)",
        tests: "The Data Exchange tests' GetNodeID (`<Integer,4><Filler Char,0>`, about 30 tests).",
        probe: "Format(5, 0, '<Integer,4><Filler Char,0>') next to '<Integer,4><Filler Character,0>', for an Integer, a Decimal and a Text.",
    },
    EnumOptionIntegerOrdinal => {
        id: "enum-option-integer-ordinal",
        assumed: "A BigInteger converts into an Option or an Enum by ordinal; an ordinal beyond the members is kept. (An Enum value into an Option, an Integer or a Decimal, and an Integer into an Enum, are pinned: types/lt-filter-enum-relation-duration-guid/B_EnumOptionVariant.al, types/PROptionInteger.al, types/PEOptionEnumBigText.al.)",
        tests: "Code that assigns a BigInteger to an Option or an Enum variable.",
        probe: "Opt := BigIntegerValue and E := BigIntegerValue for an ordinal the Option or Enum lacks and for one beyond Integer.",
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
    HttpRelativeRequestPath => {
        id: "http-relative-request-path",
        assumed: "HttpClient.Get/Post/Put/Delete/Patch with a path that is not an absolute uri joins it to the client's base address like new Uri(base, path), as a [HttpClientHandler] sees it: a rooted path replaces the base path, any other joins the base path's directory. (That the request is blocked without a handler, for every relative form, is pinned by network/lt-http-relative-request/HttpRelativeRequestPath.al.)",
        tests: "Microsoft and NP Retail code that sets SetBaseAddress and then Get('/orders').",
        probe: "SetBaseAddress('https://example.invalid/api/v1/') then Get('orders'), Get('/orders'), Get('../x'), Get('?q=1') with a handler printing Request.Path.",
    },
    HttpHeadersAddUsedResult => {
        id: "http-headers-add-used-result",
        assumed: "HttpHeaders.Add whose result is used answers false for a value it refuses with InvalidHeader:Format, where a statement raises it. (Pinned for adding a Content-Type that is there: HttpWriteFromStream.al:P100_PresetContentType_ThenWrite against lt-http-header-rules/P87Twice.al.)",
        tests: "Code that tests the result of HttpHeaders.Add.",
        probe: "Format(H.Add(N, V)) for a bad value of each typed header (Accept, Authorization, Date, Content-Type) and for a bad name.",
    },
    HttpContentLengthAfterRead => {
        id: "http-content-length-after-read",
        assumed: "ReadAs(Text) of a content adds a Content-Length header of the body's length to its headers (the headers handed out before show it); a later WriteFrom sets the new length while the header is there. (PnE12HttpContent.al:WriteFromText_ThenReadAndHeaders reads 5 after WriteFrom('hello') and ReadAs; the P87 and P100 cases show none before a read.)",
        tests: "Code that reads a content and then its headers; ReadAs(InStream) is not covered.",
        probe: "WriteFrom('hello'), GetHeaders, Contains('Content-Length') before and after ReadAs(Text) and ReadAs(InStream); WriteFrom a longer text after the read and read the header.",
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
    DataItemViewAndTableView => {
        id: "data-item-view-and-table-view",
        assumed: "A report data item whose DataItemTableView filters, run with a SetTableView (or a record given to the static Run), shows the rows that pass both: the filters of the view are added to the item's own, which live in the report's filter group. (A DataItemTableView that only sorts keeps the SetTableView filters: handlers/TestReportRuntimeContracts.al:393-410.)",
        tests: "Whse.-Source - Create Document and Production Order callers (Warehouse Request, Item data items): about 800 Tests-SCM and Tests-ERM tests.",
        probe: "A data item with DataItemTableView = where(F = filter(1..2)) run after SetTableView with F in 2..3, with a filter on another field, and the item's GetFilters in OnPreDataItem.",
    },
    PartLinkNotValidatedForUnsavedHost => {
        id: "part-link-not-validated-for-unsaved-host",
        assumed: "The blank line of a part whose host record is a new, unsaved row takes the SubPageLink values without validating them (the table relation of a link field would not find the host row yet); it is stamped and validated again when the host is saved.",
        tests: "Allocation Account (ListPlus page with parts linked on \"No.\"), OpenNew then \"No.\".SetValue: Alloc. Account Sales and Purchase E2E tests, about 34.",
        probe: "A card with a part linked on the card's key, DelayedInsert false: OpenNew, SetValue of the key, then read the part's draft line and its link field; SetValue of a second field and read again.",
    },
    FilterChangeLandsOnNewLine => {
        id: "filter-change-lands-on-new-line",
        assumed: "When a TestPage filter (or key) change leaves an editable list without a row, the page stands on its blank line, with the single-valued filters on it (a page opened with OpenNew starts a new row again); typing into it inserts the row.",
        tests: "Item Vendor Catalog (OpenNew, Filter.SetFilter(\"Item No.\"), then SetValue of the vendor): ERM Item Reference Purchase and about 12 more Tests-ERM tests.",
        probe: "An editable list opened with OpenNew and with OpenEdit over an empty filtered result: SetValue of a non-key field and the stored row's key; Next() and the row triggers that run.",
    },
    DeleteAllMarkedOnly => {
        id: "delete-all-marked-only",
        assumed: "Record.DeleteAll with MarkedOnly(true) deletes the marked rows that pass the filters and no other row, like Find, Next, Count and ModifyAll work on the marked set; with triggers and events as a DeleteAll with a filter.",
        tests: "Post Inventory Cost to G/L (PostValueEntryToGL.MarkedOnly(true); DeleteAll() in OnPostDataItem, then CapValueEntryLoop Gets the capacity entries left): SCM Kitting D3 Part 2 and SCM Costing Batch, and every test that posts inventory cost with the report.",
        probe: "Mark two of three rows, MarkedOnly(true), DeleteAll: which rows remain; the same with a filter that excludes a marked row, and DeleteAll(true) with an OnDelete trigger.",
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
        assume(Assumption::ClearAllKeepsTemporaryRows);
        assume(Assumption::ClearAllKeepsTemporaryRows);
        assume(Assumption::FindEqualOnAnotherKey);
        assert_eq!(
            take_hits(),
            vec![
                Assumption::FindEqualOnAnotherKey,
                Assumption::ClearAllKeepsTemporaryRows
            ]
        );
        assert!(take_hits().is_empty());
        assume(Assumption::ClearAllKeepsTemporaryRows);
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
