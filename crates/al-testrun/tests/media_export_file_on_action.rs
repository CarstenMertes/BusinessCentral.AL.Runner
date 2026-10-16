//! `Media.ExportFile` is a client-side download that no corpus test pins (corpus
//! streams/lt-stream-read-write-media/P102Media.al:5 skips it as OnPrem or not existing). A page action that calls
//! it must not refuse the tests that open the page without running the action: Microsoft's "Fixed Asset Picture"
//! (page 5620) calls `Rec.Image.ExportFile` in OnAction, and ~65 tests of Tests-Fixed Asset open the page. Running
//! the action is refused when it runs.

use al_sema::workspace::AppSource;
use al_testrun::runtime::{CorpusRuntime, RunOptions, SuiteRun};
use al_testrun::{CaseId, Outcome};

const SRC: &str = r#"
table 50100 "ME Row" { fields { field(1; Id; Integer) { } field(2; Pic; Media) { } } keys { key(PK; Id) { Clustered = true; } } }
page 50101 "ME Card"
{
    SourceTable = "ME Row";
    layout { area(Content) { field(Id; Rec.Id) { } } }
    actions { area(Processing) { action(Export) { trigger OnAction() begin Rec.Pic.ExportFile('x.png'); end; } } }
}
codeunit 50102 "T"
{
    Subtype = Test;
    [Test]
    procedure OpenWithoutAction()
    var P: TestPage "ME Card";
    begin
        P.OpenView();
        P.Close();
    end;
    [Test]
    procedure RunTheAction()
    var P: TestPage "ME Card";
    begin
        P.OpenView();
        P.Export.Invoke();
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
        codeunit: 50102,
        name: name.into(),
    })
}

#[test]
fn opening_the_page_without_the_action_runs() {
    assert_eq!(outcome("OpenWithoutAction"), Outcome::Pass);
}

#[test]
fn running_the_action_is_refused_when_it_runs() {
    match outcome("RunTheAction") {
        Outcome::NotRun(why) => assert!(why.contains("Media.ExportFile"), "{why}"),
        o => panic!("{o:?}"),
    }
}
