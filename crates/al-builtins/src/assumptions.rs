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
        assumed: "CurrPage.<Part>.Page in the page's own code reaches a part whose Visible came out false (and its page is opened).",
        tests: "Cash Receipt Journal OnAfterGetCurrRecord (WorkflowStatusBatch, Visible = variable); 7 Tests-General Journal tests.",
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
