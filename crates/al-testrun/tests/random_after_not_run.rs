//! A failure of a test that draws random numbers after a test of the codeunit that did not run is not trusted: BC drew the
//! numbers of the test that did not run, so the sequence this test sees is not BC's (Tests-SCM-Planning
//! CalcPlanReqWkshForTransferLFLItem). A pass stays, and so does a failure of a test that draws nothing.

use al_sema::workspace::AppSource;
use al_testrun::runtime::{CorpusRuntime, RunOptions, SuiteRun};
use al_testrun::{CaseId, Outcome};

const SRC: &str = r#"
report 50301 "R" { ProcessingOnly = true; }
codeunit 50300 "T"
{
    Subtype = Test;
    [Test]
    [HandlerFunctions('H')]
    procedure A_RefusedFirst()
    begin
    end;
    [ReportHandler]
    procedure H(var R: Report "R")
    begin
    end;
    [Test]
    procedure B_DrawsAndFails()
    begin
        if Random(10) > 0 then Error('boom');
    end;
    [Test]
    procedure C_FailsWithoutDrawing()
    begin
        Error('boom');
    end;
    [Test]
    procedure D_DrawsAndPasses()
    begin
        if Random(10) = 0 then Error('never');
    end;
}
"#;

fn run() -> CorpusRuntime {
    CorpusRuntime::run_suites_with(
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
    )
}

#[test]
fn a_failure_after_a_test_that_did_not_run_and_a_random_draw_is_not_trusted() {
    let rt = run();
    let id = |n: &str| CaseId {
        codeunit: 50300,
        name: n.into(),
    };
    assert!(matches!(rt.first_outcome(&id("A_RefusedFirst")), Outcome::NotRun(_)));
    match rt.first_outcome(&id("B_DrawsAndFails")) {
        Outcome::NotRun(m) => assert!(m.contains("draws random numbers"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        rt.first_outcome(&id("C_FailsWithoutDrawing")),
        Outcome::Fail(_)
    ));
    assert_eq!(rt.first_outcome(&id("D_DrawsAndPasses")), Outcome::Pass);
}
