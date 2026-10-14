//! Page and report extension code that names the base object's members, end to end (ADR-023).
//!
//! The probes of ADR-023 have not all run on real BC, so the lowering refuses these accesses by
//! default. This test binary turns every case on with `AL_UNPINNED_BASE_ACCESS` to exercise the
//! mechanism (checker scope, `Place::BaseGlobal`, `Expr::BaseInstance`, the instance link). The
//! expected values are the language reference's, not BC's, until a probe pins them; each case
//! names its probe. `base_access_refused.rs` checks the default (everything refused).

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const BASE: &str = r#"
table 53000 "BA Row"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[30]) { } field(3; Qty; Integer) { } }
    keys { key(PK; "No.") { } }
}

codeunit 53100 "BA Trace"
{
    SingleInstance = true;
    var Log: Text;
    procedure Add(T: Text) begin Log += T; end;
    procedure Take(): Text var R: Text; begin R := Log; Log := ''; exit(R); end;
}

page 53001 "BA Card"
{
    PageType = Card;
    SourceTable = "BA Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } } }
    trigger OnOpenPage()
    begin
        Counter := 5;
        Note := 'base';
        ItemRec.Get('B');
        Names.Add('n1');
    end;
    trigger OnAfterGetRecord()
    var T: Codeunit "BA Trace";
    begin
        T.Add('base-agr:' + Format(Counter) + ':' + Note + ':' + ItemRec.GetFilter("No.") + ':' + Format(Names.Count()) + ';');
    end;
    protected procedure Bump(By: Integer): Integer begin Counter += By; exit(Counter); end;
    procedure Twice(var V: Integer) begin V := V * 2; end;
    local procedure Hidden() begin end;
    protected var
        Counter: Integer;
        Note: Text;
        ItemRec: Record "BA Row";
        Names: List of [Text];
        Greeting: Label 'hello';
    var
        Private: Integer;
}
"#;

const EXT: &str = r#"
pageextension 53001 "BA Card Ext" extends "BA Card"
{
    layout
    {
        addlast(Content)
        {
            field(ItemName; ItemRec.Name) { }
            field(CounterText; Format(Counter) + '/' + Greeting) { }
        }
    }
    trigger OnOpenPage()
    var T: Codeunit "BA Trace"; V: Integer; R: Integer;
    begin
        T.Add('ext-open:' + Format(Counter) + ':' + Note + ';');
        Counter := 99;
        Note := Note + '+ext';
        ItemRec.SetRange("No.", 'A');
        Names.Add('n2');
        V := 3;
        Twice(V);
        R := Bump(4);
        T.Add('ext-proc:' + Format(V) + ':' + Format(R) + ';');
    end;
}
"#;

fn compile(src: String) -> (al_sema::workspace::Workspace, al_sema::lower::Lowered) {
    // SAFETY: every test of this binary sets the same value before the first compile.
    unsafe { std::env::set_var("AL_UNPINNED_BASE_ACCESS", "1") };
    let apps = vec![AppSource {
        id: "app0".into(),
        name: "app0".into(),
        dependencies: vec![],
        files: vec![("f.al".into(), src)],
    }];
    al_sema::lower::compile(apps, &Default::default())
}

fn run(extra: &str, body: &str) -> Result<Value, Signal> {
    let src = format!("{BASE}\n{extra}\ncodeunit 53200 C {{ {body} }}");
    let (ws, l) = compile(src);
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == "F")
        .unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

/// Every refusal reason of the objects named `name`, for a source that must not run.
fn reasons(extra: &str) -> Vec<String> {
    let (_, l) = compile(format!("{BASE}\n{extra}"));
    l.unsupported.iter().flatten().flatten().cloned().collect()
}

const OPEN: &str =
    "procedure F(): Text var R: Record \"BA Row\"; P: TestPage \"BA Card\"; T: Codeunit \"BA Trace\"; X: Text; begin
        R.\"No.\" := 'A'; R.Name := 'Alpha'; R.Insert();
        R.\"No.\" := 'B'; R.Name := 'Bravo'; R.Insert();
        R.Get('A');
        P.OpenView();
        X := T.Take();";

// ADR-023 probe P1: an extension trigger reads what the base's OnOpenPage set (Integer, Text).
// Probe P7: the base OnOpenPage runs before the extension's.
#[test]
fn an_extension_reads_variables_the_base_set() {
    let r = run(EXT, &format!("{OPEN} P.Close(); exit(X); end;"));
    assert!(text(r).starts_with("ext-open:5:base;"));
}

// Probe P2: the extension's writes are what the base reads next.
// Probe P5: a Record and a List variable are shared with the base.
// Probe P4: a base procedure with a var parameter, and one that changes a base variable.
#[test]
fn an_extension_write_reaches_the_base_and_base_procedures_run_on_the_base_instance() {
    let r = run(EXT, &format!("{OPEN} P.Close(); exit(X); end;"));
    // 99 written by the extension, then Bump(4) on the base's Counter.
    assert_eq!(text(r), "ext-open:5:base;ext-proc:6:103;base-agr:103:base+ext:A:2;");
}

// Probe P9: control sources that name a base variable.
#[test]
fn control_source_expressions_read_the_base_variables() {
    let r = run(
        EXT,
        &format!("{OPEN} X := P.ItemName.Value() + '|' + P.CounterText.Value(); P.Close(); exit(X); end;"),
    );
    assert_eq!(text(r), "Bravo|103/hello");
}

// Probe P3: two extensions of one page share the base's variables.
#[test]
fn two_extensions_see_the_same_base_variables() {
    let second = r#"
pageextension 53002 "BA Card Ext 2" extends "BA Card"
{
    trigger OnOpenPage() var T: Codeunit "BA Trace"; begin T.Add('ext2:' + Format(Counter) + ';'); Counter := 1000; end;
}
pageextension 53000 "BA Card Ext 0" extends "BA Card"
{
    trigger OnOpenPage() var T: Codeunit "BA Trace"; begin T.Add('ext0:' + Format(Counter) + ';'); Counter := 7; end;
}"#;
    let r = run(second, &format!("{OPEN} P.Close(); exit(X); end;"));
    // The base reads what the last extension wrote.
    assert_eq!(text(r), "ext0:5;ext2:7;base-agr:1000:base::1;");
}

// Probes R1 (protected var and protected procedure), R3 (see the refused binary for the rest).
#[test]
fn a_report_extension_shares_the_protected_variables_and_procedures_of_its_base_report() {
    let rep = r#"
report 53010 "BA Report"
{
    ProcessingOnly = true;
    UseRequestPage = false;
    dataset { dataitem(Rows; "BA Row") { } }
    trigger OnPreReport() begin Stage := 'base-pre'; Count := 1; end;
    trigger OnPostReport() var T: Codeunit "BA Trace"; begin T.Add('base-post:' + Stage + ':' + Format(Count) + ';'); end;
    protected procedure Twice(X: Integer): Integer begin exit(X * 2); end;
    protected var Stage: Text; Count: Integer;
}
reportextension 53011 "BA Report Ext" extends "BA Report"
{
    trigger OnPreReport()
    var T: Codeunit "BA Trace";
    begin
        T.Add('ext-pre:' + Stage + ';');
        Stage := Stage + '+ext';
        Count := Twice(Count) + 40;
    end;
}"#;
    let r = run(
        rep,
        "procedure F(): Text var T: Codeunit \"BA Trace\"; begin Report.Run(Report::\"BA Report\", false); exit(T.Take()); end;",
    );
    assert_eq!(text(r), "ext-pre:base-pre;base-post:base-pre+ext:42;");
}

// Probe P6: a name declared by the extension and by its base is refused, not guessed.
#[test]
fn a_variable_declared_in_both_is_refused() {
    let ext = r#"
pageextension 53003 "BA Both" extends "BA Card"
{
    trigger OnOpenPage() begin Counter := 1; end;
    var Counter: Integer;
}"#;
    let r = reasons(ext);
    assert!(
        r.iter()
            .any(|x| x.contains("declared by the page extension and by the page")),
        "{r:?}"
    );
}

// Probe P8: a `local` procedure and a variable that is not `protected` cannot be named.
#[test]
fn local_and_unprotected_members_are_refused() {
    let ext = r#"
pageextension 53004 "BA Local" extends "BA Card"
{
    trigger OnOpenPage() begin Hidden(); end;
    trigger OnClosePage() begin Private := 1; end;
}"#;
    let r = reasons(ext);
    assert!(r.iter().filter(|x| x.contains("cannot name")).count() >= 2, "{r:?}");
}

// Probe R3: a Dialog, a Record and a List that the base report declares `protected`.
// Probe R2: a public procedure of the base report is not named by the extension's scope.
#[test]
fn a_report_extension_uses_a_protected_dialog_record_and_list_of_its_base() {
    let rep = r#"
report 53020 "BA Report 2"
{
    ProcessingOnly = true;
    UseRequestPage = false;
    dataset { dataitem(Rows; "BA Row") { } }
    trigger OnPostReport() var T: Codeunit "BA Trace"; begin T.Add('base-post:' + Format(Names.Count()) + ':' + Buf."No." + ';'); end;
    procedure PublicOne() begin end;
    protected var Window: Dialog; Buf: Record "BA Row"; Names: List of [Text];
}
reportextension 53021 "BA Report 2 Ext" extends "BA Report 2"
{
    trigger OnPreReport()
    begin
        Window.Open('Working #1');
        Window.Update(1, 5);
        Window.Close();
        Buf."No." := 'Z';
        Names.Add('a');
        Names.Add('b');
    end;
}"#;
    let r = run(
        rep,
        "procedure F(): Text var T: Codeunit \"BA Trace\"; begin Report.Run(Report::\"BA Report 2\", false); exit(T.Take()); end;",
    );
    assert_eq!(text(r), "base-post:2:Z;");
}

// Probe R2: a public procedure of the base report is not named by the extension's scope, so the
// call is refused (whether BC accepts it is the open question).
#[test]
fn a_public_procedure_of_the_base_report_is_refused() {
    let src = r#"
report 53030 "BA Report 3" { ProcessingOnly = true; dataset { dataitem(Rows; "BA Row") { } } procedure PublicOne() begin end; }
reportextension 53031 "BA Report 3 Ext" extends "BA Report 3" { trigger OnPreReport() begin PublicOne(); end; }"#;
    let r = reasons(src);
    assert!(
        r.iter()
            .any(|x| x.contains("`PublicOne` is a variable or procedure of the report it extends")),
        "{r:?}"
    );
}

// ADR-023, refusals: a [TryFunction] or an event publisher of the base is not called from the extension.
#[test]
fn a_try_function_or_an_event_of_the_base_page_is_refused() {
    let src = r#"
page 53040 "BA Page 4" { PageType = Card; SourceTable = "BA Row";
    [TryFunction] procedure Tries() begin end;
    [IntegrationEvent(false, false)] procedure OnSomething() begin end;
}
pageextension 53041 "BA Page 4 Ext" extends "BA Page 4" { trigger OnOpenPage() begin Tries(); OnSomething(); end; }"#;
    let r = reasons(src);
    assert!(
        r.iter().any(|x| x.contains("[TryFunction] procedure of the page")),
        "{r:?}"
    );
    assert!(
        r.iter().any(|x| x.contains("IntegrationEvent] procedure of the page")),
        "{r:?}"
    );
}

/// ADR-023 item 4: base and extension live in a dependency app, which is compiled on demand. The
/// reference to a base procedure must reach the base routine, and the base page stays compiled
/// with its extension.
#[test]
fn a_base_reference_in_a_dependency_app_is_reached_by_the_demand_driven_compile() {
    // SAFETY: every test of this binary sets the same value before the first compile.
    unsafe { std::env::set_var("AL_UNPINNED_BASE_ACCESS", "1") };
    let dep = AppSource {
        id: "dep".into(),
        name: "dep".into(),
        dependencies: vec![],
        files: vec![("dep.al".into(), format!("{BASE}\n{EXT}"))],
    };
    let test_app = AppSource {
        id: "tests".into(),
        name: "tests".into(),
        dependencies: vec!["dep".into()],
        files: vec![(
            "c.al".into(),
            format!("codeunit 53200 C {{ {OPEN} P.Close(); exit(X); end; }}"),
        )],
    };
    use al_sema::workspace::AppKind;
    let (ws, l) = al_sema::lower::compile_apps_with(
        vec![dep, test_app],
        vec![AppKind::Dependency, AppKind::Workspace],
        &Default::default(),
        None,
        Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == "F")
        .unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    let r = s.call(oi, ri, inst, vec![]).map(|(v, _)| v);
    assert_eq!(text(r), "ext-open:5:base;ext-proc:6:103;base-agr:103:base+ext:A:2;");
}

// Probe P2: a base variable as a `var` argument of a builtin, a compound assignment, an array element.
#[test]
fn base_variables_work_as_var_arguments_compound_targets_and_array_elements() {
    let page = r#"
page 53050 "BA Page 5" { PageType = Card; SourceTable = "BA Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } } }
    trigger OnAfterGetRecord() var T: Codeunit "BA Trace"; begin T.Add('base:' + Format(Counter) + ':' + Note + ':' + Format(Arr[2]) + ';'); end;
    protected var Counter: Integer; Note: Text; Arr: array[3] of Integer;
}
pageextension 53051 "BA Page 5 Ext" extends "BA Page 5"
{
    trigger OnOpenPage()
    begin
        Evaluate(Counter, '12');
        Counter += 3;
        Note := 'abc';
        Clear(Note);
        Arr[2] := 8;
        Arr[2] += 1;
    end;
}
codeunit 53201 "BA Run 5"
{
    procedure F(): Text var R: Record "BA Row"; P: TestPage "BA Page 5"; T: Codeunit "BA Trace";
    begin R."No." := 'A'; R.Insert(); P.OpenView(); P.Close(); exit(T.Take()); end;
}"#;
    let src = format!(
        "{BASE}\n{page}\ncodeunit 53200 C {{ procedure F(): Text var R: Codeunit \"BA Run 5\"; begin exit(R.F()); end; }}"
    );
    let (ws, l) = compile(src);
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == "F")
        .unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    let r = s.call(oi, ri, inst, vec![]).map(|(v, _)| v);
    assert_eq!(text(r), "base:15::9;");
}
