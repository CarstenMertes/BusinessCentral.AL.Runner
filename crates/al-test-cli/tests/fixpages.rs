//! Page, part, handler and report contracts pinned by the probe tests of the corpus branch
//! `al-runtime-rs`. AL source -> compile -> run as a test codeunit. Each test names the probe it
//! mirrors; the probes marked "(pending push)" are local to the corpus-probes checkout.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

const OBJECTS: &str = r#"
table 64000 "FP Row"
{
    fields { field(1; "No."; Integer) { } field(2; Link; Integer) { } field(3; Txt; Text[30]) { } }
    keys { key(PK; "No.") { } }
}
codeunit 64001 "FP Log"
{
    SingleInstance = true;
    var L: Text;
    procedure Add(E: Text) begin L += E + ';'; end;
    procedure Take(): Text var R: Text; begin R := L; L := ''; exit(R); end;
}
page 64010 "FP Part"
{
    PageType = ListPart; SourceTable = "FP Row";
    layout { area(Content) { repeater(R) { field(NoCtl; Rec."No.") { } field(TxtCtl; Rec.Txt) { } } } }
    var Log: Codeunit "FP Log";
    trigger OnInit() begin Log.Add('part.OnInit'); end;
    trigger OnOpenPage() begin Log.Add('part.OnOpenPage'); end;
    trigger OnQueryClosePage(CloseAction: Action): Boolean begin Log.Add('part.Query(' + Format(CloseAction) + ')'); exit(true); end;
    trigger OnClosePage() begin Log.Add('part.Close'); end;
}
page 64011 "FP Host"
{
    PageType = Card; SourceTable = "FP Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { }
        part(Lines; "FP Part") { SubPageLink = Link = field("No."); }
        part(ReadOnly; "FP Part") { Editable = false; SubPageLink = Link = field("No."); } } }
    var Log: Codeunit "FP Log";
    trigger OnInit() begin Log.Add('host.OnInit'); end;
    trigger OnOpenPage() begin Log.Add('host.OnOpenPage'); end;
    trigger OnQueryClosePage(CloseAction: Action): Boolean begin Log.Add('host.Query'); exit(true); end;
    trigger OnClosePage() begin Log.Add('host.Close'); end;
}
page 64012 "FP Dialog"
{
    PageType = StandardDialog;
}
page 64013 "FP Hidden Host"
{
    PageType = Card; SourceTable = "FP Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } part(Hidden; "FP Part") { Visible = false; } } }
}
report 64020 "FP Report"
{
    ProcessingOnly = true; UseRequestPage = false;
    dataset
    {
        dataitem(Parent; "FP Row")
        {
            DataItemTableView = sorting("No.") where(Link = const(0));
            dataitem(Child; "FP Row")
            {
                DataItemLink = Link = field("No.");
                trigger OnAfterGetRecord() begin Log.Add('child(' + Format(Child."No.") + ',' + Child.GetFilters() + ')'); end;
            }
            trigger OnAfterGetRecord()
            begin
                Log.Add('parent(' + Format(Parent."No.") + ')');
                if (Mode = 'skip') and (Parent."No." = 1) then CurrReport.Skip();
                if (Mode = 'break') and (Parent."No." = 2) then CurrReport.Break();
                if (Mode = 'quit') and (Parent."No." = 2) then CurrReport.Quit();
            end;
            trigger OnPostDataItem() begin Log.Add('post-item'); end;
        }
    }
    var Log: Codeunit "FP Log"; Mode: Text;
    procedure SetMode(M: Text) begin Mode := M; end;
    trigger OnPreReport() begin Parent.SetRange("No.", 2); Log.Add('pre'); end;
    trigger OnPostReport() begin Log.Add('post-report'); end;
}
"#;

fn outcomes(tests: &str) -> Vec<(String, Outcome)> {
    let src = format!("{OBJECTS}\ncodeunit 64100 T {{ Subtype = Test; {tests} }}");
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
    for (n, r) in &outcomes(tests) {
        assert_eq!(r, &Outcome::Pass, "{n}");
    }
}

const HELPERS: &str = "local procedure Check(Want: Text; Got: Text) begin if Want <> Got then Error('want [%1] got [%2]', Want, Got); end;
local procedure Seed() var R: Record \"FP Row\"; Log: Codeunit \"FP Log\"; begin R.DeleteAll();
  R.\"No.\" := 1; R.Insert(); R.\"No.\" := 2; R.Insert(); R.\"No.\" := 3; R.Insert();
  R.\"No.\" := 11; R.Link := 1; R.Insert(); R.\"No.\" := 12; R.Link := 1; R.Insert(); R.\"No.\" := 31; R.Link := 3; R.Insert();
  Commit(); Log.Take(); end;";

// handlers/TestUnhandledUiAndDialogDefaults.al:68-107 (one space, MissingUIHandler, page ids)
#[test]
fn unhandled_ui_text_and_code() {
    passes(&format!(
        "{HELPERS}
        [Test] procedure Texts() var D: Page \"FP Dialog\"; begin
          asserterror Message('hello %1', 5);
          Check('Unhandled UI: Message hello 5', GetLastErrorText()); Check('MissingUIHandler', GetLastErrorCode());
          asserterror Page.RunModal(Page::\"FP Dialog\");
          Check('Unhandled UI: ModalPage 64012', GetLastErrorText());
          asserterror Page.Run(Page::\"FP Dialog\");
          Check('Unhandled UI: Page 64012', GetLastErrorText()); end;
        [Test] [HandlerFunctions('Boom')] procedure InHandler() begin
          asserterror Message('x'); Check('boom', GetLastErrorText()); Check('TestWrapped:Dialog', GetLastErrorCode()); end;
        [MessageHandler] procedure Boom(M: Text) begin Error('boom'); end;"
    ));
}

// testpart/TestPartTriggerOrderAndRefresh.al:236-247 and the close order of
// testpart/TestPartCloseLinkEditableContracts.al (pending push)
#[test]
fn part_init_and_close_order() {
    passes(&format!(
        "{HELPERS}
        [Test] procedure Order() var H: TestPage \"FP Host\"; Log: Codeunit \"FP Log\"; begin Seed();
          H.OpenEdit(); Check('host.OnInit;part.OnInit;part.OnInit;host.OnOpenPage;part.OnOpenPage;part.OnOpenPage;', Log.Take());
          H.Close();
          Check('part.Query(None);part.Query(None);host.Query;part.Close;part.Close;host.Close;', Log.Take()); end;"
    ));
}

// testpart/TestPartTriggerOrderAndRefresh.al:316-340 and handlers/TestPageRuntimeContracts.al:245-278
#[test]
fn not_found_ids_and_codes() {
    passes(&format!(
        "{HELPERS}
        [Test] procedure Ids() var H: TestPage \"FP Hidden Host\"; B: Boolean; begin Seed(); H.OpenEdit();
          asserterror B := H.Hidden.Visible();
          Check('The part with ID = 1732295561 was not found on the page.', GetLastErrorText());
          Check('TestPartNotFound', GetLastErrorCode());
          asserterror H.GetField(99999).Value();
          Check('The field with ID = 99999 is not found on the page.', GetLastErrorText());
          Check('TestFieldNotFound', GetLastErrorCode());
          asserterror H.GetValidationError(0);
          Check('Index out of bounds.', GetLastErrorText()); Check('IndexOutOfBounds', GetLastErrorCode()); end;"
    ));
}

// handlers/TestReportRuntimeContracts.al:322-390 (Skip, Break, Quit, discarded OnPreReport filter,
// DataItemLink hidden from GetFilters)
#[test]
fn report_skip_break_quit_and_filters() {
    passes(&format!(
        "{HELPERS}
        [Test] procedure Flow() var Rep: Report \"FP Report\"; Log: Codeunit \"FP Log\"; begin Seed();
          Rep.Run();
          Check('pre;parent(1);child(11,);child(12,);parent(2);parent(3);child(31,);post-item;post-report;', Log.Take());
          Clear(Rep); Rep.SetMode('skip'); Rep.Run();
          Check('pre;parent(1);parent(2);parent(3);child(31,);post-item;post-report;', Log.Take());
          Clear(Rep); Rep.SetMode('break'); Rep.Run();
          Check('pre;parent(1);child(11,);child(12,);parent(2);post-item;post-report;', Log.Take());
          Clear(Rep); Rep.SetMode('quit'); Rep.Run();
          Check('pre;parent(1);child(11,);child(12,);parent(2);', Log.Take()); end;"
    ));
}

// corpus branch al-runtime-rs (pending push): handlers/TestPageModalResultsAndTriggersContracts.al:
// RunModal_Dialog_StaticVersusVariable_ReportedAction
#[test]
fn static_run_modal_reports_the_lookup_actions() {
    passes(&format!(
        "{HELPERS}
        [Test] [HandlerFunctions('Ok')] procedure Static() var R: Action; begin
          R := Page.RunModal(Page::\"FP Dialog\"); Check('LookupOK', Format(R)); end;
        [ModalPageHandler] procedure Ok(var D: TestPage \"FP Dialog\") begin D.OK().Invoke(); end;"
    ));
}

// corpus branch al-runtime-rs (pending push): testpart/TestPartCloseLinkEditableContracts.al:
// Parts_EnabledReadsTheControl_EditableFalseDoesNotBlockWrites
#[test]
fn editable_false_part_still_takes_writes() {
    passes(&format!(
        "{HELPERS}
        [Test] procedure Write() var H: TestPage \"FP Host\"; R: Record \"FP Row\"; begin Seed();
          H.OpenEdit(); H.GoToKey(1); H.ReadOnly.First(); H.ReadOnly.TxtCtl.SetValue('ro'); H.ReadOnly.Next();
          R.Get(11); Check('ro', R.Txt); H.Close(); end;"
    ));
}
