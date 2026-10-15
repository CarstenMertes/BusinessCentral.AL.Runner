//! A call of a control add-in procedure (`CurrPage.BarcodeControl.RequestBarcodeAsync()`) is refused when it runs,
//! not before the test (issue 34). Base Application's "Item Tracking Lines" calls it only when the add-in said it
//! is supported (`ControlAddInReady`, a client event); a test that only opens the page never reaches it.
//! Control add-ins are not modelled, and no corpus test calls one.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

const SRC: &str = r#"
controladdin "CA Scanner" { event ControlAddInReady(IsSupported: Boolean); procedure RequestBarcodeAsync(); }
table 63500 "CA Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
page 63501 "CA Lines"
{
    PageType = List; SourceTable = "CA Row";
    layout { area(Content) {
        usercontrol(BarcodeControl; "CA Scanner") { trigger ControlAddInReady(IsSupported: Boolean) begin Available := IsSupported; end; }
        repeater(Rows) { field(Id; Rec.Id) { } } } }
    actions { area(Processing) { action(Scan) { trigger OnAction() begin if Available then CurrPage.BarcodeControl.RequestBarcodeAsync(); end; }
                                 action(ScanNow) { trigger OnAction() begin CurrPage.BarcodeControl.RequestBarcodeAsync(); end; } } }
    var Available: Boolean;
}
codeunit 63502 T
{
    Subtype = Test;
    [Test] [HandlerFunctions('Lines')] procedure OpensThePage() begin Page.RunModal(Page::"CA Lines"); end;
    [Test] procedure ScanWithoutTheAddIn() var P: TestPage "CA Lines"; begin P.OpenView(); P.Scan.Invoke(); P.Close(); end;
    [Test] procedure ScanNow() var P: TestPage "CA Lines"; begin P.OpenView(); P.ScanNow.Invoke(); end;
    [ModalPageHandler] procedure Lines(var P: TestPage "CA Lines") begin P.Scan.Invoke(); end;
}
"#;

#[test]
fn a_control_add_in_call_refuses_only_when_it_runs() {
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), SRC.into())],
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
    let o = run_codeunit(&engine, &l, oi);
    let by = |n: &str| o.iter().find(|(t, _)| t == n).map(|(_, r)| r.clone()).unwrap();
    assert_eq!(by("OpensThePage"), Outcome::Pass);
    assert_eq!(by("ScanWithoutTheAddIn"), Outcome::Pass);
    match by("ScanNow") {
        Outcome::NotRun(why) => assert!(
            why.contains("control add-in") && why.contains("BarcodeControl"),
            "{why}"
        ),
        other => panic!("{other:?}"),
    }
}

// `CurrPage.Update(true)` on a page that has an action named Update is the page's method: an action is not called
// (System Application's "Schedule Feature Data Update" does it; real BC compiles it). Issue 34.
#[test]
fn curr_page_update_is_the_method_when_an_action_has_the_same_name() {
    let src = r#"
table 63510 "CU Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
page 63511 "CU Card"
{
    PageType = Card; SourceTable = "CU Row";
    layout { area(Content) { field(Id; Rec.Id) { } } }
    actions { area(Processing) { action(Update) { trigger OnAction() begin Done := true; CurrPage.Update(true); end; } } }
    var Done: Boolean;
}
codeunit 63512 T
{
    Subtype = Test;
    [Test] procedure UpdateAction() var R: Record "CU Row"; P: TestPage "CU Card"; begin R.Id := 1; R.Insert(); P.OpenEdit(); P.GoToRecord(R); P.Update.Invoke(); P.Close(); end;
}
"#;
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src.into())],
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
    let o = run_codeunit(&engine, &l, oi);
    assert_eq!(o[0].1, Outcome::Pass, "{:?}", o[0]);
}
