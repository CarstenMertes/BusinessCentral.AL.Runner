//! Smaller gaps of the headless test client (ADR-012 follow-up): procedures on a `Page`
//! variable, `Clear` of a TestPage, and further. AL source → compile → run as a test codeunit.
//! Each test names the corpus test it mirrors.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

const OBJECTS: &str = r#"
table 62000 "PM Row"
{
    fields { field(1; "No."; Code[20]) { } field(2; Descr; Text[50]) { } field(3; Rank; Integer) { } }
    keys { key(PK; "No.") { } }
}
codeunit 62002 "PM Probe"
{
    SingleInstance = true;
    var Trail: Text;
    procedure Reset() begin Trail := ''; end;
    procedure Note(T: Text) begin Trail += T; end;
    procedure Trace(): Text begin exit(Trail); end;
}
page 62010 "PM Wizard"
{
    PageType = Card;
    layout { area(Content) { field(StepCtl; Step) { } } }
    var Step: Integer;
    trigger OnInit() var P: Codeunit "PM Probe"; begin P.Note('I'); Step := 0; end;
    trigger OnOpenPage() var P: Codeunit "PM Probe"; begin P.Note('O'); end;
    procedure SetStep(S: Integer) var P: Codeunit "PM Probe"; begin P.Note('S'); Step := S; end;
    procedure GetStep(): Integer begin exit(Step); end;
}
page 62011 "PM Persist"
{
    PageType = Worksheet;
    layout { area(Content) { field(Dummy; Dummy) { } } }
    var Target: Record "PM Row"; Dummy: Text[10];
    trigger OnQueryClosePage(CloseAction: Action): Boolean
    begin
        if (CloseAction <> CloseAction::OK) and (CloseAction <> CloseAction::LookupOK) then exit(true);
        Target.Rank := 0; Target.Modify(); exit(true);
    end;
    procedure SetTarget(var Row: Record "PM Row") begin Target := Row; end;
}
page 62012 "PM Card"
{
    PageType = Card; SourceTable = "PM Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } field(DescrCtl; Rec.Descr) { } } }
}
"#;

fn outcomes(tests: &str) -> Vec<(String, Outcome)> {
    let src = format!("{OBJECTS}\ncodeunit 62100 T {{ Subtype = Test; {tests} }}");
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src)],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let engine = Engine::new(l.program.clone(), blocked);
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    run_codeunit(&engine, &l, oi)
}

fn passes(tests: &str) {
    let o = outcomes(tests);
    assert!(!o.is_empty());
    for (n, r) in &o {
        assert_eq!(r, &Outcome::Pass, "{n}");
    }
}

const HELPERS: &str = "procedure Check(Expected: Text; Actual: Text) begin if Expected <> Actual then Error('expected <%1> got <%2>', Expected, Actual); end;";

// handlers/TestPageOnInit_Tests.al:188 POI_SetterBeforeRunModal_RunsAfterOnInitAndSurvives
#[test]
fn a_procedure_on_a_page_variable_runs_after_on_init_and_before_the_open() {
    passes(&format!(
        "{HELPERS}
        [Test] [HandlerFunctions('WizardHandler')]
        procedure SetterBeforeRunModal() var W: Page \"PM Wizard\"; P: Codeunit \"PM Probe\"; begin
            P.Reset(); W.SetStep(2); W.RunModal();
            Check('ISO', P.Trace()); end;
        [ModalPageHandler] procedure WizardHandler(var T: TestPage \"PM Wizard\") begin Check('2', T.StepCtl.Value()); end;"
    ));
}

// handlers/TestPageOnInit_Tests.al:206 POI_SetterBeforeRun_RunsAfterOnInitAndSurvives
#[test]
fn the_non_modal_run_of_a_page_variable_keeps_what_the_setter_chose() {
    passes(&format!(
        "{HELPERS}
        [Test] [HandlerFunctions('WizardHandler')]
        procedure SetterBeforeRun() var W: Page \"PM Wizard\"; P: Codeunit \"PM Probe\"; begin
            P.Reset(); W.SetStep(1); W.Run();
            Check('ISO', P.Trace()); end;
        [PageHandler] procedure WizardHandler(var T: TestPage \"PM Wizard\") begin Check('1', T.StepCtl.Value()); end;"
    ));
}

// handlers/TestPageOnInit_Tests.al:223 POI_SetterBeforeTrappedRun_RunsAfterOnInitAndSurvives
#[test]
fn a_trapped_run_receives_the_page_variables_instance() {
    passes(&format!(
        "{HELPERS}
        [Test]
        procedure SetterBeforeTrappedRun() var W: Page \"PM Wizard\"; T: TestPage \"PM Wizard\"; P: Codeunit \"PM Probe\"; begin
            P.Reset(); T.Trap(); W.SetStep(1); W.Run();
            Check('ISO', P.Trace()); Check('1', T.StepCtl.Value()); T.Close(); end;"
    ));
}

// handlers/TestPageOnInit_Tests.al GetSelectedKindOrdinal_WithoutRunModal_ReadsTheDeclaredDefault (handlers/TestPageEnumGlobal*)
#[test]
fn a_getter_on_a_page_variable_that_never_ran_reads_the_declared_default() {
    passes(&format!(
        "{HELPERS}
        [Test]
        procedure GetterWithoutRun() var W: Page \"PM Wizard\"; P: Codeunit \"PM Probe\"; begin
            P.Reset(); if W.GetStep() <> 0 then Error('step'); Check('I', P.Trace()); end;"
    ));
}

// handlers/TestPageModalQueryClose_Tests.al:186 PersistInQueryClosePage_HandlerOk_WriteReachesTheTable
#[test]
fn a_page_variable_handed_a_record_writes_it_back_in_on_query_close_page() {
    passes(&format!(
        "{HELPERS}
        [Test] [HandlerFunctions('OkHandler')]
        procedure PersistOk() var R: Record \"PM Row\"; M: Page \"PM Persist\"; begin
            R.DeleteAll(); R.\"No.\" := 'F'; R.Rank := 999; R.Insert();
            M.SetTarget(R); M.LookupMode(true); M.RunModal();
            R.Get('F'); if R.Rank <> 0 then Error('rank %1', R.Rank); end;
        [ModalPageHandler] procedure OkHandler(var T: TestPage \"PM Persist\") begin T.OK().Invoke(); end;"
    ));
}

// handlers/TestPageEditable_Tests.al:93 EditableBoundToPageVariable_FollowsTheRow
#[test]
fn clear_of_a_closed_test_page_gives_a_fresh_variable() {
    passes(&format!(
        "{HELPERS}
        [Test]
        procedure ClearClosed() var R: Record \"PM Row\"; C: TestPage \"PM Card\"; begin
            R.DeleteAll(); R.\"No.\" := 'A'; R.Descr := 'x'; R.Insert();
            C.OpenView(); Check('A', C.NoCtl.Value()); C.Close();
            Clear(C);
            C.OpenView(); Check('A', C.NoCtl.Value()); C.Close(); end;"
    ));
}
