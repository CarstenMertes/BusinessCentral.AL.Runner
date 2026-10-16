//! A report run with a write pending is refused with "An error occurred and the transaction is stopped." when AL code
//! runs it (corpus record/TestTxModelReportRun.al:117-148). What a TestPage action that runs a report does with the
//! test's pending write is not pinned: Microsoft's Tests-Fixed Asset (Fixed Assets Plan-based E2E.DepreciateFixedAsset)
//! invokes the Fixed Asset List's Calculate Depreciation action after writes and passes on real BC. The run is refused
//! instead of raising the AL-level error.

use al_sema::workspace::AppSource;
use al_testrun::runtime::{CorpusRuntime, RunOptions, SuiteRun};
use al_testrun::{CaseId, Outcome};

const SRC: &str = r#"
table 50100 "PA Row" { fields { field(1; Code; Code[10]) { } } keys { key(PK; Code) { Clustered = true; } } }
report 50101 "PA Rpt" { ProcessingOnly = true; dataset { dataitem(Rows; "PA Row") { } } requestpage { layout { area(Content) { field(Opt; Opt) { } } } } var Opt: Integer; }
page 50102 "PA Card"
{
    PageType = Card; SourceTable = "PA Row";
    layout { area(Content) { field(Code; Rec.Code) { } } }
    actions { area(Processing) { action(Go) { trigger OnAction() begin Report.RunModal(Report::"PA Rpt", true, false, Rec); end; } } }
}
codeunit 50103 "T"
{
    Subtype = Test;
    [RequestPageHandler]
    procedure H(var RP: TestRequestPage "PA Rpt") begin RP.Cancel().Invoke(); end;
    [Test]
    [HandlerFunctions('H')]
    procedure ActionAfterAWrite()
    var R: Record "PA Row"; P: TestPage "PA Card";
    begin
        R.Code := 'A'; R.Insert();
        P.OpenView();
        P.Go.Invoke();
    end;
}
"#;

#[test]
fn a_page_action_that_runs_a_report_after_a_write_is_refused() {
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
    match rt.first_outcome(&CaseId {
        codeunit: 50103,
        name: "ActionAfterAWrite".into(),
    }) {
        Outcome::NotRun(why) => assert!(why.contains("TestPage action"), "{why}"),
        o => panic!("{o:?}"),
    }
}
