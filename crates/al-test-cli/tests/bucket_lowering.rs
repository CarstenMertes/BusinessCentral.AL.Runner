//! Lowering gaps found by running Microsoft's test buckets (branch bucket-lowering). The
//! fixtures are small AL written for this file; each test names the corpus test that pins the
//! behaviour it relies on.

use al_sema::workspace::{AppKind, AppSource};
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

fn outcomes(main: &str) -> Vec<(String, Outcome)> {
    let symbols = al_version::corpus_symbols(27);
    let (_, l) = al_sema::lower::compile_apps_with(
        vec![AppSource {
            id: "main".into(),
            name: "main".into(),
            dependencies: Vec::new(),
            files: vec![("main.al".into(), main.into())],
        }],
        vec![AppKind::Workspace],
        &symbols,
        None,
        al_sema::check::Roots::default(),
    );
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let engine = Engine::new(l.program.clone(), blocked);
    let oi = l
        .program
        .objects
        .iter()
        .position(|o| o.name == "T")
        .expect("test codeunit T");
    run_codeunit(&engine, &l, oi)
}

fn passes(o: &[(String, Outcome)], test: &str) {
    match o.iter().find(|(n, _)| n == test).map(|(_, o)| o) {
        Some(Outcome::Pass) => {}
        other => panic!("{test} was expected to pass, got {other:?}"),
    }
}

fn not_run(o: &[(String, Outcome)], test: &str) -> String {
    match o.iter().find(|(n, _)| n == test).map(|(_, o)| o) {
        Some(Outcome::NotRun(why)) => why.clone(),
        other => panic!("{test} was expected to be refused, got {other:?}"),
    }
}

/// `EI.Message := 'x'` is the property form of `EI.Message('x')`; the storage and the getter are
/// pinned through the call form (errorinfo/TestErrorInfoType.al:249-271 Message and
/// DetailedMessage, :284-307 Verbosity, :313-345 CustomDimensions, errorinfo/
/// TestErrorInfoErrorType.al:25-45 ErrorType).
#[test]
fn error_info_members_assign_as_properties() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Texts() var EI: ErrorInfo; begin
    EI := ErrorInfo.Create();
    EI.Message := 'MSG-1'; EI.DetailedMessage := 'DETAIL-3'; EI.Title := 'TTL';
    if EI.Message() <> 'MSG-1' then Error('message %1', EI.Message());
    if EI.DetailedMessage() <> 'DETAIL-3' then Error('detailed');
    if EI.Title() <> 'TTL' then Error('title');
    if EI.Message <> 'MSG-1' then Error('getter'); end;
  [Test] procedure Options() var EI: ErrorInfo; begin
    EI := ErrorInfo.Create();
    EI.Verbosity := Verbosity::Warning; EI.ErrorType := ErrorType::Client; EI.Collectible := false;
    if Format(EI.Verbosity()) <> 'Warning' then Error('verbosity');
    if EI.Collectible() then Error('collectible'); end;
  [Test] procedure Numbers() var EI: ErrorInfo; begin
    EI := ErrorInfo.Create();
    EI.FieldNo := 7; EI.PageNo := 9; EI.TableId := 3;
    if (EI.FieldNo() <> 7) or (EI.PageNo() <> 9) or (EI.TableId() <> 3) then Error('numbers'); end; }"#,
    );
    for t in ["Texts", "Options", "Numbers"] {
        passes(&o, t);
    }
}

const MARKS: &str = r#"table 50001 "Thing" { fields { field(1; "No."; Integer) { } } keys { key(PK; "No.") { Clustered = true; } }
  procedure GroupInTableCode(): Integer begin FilterGroup := 2; exit(FilterGroup); end;
  procedure MarkedInTableCode(): Boolean begin MarkedOnly := true; exit(MarkedOnly); end; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure MarkAndMarkedOnly() var R: Record "Thing"; begin
    R.DeleteAll(); R."No." := 1; R.Insert(); R."No." := 2; R.Insert();
    R.FindFirst(); R.Mark := true;
    if not R.Mark then Error('mark getter');
    R.MarkedOnly := true;
    if R.Count() <> 1 then Error('marked only count %1', R.Count());
    if not R.MarkedOnly then Error('marked only getter'); end;
  [Test] procedure AscendingProperty() var R: Record "Thing"; begin
    R.DeleteAll(); R."No." := 1; R.Insert(); R."No." := 2; R.Insert();
    R.Ascending := false;
    R.FindFirst(); if R."No." <> 2 then Error('descending first %1', R."No.");
    if R.Ascending then Error('ascending getter'); end;
  [Test] procedure ImplicitRecInTableCode() var R: Record "Thing"; begin
    if R.GroupInTableCode() <> 2 then Error('group');
    if not R.MarkedInTableCode() then Error('marked'); end;
  [Test] procedure WorkDateAssignment() var Old: Date; begin
    Old := WorkDate(); WorkDate := 20240101D;
    if WorkDate() <> 20240101D then Error('workdate %1', WorkDate()); WorkDate := Old; end; }"#;

/// `Rec.Mark := b`, `Rec.MarkedOnly := b`, `Rec.Ascending := b` and `WorkDate := d` are the
/// property forms of the calls the corpus pins (record/TestRecordFind.al:237-284 for Mark and
/// MarkedOnly, handlers/TestPartOwnSortVsSubPageView.al:187 for Ascending, session/
/// TestSessionFunctions.al:63-65 for WorkDate). Unqualified in table code they act on the
/// implicit Rec.
#[test]
fn record_properties_and_work_date_assign() {
    let o = outcomes(MARKS);
    for t in ["MarkAndMarkedOnly", "AscendingProperty", "ImplicitRecInTableCode", "WorkDateAssignment"] {
        passes(&o, t);
    }
}

/// Nothing in the corpus sets `ErrorInfo.RecordId` (only `Create(.., Record)` fills it,
/// errorinfo/TestErrorInfoType.al:360-377), and whether the assignment also sets `TableId` is not
/// pinned. A routine that does not reach the assignment runs; one that does is refused there.
#[test]
fn assigning_error_info_record_id_is_refused_only_where_it_runs() {
    let o = outcomes(
        r#"codeunit 50001 "Helper" {
  procedure Fill(var EI: ErrorInfo; Rid: RecordId; Go: Boolean) begin if Go then EI.RecordId := Rid; end; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure NotReached() var H: Codeunit "Helper"; EI: ErrorInfo; Rid: RecordId; begin
    EI := ErrorInfo.Create(); H.Fill(EI, Rid, false); end;
  [Test] procedure Reached() var H: Codeunit "Helper"; EI: ErrorInfo; Rid: RecordId; begin
    EI := ErrorInfo.Create(); H.Fill(EI, Rid, true); end; }"#,
    );
    passes(&o, "NotReached");
    assert!(not_run(&o, "Reached").contains("RecordId"));
}

/// Unqualified in a dataitem trigger, `FilterGroup := g` and `MarkedOnly := b` act on the
/// dataitem's record, as the unqualified call forms do. The properties are the ones pinned above;
/// the dataitem record in triggers is pinned by handlers/RgfFilterReport.Report.al:38 (`Rows`).
#[test]
fn dataitem_record_properties_assign_unqualified_in_report_triggers() {
    let o = outcomes(
        r#"table 50002 "Row" { fields { field(1; "No."; Integer) { } } keys { key(PK; "No.") { Clustered = true; } } }
table 50003 "Log" { fields { field(1; "Key"; Integer) { } field(2; Seen; Integer) { } } keys { key(PK; "Key") { Clustered = true; } } }
report 50004 "Rep" { ProcessingOnly = true;
  dataset { dataitem(Rows; "Row") {
    trigger OnPreDataItem() var L: Record "Log"; begin
      FilterGroup := 2; MarkedOnly := true;
      L.DeleteAll(); L."Key" := 1; L.Seen := FilterGroup; L.Insert(); end; } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Unqualified() var Rep: Report "Rep"; L: Record "Log"; begin
    Rep.UseRequestPage(false); Rep.RunModal();
    L.Get(1); if L.Seen <> 2 then Error('group %1', L.Seen); end; }"#,
    );
    passes(&o, "Unqualified");
}

/// Microsoft's tests write `Page.FindFirstField(Name, Value)` with the control named bare. The
/// corpus only qualifies the control (`ListPage."Integer Field"`, handlers/
/// TestBCReportHandlerContracts.al:247), so the bare form is refused with a reason that says so,
/// not with "`Name` is not assignable".
#[test]
fn a_bare_control_name_as_a_testfield_argument_is_refused_with_its_own_reason() {
    let o = outcomes(
        r#"table 50002 "Row" { fields { field(1; "No."; Integer) { } } keys { key(PK; "No.") { Clustered = true; } } }
page 50010 "Rows" { PageType = List; SourceTable = "Row"; layout { area(Content) { repeater(R) { field("No."; Rec."No.") { } } } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Bare() var TP: TestPage "Rows"; begin TP.OpenView(); if TP.FindFirstField("No.", 1) then; end; }"#,
    );
    let why = not_run(&o, "Bare");
    assert!(why.contains("TestField") && !why.contains("not assignable"), "{why}");
}

/// Microsoft's code compares `ClientType::ODataV4` (Price List Lines, Calc. Running GL. Acc.
/// Balance). The member is a name of the platform option; its position is not pinned
/// (session/TestSessionExtended.al:21,31 only calls the Session functions), so nothing here may
/// depend on it.
#[test]
fn the_odata_v4_client_type_member_exists_and_differs_from_the_others() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Names() var C: ClientType; begin
    C := ClientType::ODataV4;
    if C = ClientType::OData then Error('same as OData');
    if C <> ClientType::ODataV4 then Error('not itself'); end; }"#,
    );
    passes(&o, "Names");
}
