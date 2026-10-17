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
    PageFieldFollowsSourceFieldEditableFalse => {
        id: "page-field-follows-source-field-editable-false",
        assumed: "A page field over a source table field that declares Editable = false, with no Editable of its own on the control, reads Editable() No on a TestPage.",
        tests: "Payment Export Mgt Unit Test.FilterByHasPaymentExportError (Gen. Journal Line.\"Has Payment Export Error\", a FlowField with Editable = false, on the Payment Journal).",
        probe: "A table field with Editable = false (stored, and a FlowField) and one without; a page with a plain field control over each and one that sets Editable = true itself: Editable() of each control on an open TestPage, and whether SetValue on the first is refused.",
    },
    ListPlusBlankRecordOnEmptyView => {
        id: "listplus-blank-record-on-empty-view",
        assumed: "A ListPlus page without a repeater, opened for edit over an empty view, stands on a blank record: OnNewRecord and OnAfterGetCurrRecord run after OnOpenPage. A Card or Document over an empty view is not covered.",
        tests: "Suggest Price Lines UT.T212_CopyLinesEnabledOnPurchPriceListPageIfFixedUpgradeCompleted (Purchase Price List sets PriceListIsEditable in OnAfterGetCurrRecord).",
        probe: "A ListPlus, a Card and a Document page over an empty table, each with OnOpenPage, OnNewRecord, OnAfterGetRecord and OnAfterGetCurrRecord logging, opened with OpenEdit and OpenView: the trigger sequence and the field values.",
    },
    UpperLimitOfASingleValue => {
        id: "upperlimit-of-a-single-value",
        assumed: "field(upperlimit(F)) of a flow filter that holds one value (SetFilter(F, Format(d))) applies the range ..d, so the single value is its own upper limit; other non-range filters stay refused.",
        tests: "Customer.\"Balance Due\" and \"Balance Due (LCY)\" (Detailed Cust. Ledg. Entry `Initial Entry Due Date = field(upperlimit(Date Filter))`): Tests-ERM UT_CustBalanceDueCalcOnMaxLimitOfDateFilter and 10 more.",
        probe: "A flow filter set with SetFilter to a single value, SetFilter('..d') and SetFilter('<=d'), then CalcFields on a Sum with `Date = field(upperlimit(Filter))`: which rows are included.",
    },
    RenameFollowsFilterConditionArms => {
        id: "rename-follows-filter-condition-arms",
        assumed: "A rename of a primary key rewrites a referencing field through a TableRelation arm whose conditions include `Field = filter(x)`, when the referencing row's field passes the filter, as it does for `const` conditions.",
        tests: "Tests-ERM price list tests that rename an Item, Resource, G/L Account or Item Variant (Assembly Header.\"Bin Code\" `Quantity = filter(< 0)`, Purchase Line.\"No.\" and \"Variant Code\" `Document Type = filter(<> \"Credit Memo\" & <> \"Return Order\")`); about 28 Tests-ERM tests.",
        probe: "A table with a field whose TableRelation is `if (Kind = filter(<> B)) Parent else Other`: rename a Parent row with a referencing row of Kind A and one of Kind B; read both fields.",
    },
    RelationOptionBlankConst => {
        id: "relation-option-blank-const",
        assumed: "A TableRelation condition `Field = const(\" \")` on an Option field selects the arm when the field holds the option's blank member (ordinal of the member written \" \"), as it does for an Enum.",
        tests: "Finance Charge Memo Line.\"No.\" (`if (Type = const(\" \")) \"Standard Text\"`); Tests-ERM ERM Finance Charge Memo Apply IssueFinanceChargeMemoWithInvoiceRounding and 25 more.",
        probe: "An Option field with OptionMembers = \" \",A and a TableRelation if (Opt = const(\" \")) T1 else if (Opt = const(A)) T2: Validate a value of each table with Opt at each member.",
    },
    SetViewFieldByCaption => {
        id: "set-view-field-by-caption",
        assumed: "SetView resolves a field name that no field has by the field's caption (GetView writes captions, so SetView(GetView()) restores the view of a table whose caption differs from the name); a name wins over a caption.",
        tests: "Purchase Alloc. Acc. Mgt.ReplaceInheritFromParent (SetView(GetView()) on Allocation Line, whose \"Line No.\" has Caption 'Primary Key'); about 20 Tests-ERM tests.",
        probe: "A table with field \"Line No.\" (Caption 'Primary Key'): SetView(GetView()) after SetRange on it; SetView('WHERE(Primary Key=FILTER(2))'); a caption equal to another field's name.",
    },
    PackageAutoIncrementFromOne => {
        id: "package-auto-increment-from-one",
        assumed: "A table built from a RapidStart package whose rows hold no value for the table's AutoIncrement field (Dimension Value.\"Dimension Value ID\" is not in the shipped package) gets 1, 2, 3 in the order of the package's rows, as the database numbers rows it inserts, and the counter continues after them.",
        tests: "Sales-Calc. Discount Test LcyDiscountTest and ERM Sales Quotes SalesQuoteWithContact (a dimension set entry must carry a nonzero Dimension Value ID); Production & Assembly Costing AssemblyOrderWithDimensionShouldPostWithoutError.",
        probe: "Not measurable from the corpus (it depends on the import tool): import a RapidStart package into Dimension Value and read the field. Until then the rule rests on the field being AutoIncrement and absent from Microsoft's package.",
    },
    DatabaseTriggerSetupPerCompany => {
        id: "database-trigger-setup-per-company",
        assumed: "The answer of the Global Triggers event GetDatabaseTableTriggerSetup is not reused after the session's company changes (the install baseline's per-database roots run with no company after the per-company roots).",
        tests: "Microsoft's install roots Plan Installer, Upgrade Tag Install and No. Series Copilot Install must run to their end on real BC for Microsoft's tests to pass; their setup subscribers (Change Log Management.GetDatabaseTableTriggerSetup, CRM Integration Management.GetDatabaseTableTriggerSetup) exit when CompanyName() is empty.",
        probe: "A GetDatabaseTableTriggerSetup subscriber that logs CompanyName(); write a table, ChangeCompany, write it again: is the setup asked again per company?",
    },
    GetUrlWebBaseAddress => {
        id: "geturl-web-base-address",
        assumed: "GetUrl(ClientType::Web) without company and object returns the host's server base address as set (--server-url, default http://localhost:8080/BC/), unchanged (ADR-042).",
        tests: "Company-Initialize (Doc. Exch. Service Setup.SetURLsToDefault -> OAuth2Impl.GetDefaultRedirectUrl), so the install baseline of a company built from the RapidStart package; every Microsoft test that reads Application Area Setup or Workflow after it.",
        probe: "GetUrl(ClientType::Web) asserted for scheme, path and query, with and without a company, and inside a per-database install trigger (docs/probes-needed.md).",
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
    NavUserAccountHelperIsDelegatedUser => {
        id: "nav-user-account-helper-is-delegated-user",
        assumed: "DotNet NavUserAccountHelper.IsDelegatedUser() answers false for the test session user (the member is in no corpus test; the plan-based Azure AD User Management.IsUserDelegated is false in session/TestEnvironmentUserTenantContracts.al:245).",
        tests: "Azure AD Graph User Impl.IsUserDelegated, reached by 165 Microsoft tests (for example Suggest Price Lines UT T202_NotificationActionFixOpensJobQueueEntryCard).",
        probe: "A DotNet NavUserAccountHelper variable in a test session: IsDelegatedUser(), IsUserDelegatedAdmin(); and Codeunit \"Azure AD Graph User\".IsUserDelegated().",
    },
    FieldRefCalcFieldOfABlob => {
        id: "fieldref-calcfield-of-a-blob",
        assumed: "FieldRef.CalcField on a Blob field is accepted and loads the blob, as Record.CalcFields does (record/TestCalcFieldsFieldClass.Codeunit.al); on another stored field it stays refused.",
        tests: "Temp Blob Impl.FromFieldRef (FieldRef.CalcField on each Blob it reads); 14 Tests-SCM-Manufacturing tests (Demand Forecast Card).",
        probe: "A FieldRef on a Blob field of a stored row read without CalcFields: CalcField() then Value and HasValue; the same on a Code field (the error text) and on a FlowField.",
    },
    ModifyWithoutChangeKeepsRowVersion => {
        id: "modify-without-change-keeps-row-version",
        assumed: "A Modify of a persistent record whose fields equal the stored row's does not change the row's SystemRowVersion, so a buffer another variable read before it is not stale afterwards.",
        tests: "Create Prod. Order Lines.Copy (ProdOrder.Modify after Refresh Production Order) followed by the test's Modify of the same Production Order; 4 Tests-SCM-Manufacturing tests (SCM Prod. Whse. Handling).",
        probe: "Two variables Get the same row; one Modifies with no change and Commit; the other changes a field and Modifies: accepted or DB:RecordChanged; the same after a Modify that changes a field.",
    },
    EvaluateRecordIdCompositeKey => {
        id: "evaluate-recordid-composite-key",
        assumed: "Evaluate into a RecordId of a table whose primary key has several fields reads `Table: v1,v2,...` as Format writes it (a value that is empty or holds a comma is in double quotes): one value per key field, each read as Evaluate reads that field's type (a Code as written, an Integer or an Enum by its name or ordinal); a count of values other than the key's field count is rejected. A rejected text leaves a RecordId that is not blank as it was and answers false (any key).",
        tests: "Library - Error Message.LoadErrorMessages (Evaluate(TempErrorMessage.\"Record ID\", ErrorMessages.Source.Value, 9)); 36 Tests-Misc tests (Check Dimensions On Posting: Default Dimension, Sales Header, Dimension Value keys).",
        probe: "Evaluate(Rid, Format(Rid)) and Format(Rid, 0, 9) for a table with a Code+Code key, an Enum+Code key (Sales Header), an Integer+Code+Code key (Default Dimension), a Date key part and a Code value holding a comma; the result and Rid; and too few and too many values.",
    },
    TempRecordByValueToPersistentParameter => {
        id: "temp-record-by-value-to-persistent-parameter",
        assumed: "A temporary record passed by value to a parameter that is not declared temporary gives the callee a record of the stored table holding the current row (the copy is no longer temporary); a parameter declared temporary gets an independent empty buffer (pinned).",
        tests: "To-do.InsertTask / InsertTaskAndRelatedData (Task2: Record \"To-do\" receives the temporary Rec of CreateTaskFromTask, Task2.Insert(true), then Task2.Get); Tests-Marketing Marketing Matrix Management TaskWith* (10 tests).",
        probe: "A temporary record with a row, passed by value to a procedure with a parameter of the same table not declared temporary: the callee's IsTemporary, Insert then Count of the stored table, and whether the caller's temporary rows change.",
    },
    TestPageAssertEqualsIntegerOnTextControl => {
        id: "testpage-assertequals-integer-on-text-control",
        assumed: "TestPageField.AssertEquals(<Integer>) on a control over a Text or Code value compares the plain digits of the integer with the control's text, instead of raising TypeConversion; another number or an Enum there stays refused.",
        tests: "Tests-Marketing Marketing Matrix Management.TasksMatrixHandlerWithValue (Field1.AssertEquals(1) on the Text[1024] cells of Tasks Matrix); the 10 TaskWith* tests.",
        probe: "A page with a field over a Text[30] variable holding '1': TestPage Field.AssertEquals(1) and AssertEquals(2), also with a Decimal and a BigInteger expected value, and a Code[10] control.",
    },
    TestPageSetValueBlankOptionMember => {
        id: "testpage-setvalue-blank-option-member",
        assumed: "TestPageField.SetValue of the first (blank) option member on a control whose OptionCaption starts with a blank caption (' ,Not Started,...') is accepted and selects that member; the client reads the caption ' ' as blank.",
        tests: "Tests-Marketing Marketing Matrix Management.TaskWithStatus* (Tasks.StatusFilter.SetValue(StatusFilter::\" \") resets the filter); 7 tests.",
        probe: "A page with an Option variable control with OptionCaption ' ,A,B': TestPage SetValue(Var::\" \") after SetValue(A), then Value; and SetValue('') as text; the same for an option field of a table.",
    },
    TypedHandlerKeepsRecAfterSetTableView => {
        id: "typed-handler-keeps-rec-after-settableview",
        assumed: "A Page-typed modal handler of a page variable that was given only SetTableView (no SetRecord) sees the Rec the page's own code assigned before (`Rec := X` in a procedure the test called), not a blank record.",
        tests: "Contact Profile Answers.SetRunFromForm (Rec := ProfileQuestionnaireLine) then RunModal, handler calls UpdateProfileAnswer: Tests-Marketing Marketing Profiling CreateContactProfileManually, ContactToSegmentUsingProfile.",
        probe: "A page variable: SetTableView(T) then a page procedure that does Rec := T (a row), then RunModal with a Page-typed handler reading Rec.Id through another page procedure and GetRecord.",
    },
    TypedHandlerLookupOkHandsBackRec => {
        id: "typed-handler-lookupok-hands-back-rec",
        assumed: "When a Page-typed ModalPageHandler answers LookupOK, the record the code passed to Page.RunModal(Id, Rec) takes the page's record as the handler left it (SetRecord), as a TestPage handler's selected row does.",
        tests: "Segment Header.ReuseCriteria (PAGE.RunModal(PAGE::\"Saved Segment Criteria List\", SavedSegCriteria) answered by ModalFrmHandleSavedSegCriteria); Tests-Marketing SegmentSaveCriteriaAndReuse, ReuseSegmentCriteria, ContactToSegmentUsingProfile.",
        probe: "Page.RunModal(Page::P, Rec) with Rec on row 1 and a Page-typed handler that does Row.Get(3); P.SetRecord(Row); Response := LookupOK: Rec afterwards; the same with Cancel and with no SetRecord.",
    },
    EvaluateTimeDigitForms => {
        id: "evaluate-time-digit-forms",
        assumed: "Evaluate into a Time (format 0) reads six digits as hhmmss and one or two digits as an hour; an hour from 24, minutes or seconds from 60 are rejected.",
        tests: "Calendar Management CalcTimeDelta tests (Evaluate of '085606') and Evaluate(RandomTime, Format(RandInt(23))); 11 Tests-SCM-Manufacturing tests.",
        probe: "Evaluate(T, '085606'), '23', '5', '246000', '24', '0856', '8:56' into a Time: the result and the return value; with and without a format number.",
    },
    CodeCoverageSnapshotWhileLogging => {
        id: "code-coverage-snapshot-while-logging",
        assumed: "While CodeCoverageLog(true) is on, the Code Coverage tables hold the snapshot that CodeCoverageRefresh wrote and do not grow with later hits (a read before any refresh is empty, pinned by platform/lt-code-coverage-refresh/CodeCoverageRefreshLogged.al).",
        tests: "Tests that read the Code Coverage table after a refresh while logging is still on: none known in the buckets.",
        probe: "CodeCoverageLog(true, false), run code, CodeCoverageRefresh(), count the rows, run more code, count again (still logging), then CodeCoverageRefresh() and count.",
    },
    NextWholeDecimal => {
        id: "next-whole-decimal",
        assumed: "Record.Next(Decimal) with a whole number steps like the Integer; a fraction is refused.",
        tests: "Phys. Invt. Count.-Management.CalcPeriod (Calendar.Next(12 / CountFrequency)).",
        probe: "R.Next(1.5) and R.Next(-1.5).",
    },
    NotificationByValueShares => {
        id: "notification-by-value-shares",
        assumed: "A Notification passed by value to a procedure is the caller's own notification: SetData, Message and Scope set in the callee are seen by the caller. An assignment copies (pinned).",
        tests: "Check Credit Limit.PopulateDataOnNotification(CreditLimitNotification: Notification) sets the data the Tests-ERM SendNotificationHandler reads: 8 tests of ERM Sales Documents II.",
        probe: "A procedure P(N: Notification) that calls N.SetData('k','v') and N.Message('m'); the caller reads HasData('k') and Message() after the call; the same with a var parameter.",
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
        tests: "Match General Jnl Lines UT (CopyStr(CreateGuid(), 1, 50)); 61 Tests-General Journal tests. Error Message Handler.ShowNotification (Notification.SetData with a Guid); 27 Tests-ERM tests.",
        probe: "CopyStr(G, 1, 50), StrLen(G), UpperCase(G), Notification.SetData('k', G) then GetData('k').",
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
    UpdateRefreshListOnFindRecord => {
        id: "update-refresh-list-onfindrecord",
        assumed: "CurrPage.Update on a list with OnFindRecord (with or without OnNextRecord) reads the rows as the pinned table of 3 rows with the cursor on the middle one does (misc/W2FindRecordUpdate.al), for any number of rows and any cursor position below a window.",
        tests: "ERM Sales Subform (InvoiceAddingLinesUpdatesTotals) and about 120 more Tests-ERM tests whose sales and purchase subforms refresh.",
        probe: "The same list with 1, 2, 5 and 25 rows and the cursor on the first, last and a middle row; a list with OnFindRecord only; a list part of a card.",
    },
    SetValueWholeNumberIntoTextControl => {
        id: "setvalue-whole-number-into-text-control",
        assumed: "TestPage SetValue of an Integer, BigInteger or whole Decimal (below 1000 in size) on a Text or Code control enters the number's digits.",
        tests: "SCM RTAM Item Tracking-II (RegisterWhseJournalLineWithSNWarehouseTracking): WhseItemTrackingPageHandler sets Serial No. to a Decimal.",
        probe: "SetValue(14), SetValue(14.0), SetValue(1234) and SetValue(-3) on a Text[30] and a Code[30] control bound to a field and to a variable.",
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
    TrappedPageLeftOpenIsReplaced => {
        id: "trapped-testpage-left-open-is-replaced",
        assumed: "A TestPage variable that trapped a page and was left open (never closed) can trap the next Page.Run or RunObject of its page: the new page replaces the open one, whose close triggers do not run.",
        tests: "Library - Error Message keeps one TestPage \"Error Messages\" in a codeunit variable, traps it in every VerifyDimErrors and never closes it; Tests-Misc Check Dimensions On Posting (35 tests) trap it again in the next test of the codeunit.",
        probe: "A codeunit with a TestPage variable that Trap()s and runs a page in test one and leaves it open; test two Trap()s the same variable and runs the page again: does it open, and do OnQueryClosePage and OnClosePage of the first page run?",
    },
    RunModalFieldKeptUnlessLookupOk => {
        id: "runmodal-field-kept-unless-lookupok",
        assumed: "Page.RunModal(Id, Rec, Field) with a handler that answers anything but LookupOK leaves the field and the record as the handler left them; only LookupOK copies the page's value into the field (a LookupOK answer is refused).",
        tests: "Segment Line.RunCreateInteraction (PAGE.RunModal(PAGE::\"Create Interaction\", Rec, \"Interaction Template Code\")) in Tests-Marketing: CampaignAndInteractionsLinked and about 32 more, answered by Page-typed handlers that leave Response alone.",
        probe: "Page.RunModal(Page::P, Rec, Rec.F) with a ModalPageHandler that (a) invokes OK, (b) Cancel, (c) does nothing, (d) a Page-typed handler leaving Response; with P a card and a list in lookup mode, for LookupOK: what Rec.F holds and what the call returns.",
    },
    SubPageLinkEmptyTerm => {
        id: "subpagelink-empty-term-selects-blank",
        assumed: "An empty const('') or filter('') term of a SubPageLink or RunPageLink selects the rows whose field is blank, as the same term of a TableRelation where() does (pinned there, not for links).",
        tests: "Microsoft's Production BOM page (part ProdBOMLine: SubPageLink \"Version Code\" = const('')), opened by SCM Manufacturing 7.0 CalculateProdOrderWithSKUWithRoutingAndProdBOMNo and about 10 more Tests-SCM-Manufacturing tests.",
        probe: "A part page with SubPageLink F = const('') and another with F = filter(''), over rows with blank and non-blank F: which rows show; the same for RunPageLink.",
    },
    DurationDividedByDecimal => {
        id: "duration-divided-by-decimal",
        assumed: "A Duration divided by a Decimal counts the Duration as its milliseconds and answers a Decimal (no whole-millisecond requirement, unlike the pinned division by an Integer).",
        tests: "Microsoft's Calculate Routing Line CalcDuration (Time difference / CalendarMgt.TimeFactor): Tests-SCM-Manufacturing, SCM Production Journal CheckAllocatedCapacityTimeCalculation and about 20 more.",
        probe: "D: Duration := 26820000 and 30000; X := D / Dec60000 into a Decimal; Format(D / Dec60000); a Decimal divisor that is zero.",
    },
    DateToDateTime => {
        id: "date-to-datetime-is-midnight",
        assumed: "A Date assigned or converted to a DateTime is that day at midnight; the blank Date is the blank DateTime.",
        tests: "Microsoft's production order and routing code (Starting Date-Time := Starting Date): Tests-SCM-Manufacturing, about 20 tests.",
        probe: "DT := D for a defined and for a blank Date; Format(DT, 0, 9); DT := ClosingDate(D).",
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
    TestFieldBlobWithStoredContent => {
        id: "test-field-blob-with-stored-content",
        assumed: "TestField of a Blob field that holds content from the database (after CalcFields) passes. (An empty Blob fails and a Blob written through a stream and not saved passes: misc/lt-probes-needed-5/PnE08RecordMisc.al:TestField_Blob.)",
        tests: "Tests-ERM tests that call TestField on a Blob after CalcFields (XSD Parser `XMLSchema.TestField(XSD)`, about 25 tests).",
        probe: "Save a Blob with content, Get the record, CalcFields, then TestField(Blob); and TestField(Blob) after Get without CalcFields.",
    },
    ReportViewOverStoredRequestFilters => {
        id: "report-view-over-stored-request-filters",
        assumed: "A data item the caller gave a view (SetTableView, or the record of Report.Run or RunModal) keeps that view when the report has SaveValues and an earlier run stored other filters for the data item. (That an OK stores the view's filters is pinned by report/lt-requestpage-savevalues-date-text-setvalue/P147SaveValuesFilter.al:P147_Filter_SetTableViewVsStored.)",
        tests: "Tests-ERM tests that run `Suggest Fin. Charge Memo Lines` for a second memo in one codeunit (IssueFinanceChargeMemoWith* and others).",
        probe: "A report with SaveValues: run it once with the filter A* stored by OK, then with SetTableView(C*) and see the data item's filter on the request page and in the run; the same with the record of Report.RunModal.",
    },
    HttpRelativeRequestPath => {
        id: "http-relative-request-path",
        assumed: "HttpClient.Get/Post/Put/Delete/Patch with a path that is not an absolute uri joins it to the client's base address like new Uri(base, path), as a [HttpClientHandler] sees it: a rooted path replaces the base path, any other joins the base path's directory. (That a plain word and a rooted path with no base address, and the forms of P168 with a base address, are blocked without a handler is pinned by network/lt-http-relative-request/HttpRelativeRequestPath.al.)",
        tests: "Microsoft and NP Retail code that sets SetBaseAddress and then Get('/orders').",
        probe: "SetBaseAddress('https://example.invalid/api/v1/') then Get('orders'), Get('/orders'), Get('../x'), Get('?q=1') with a handler printing Request.Path. Also wanted: Get with no base address for 'a/b', 'a.b' and '?q=1' (only a plain word and a rooted word are pinned; the others stay refused).",
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
        assumed: "HttpHeaders.Add checks a header the way the probes measured: the name is a token (empty: Argument, otherwise Format), the value has no line break, a content header is refused on request, response and client headers, and Accept, Authorization, Date and Content-Type values are checked; every other header .NET types (Range, Via, ...) is accepted as written, and the content headers other than Content-Type and Content-Length follow the same rule as those two.",
        tests: "Every Microsoft or NP Retail test that builds a request with headers (Authorization, Accept, Content-Type, custom X- headers).",
        probe: "network/lt-http/HttpHeaders.al covers the measured part; the case and the content-or-not of all 39 known names are pinned by network/lt-http-header-names-cache-control/HeaderNameCase*.al. Wanted: the value rules of the typed headers other than Cache-Control (Range, If-Match, Content-Disposition, Content-Encoding, Expires and the rest) on request, response and content headers; Cache-Control (directive order, values, merging) is pinned by CacheControl*.al.",
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
    OpenNewSkipsOnFindRecord => {
        id: "open-new-skips-onfindrecord",
        assumed: "TestPage.OpenNew on a page with an OnFindRecord trigger does not run that trigger: the page finds its last row for xRec by itself (as for a page without the trigger) and goes to OnNewRecord.",
        tests: "Service Credit Memo, Service Invoice and Service Order pages (OnFindRecord: Rec.Find(Which), else SetRange(\"No.\") and Find again): ServiceCreditMemoByPage, ServiceInvoiceByPage and about 30 more Tests-SCM-Service tests, and the 24 that follow them in their codeunits.",
        probe: "A list or card with OnFindRecord(Which) that notes Which in a trace, a source table with and without rows: OpenNew, then the trace, xRec of OnNewRecord and the first Rec.",
    },
    TextToDecimalFromVariant => {
        id: "text-to-decimal-from-variant",
        assumed: "A Text of an optional sign, digits and at most one decimal point converts into a Decimal where a Variant meets a number, such as the return of DequeueDecimal; a text with anything else (thousands separators, other symbols, an empty text) stays refused.",
        tests: "Library - Variable Storage.DequeueDecimal after a page handler enqueued a control's Value text: Service Statistics (OrderStatisticsNegativeProfit and 11 more Tests-SCM-Service tests).",
        probe: "Dequeue a Variant holding Text '-111.69', '5', '1,000.5', '' and 'x' into a Decimal: the value or the error and its text.",
    },
    PartLinkNotValidatedForUnsavedHost => {
        id: "part-link-not-validated-for-unsaved-host",
        assumed: "The blank line of a part whose host record is a new, unsaved row takes the SubPageLink values without validating them (the table relation of a link field would not find the host row yet); it is stamped and validated again when the host is saved.",
        tests: "Allocation Account (ListPlus page with parts linked on \"No.\"), OpenNew then \"No.\".SetValue: Alloc. Account Sales and Purchase E2E tests, about 34.",
        probe: "A card with a part linked on the card's key, DelayedInsert false: OpenNew, SetValue of the key, then read the part's draft line and its link field; SetValue of a second field and read again.",
    },
    NewLineStampsAllBeforeValidating => {
        id: "new-line-stamps-all-before-validating",
        assumed: "The blank line of a part (or list) takes every single-valued filter value (key fields, Document Type from SourceTableView and the SubPageLink fields) onto the record first, and only then runs the OnValidate and table relation of each, in field order; a link field declared before Document Type is validated with the Document Type already set.",
        tests: "Library - Service.CreateServiceOrderHeaderUsingPage (TestPage Service Order OpenNew, Customer No. Activate, OK): the part Service Order Subform links on \"Document No.\" (field 1), its table relation of Document No. filters on \"Document Type\" (field 43); about 9 Tests-SCM-Service tests.",
        probe: "A part page with SourceTableView where(Type = const(B)) and SubPageLink on a field declared before Type whose table relation filters on Type, and an OnValidate on the first field that reads Type: open the host, read the draft line and what the OnValidate saw.",
    },
    DeleteAllMarkedOnly => {
        id: "delete-all-marked-only",
        assumed: "Record.DeleteAll with MarkedOnly(true) deletes the marked rows that pass the filters and no other row, like Find, Next, Count and ModifyAll work on the marked set; with triggers and events as a DeleteAll with a filter.",
        tests: "Post Inventory Cost to G/L (PostValueEntryToGL.MarkedOnly(true); DeleteAll() in OnPostDataItem, then CapValueEntryLoop Gets the capacity entries left): SCM Kitting D3 Part 2 and SCM Costing Batch, and every test that posts inventory cost with the report.",
        probe: "Mark two of three rows, MarkedOnly(true), DeleteAll: which rows remain; the same with a filter that excludes a marked row, and DeleteAll(true) with an OnDelete trigger.",
    },
    DateFormulaEvaluatePercentPlaceholder => {
        id: "date-formula-evaluate-percent-placeholder",
        assumed: "Evaluate into a DateFormula reads `<+%1D>` as CalcDate of the same text does (one day: misc/lt-protected-var-abs-query-views/P146FormatCalcDate.al pins CalcDate only) and stores the text as typed; what Format of that DateFormula prints is not assumed anywhere.",
        tests: "SCM Planning and SCM Planning NTF tests Initialize: Evaluate(DaysInMonthFormula, Format('<+%1D>', CalcDate('<1M>') - Today)); about 200 Tests-SCM tests.",
        probe: "Evaluate(F, '<+%1D>') for a DateFormula: the Boolean, Format(F) and CalcDate(F, D).",
    },
    CalcDateClosingBracketMissingAfterPlus => {
        id: "calcdate-closing-bracket-missing-after-plus",
        assumed: "CalcDate of a formula that starts with `<+` and has no closing bracket (`<+6W`) reads it as the same formula with the bracket. (P146FormatCalcDate.al pins it for `<+%1D` only.)",
        tests: "Tests-SCM tests that build a formula with Format of a text and a length: `<+6W` (13), `<+7W` (3), `<+1D` (2), `<+0D` (1).",
        probe: "CalcDate('<+6W', D), CalcDate('<+0D', D) and CalcDate('<-1D', D) with the closing bracket missing: the date or the error text.",
    },
    GroupedQueryTiesInGroupingOrder => {
        id: "grouped-query-ties-in-grouping-order",
        assumed: "Groups of an aggregated query that tie on every OrderBy column are read in the order of the query's grouping columns (column declaration order, each ascending).",
        tests: "Create Pick's CalcQtyInWhseEntries (OrderBy Block_Movement, ten grouping columns): SCM Extend Warehouse CheckItemAvailabilityWhenCreatingPickAndExistReservedQtyForProdOrder and the tests that create a pick for a directed put-away location; about 77 Tests-SCM tests.",
        probe: "An aggregated query with an OrderBy on a column most groups share and several grouping columns, rows inserted in non-key order: the order of the tied groups on Read.",
    },
    PageGetValidationErrorNoIndex => {
        id: "page-getvalidationerror-no-index",
        assumed: "TestPage.GetValidationError() without an index answers the first validation error of the page, and an empty text while the page has none.",
        tests: "SCM Item Tracking III (Job Journal, Purchase Order and Prod. Order Components pages: Assert.AreEqual('', Page.GetValidationError(), ...) and StrPos(Page.GetValidationError(), ItemTrackingExistErr) > 0); about 28 Tests-SCM tests.",
        probe: "A TestPage with no error: GetValidationError(); after one refused SetValue and after two refused SetValues on different controls: GetValidationError() next to GetValidationError(1) and (2).",
    },
    ReportItemCalcFieldsNestedOrList => {
        id: "report-dataitem-calcfields-nested-or-list",
        assumed: "The CalcFields property of a report data item that is nested under another item, or that lists several fields, calculates every listed FlowField of each row before the row's triggers, as for a top-level item with one field (pinned by misc/lt-probes-needed-8/PnH3Link.al).",
        tests: "Get Inbound Source Documents (CalcFields = \"Put-away Qty.\", \"Put-away Qty. (Base)\" on Posted Whse. Receipt Line): about 32 Tests-SCM tests.",
        probe: "A report with a nested item that has CalcFields = A, B (two FlowFields), and a reportextension-added item with CalcFields: the values in OnAfterGetRecord; one name that is not a FlowField.",
    },
    ReportItemFilterOnLinkFieldReplaced => {
        id: "report-item-filter-on-link-field-replaced",
        assumed: "A filter that is on a field of a report data item's DataItemLink before the run starts (the item's DataItemTableView, or a SetTableView of the caller) is in the report's filter group, where the link's range on that field takes its place for each parent row.",
        tests: "Get Source Documents and Whse.-Source - Create Document (the Assembly Header item: DataItemTableView where(\"Document Type\" = const(Order)) and a link on \"Document Type\"), and the Value Entry, Item Journal Line and Item Ledger Entry items of the cost reports; about 37 Tests-SCM tests.",
        probe: "A nested item with DataItemTableView = where(G = filter(1|2)) and DataItemLink = G = field(G) over parents with G 1 to 3: which children each parent shows; the same with the filter set by SetTableView.",
    },
    TableRelationSeveralWhereNonKey => {
        id: "table-relation-several-where-non-key",
        assumed: "A TableRelation check that fails, with several where() conditions none of which is on a primary key field of the related table, raises DB:NothingInsideFilter, as one condition on a non-key field does. (A mix with key fields stays refused.)",
        tests: "Item Blocked Test and Item Variant Blocked Test (Item where(Blocked = const(false), \"Service Blocked\" = const(false)) and the sales and purchase variants), SCM Dedicated Bins; about 26 Tests-SCM tests. They compare the message only.",
        probe: "A relation with two non-key where() conditions and a value that fails one of them, and one that is missing: GetLastErrorCode; the same with one key and one non-key condition.",
    },
    EnumFromIntegerVariantEnum => {
        id: "enum-from-integer-variant-enum",
        assumed: "Enum.FromInteger(V) with a Variant V that holds an Enum value takes the ordinal, as an assignment of that Variant to an Integer does (pinned by B_EnumOptionVariant.al:EnumVariant_To_Integer).",
        tests: "SCM Performance Tests ItemTrackingLinesPageHandler (\"Item Tracking Entry Type\".FromInteger(OptionValue), OptionValue dequeued from Library - Variable Storage after an enum was enqueued): 9 Tests-SCM tests.",
        probe: "V := EnumValue; E := \"Enum\".FromInteger(V): the ordinal or the error; the same with an Option in the Variant.",
    },
    ForeachIntegerIntoEnum => {
        id: "foreach-integer-into-enum",
        assumed: "A foreach over a List of Integer whose loop variable is an Enum gives the variable the enum value of each ordinal (an ordinal the enum does not declare is kept).",
        tests: "Email Account Impl.GetAllAccounts (`foreach Connector in Connector.Ordinals do`), which Job Planning Lines.OnInit reaches through Email Account.IsAnyAccountRegistered: SCM Fulfillment and other tests that open the Job Planning Lines page; 6 Tests-SCM tests.",
        probe: "foreach E in E.Ordinals with E an Enum variable: Format(E) and E.AsInteger() in the body; the same with a List of Integer that holds an ordinal the enum lacks.",
    },
    TestFilterSetFilterEmptyClears => {
        id: "testfilter-setfilter-empty-clears",
        assumed: "TestFilter.SetFilter(Field, '') (a text empty or of spaces) on a page or request page filter removes the user's filter on that field, as FieldRef.SetFilter does (pinned: P134DateTextFilters.al:P134_FieldRef_Date_EmptyAndBlankForms).",
        tests: "SCM Inventory Misc. II (Inventory Pick and Put-away \"WithAdditionalFilters\"), SCM Invt Item Tracking III, SCM Plan-Req. Wksht, SCM Inventory Journals, SCM Inventory Miscellaneous: about 17 Tests-SCM tests that clear a request page filter with an empty text.",
        probe: "TestPage.Filter.SetFilter(F, '') and (F, ' ') after a filter on F: GetFilter(F) and the rows; the same on a request page's data item filter.",
    },
    PageRunRecordViewOrderApplies => {
        id: "page-run-record-view-order-applies",
        assumed: "Page.Run with a record on a page whose SourceTableView names an ORDER(ascending|descending): the page runs in the view's direction, as the view alone does. (P145PageRunView.al pins the view's SORTING and the record's direction when the view names none.)",
        tests: "Pages with SourceTableView = sorting(...) order(descending), such as Item Ledger Entries, opened with a record: SCM Item Journal and SCM Warehouse tests; about 11 Tests-SCM tests.",
        probe: "Page.Run(Page, Rec) with Rec ascending and the page's view order(descending), and the reverse: the first row the page shows.",
    },
    ListGoneOnlyRowWithoutUpdate => {
        id: "list-gone-only-row-without-update",
        assumed: "A list whose only row an action deleted, when the action did not call CurrPage.Update, shows its blank line and raises OnAfterGetCurrRecord three times, as it does after an action that called CurrPage.Update (pageupdate/TestPageUpdateGoneRow.al pins that case).",
        tests: "Planning worksheet and requisition tests whose action deletes the only line (SCM Plan-Req. Wksht CalcPlan... from a blanket order): about 11 Tests-SCM tests.",
        probe: "The Gone List fixture of TestPageUpdateGoneRow.al with an action that deletes the only row and does not call CurrPage.Update: the trace of OnAfterGetCurrRecord calls.",
    },
    ReportRenderDiscarded => {
        id: "report-render-discarded",
        assumed: "A rendering save (SaveAsPdf, SaveAsExcel, SaveAsWord, SaveAsHtml, SaveAs to Pdf, Excel, Word or Html) of a report on the allow-list (ADR-040) runs the report as the Xml path does, answers true and writes no output; AL that looks at the output is refused.",
        tests: "Post Inventory Cost to G/L through Library - Costing and Library - Assembly PostInvtCostToGL (SCM Kitting D3 Part 1 and 2, SCM WIP Costing Production I and II, SCM Costing Batch); Check.SaveAsPdf (ERM Cash Manager, Match Bank Rec. Scenarios).",
        probe: "A report on the list saved to a temp file and to a stream on a Windows tier: the answer, the order of the triggers, File.Exists and TempBlob.Length afterwards.",
    },
    ReportProcedureFilterUnderRequestPage => {
        id: "report-procedure-filter-under-request-page",
        assumed: "A filter a procedure of the report set on a data item before a run with a request page is not in that run: the page does not show it (pinned) and the filters of the page replace it when the page closes with OK (assumed).",
        tests: "Microsoft tests that call a report procedure which filters a data item and then RunModal the report with its request page.",
        probe: "A report procedure RowDi.SetFilter(Code, 'B*'), RunModal with a handler that clicks OK and a report that counts the rows in OnPreDataItem and OnAfterGetRecord: are the B rows alone read?",
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
        assume(Assumption::NextWholeDecimal);
        assert_eq!(
            take_hits(),
            vec![Assumption::ClearAllKeepsTemporaryRows, Assumption::NextWholeDecimal]
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
