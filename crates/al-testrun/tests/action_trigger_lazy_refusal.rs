//! A page action's OnAction that cannot be lowered is refused when the action runs, not before: Base Application's Item Card
//! pages hold actions (Xmlport.Run, Media.ExportFile) that the Tests-SCM tests opening the card never invoke (81 tests). A
//! test that invokes the action is not run (reported with the reason), as before.

use al_sema::workspace::AppSource;
use al_testrun::runtime::{CorpusRuntime, RunOptions, SuiteRun};
use al_testrun::{CaseId, Outcome};

const SRC: &str = r#"
page 50430 "LAZ Page" { PageType = Card;
    layout { area(Content) { field(F; Flag) { } } }
    actions { area(Processing) { action(Boom) { trigger OnAction() begin CodeCoverageLoad(); end; } } }
    var Flag: Boolean; }
codeunit 50430 "T"
{
    Subtype = Test;
    [Test]
    procedure OpeningThePageRuns()
    var P: TestPage "LAZ Page";
    begin
        P.OpenView();
        P.Close();
    end;
    [Test]
    procedure InvokingTheActionIsRefused()
    var P: TestPage "LAZ Page";
    begin
        P.OpenView();
        P.Boom.Invoke();
    end;
}
"#;

#[test]
fn an_unlowerable_action_trigger_is_refused_only_when_invoked() {
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
    let o = rt.first_outcome(&CaseId {
        codeunit: 50430,
        name: "OpeningThePageRuns".into(),
    });
    assert_eq!(o, Outcome::Pass, "{o:?}");
    let o = rt.first_outcome(&CaseId {
        codeunit: 50430,
        name: "InvokingTheActionIsRefused".into(),
    });
    assert!(matches!(o, Outcome::NotRun(_)), "{o:?}");
}
