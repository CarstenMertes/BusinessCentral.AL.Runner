//! DrillDown on a test page control with no OnDrillDown trigger raises "The NavDrilldownAction method is not supported."
//! for a plain field (corpus handlers/TestPageFieldDrillDown_Tests.al:140-146). A FlowField opens the page of the table
//! it sums over in BC (Tests-Fixed Asset ERM Fixed Assets.FALedgerEntriesUsingStatistics drills down "Book Value" of the
//! Fixed Asset Statistics page); no corpus test pins that, so the run is refused instead of raising the plain error.

use al_sema::workspace::AppSource;
use al_testrun::runtime::{CorpusRuntime, RunOptions, SuiteRun};
use al_testrun::{CaseId, Outcome};

const SRC: &str = r#"
table 50100 "DF Row" { fields { field(1; Code; Code[10]) { } field(2; Plain; Integer) { } field(3; Total; Decimal) { FieldClass = FlowField; CalcFormula = sum("DF Line".Amount where("Head Code" = field(Code))); } } keys { key(PK; Code) { Clustered = true; } } }
table 50101 "DF Line" { fields { field(1; "Entry No."; Integer) { } field(2; "Head Code"; Code[10]) { } field(3; Amount; Decimal) { } } keys { key(PK; "Entry No.") { Clustered = true; } } }
page 50102 "DF Card" { PageType = Card; SourceTable = "DF Row"; layout { area(Content) { field(Code; Rec.Code) { } field(Plain; Rec.Plain) { } field(Total; Rec.Total) { } } } }
codeunit 50103 "T"
{
    Subtype = Test;
    [Test]
    procedure PlainFieldDrillDownRaises()
    var R: Record "DF Row"; P: TestPage "DF Card";
    begin
        R.Code := 'A'; R.Insert();
        P.OpenView();
        asserterror P.Plain.DrillDown();
        if GetLastErrorText() <> 'The NavDrilldownAction method is not supported.' then Error('%1', GetLastErrorText());
    end;
    [Test]
    procedure FlowFieldDrillDownIsRefused()
    var R: Record "DF Row"; P: TestPage "DF Card";
    begin
        R.Code := 'A'; R.Insert();
        P.OpenView();
        P.Total.DrillDown();
    end;
}
"#;

fn outcome(name: &str) -> Outcome {
    let rt = CorpusRuntime::run_suites_with(
        vec![AppSource {
            id: "main".into(),
            name: "main".into(),
            dependencies: vec![],
            files: vec![("main.al".into(), SRC.into())],
        }],
        vec![al_sema::AppKind::Workspace],
        &[],
        &Default::default(),
        None,
        Default::default(),
        &[SuiteRun::everything()],
        &RunOptions::default(),
    );
    rt.first_outcome(&CaseId {
        codeunit: 50103,
        name: name.into(),
    })
}

#[test]
fn a_plain_field_keeps_the_pinned_error() {
    assert_eq!(outcome("PlainFieldDrillDownRaises"), Outcome::Pass);
}

#[test]
fn a_flowfield_is_refused() {
    match outcome("FlowFieldDrillDownIsRefused") {
        Outcome::NotRun(why) => assert!(why.contains("FlowField"), "{why}"),
        o => panic!("{o:?}"),
    }
}
