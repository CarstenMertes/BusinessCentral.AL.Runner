//! The page test client end to end: AL source → compile → run (ADR-012).
//! Each test names the corpus test it mirrors.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const OBJECTS: &str = r#"
table 50000 "PG Row"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Qty; Integer) { MinValue = 0; MaxValue = 10; trigger OnValidate() begin Trace.Add('t-qty;'); end; }
        field(3; Descr; Text[50]) { }
    }
    var
        Trace: Codeunit "PG Trace";
    keys { key(PK; "No.") { } }
}

page 50001 "PG Card"
{
    PageType = Card;
    SourceTable = "PG Row";
    layout
    {
        area(Content)
        {
            field(NoCtl; Rec."No.") { }
            field(QtyCtl; Rec.Qty) { }
            field(DescrCtl; Rec.Descr) { }
        }
    }
}

page 50003 "PG Format"
{
    PageType = Card;
    SourceTable = "PG Row";
    layout
    {
        area(Content)
        {
            field(NoCtl; Rec."No.") { }
            field(Fmt; Rec.Qty) { AutoFormatType = 10; AutoFormatExpression = Pattern(); }
            field(Hide; Rec.Qty) { Visible = Shown; Editable = Shown; Enabled = Shown; }
            field(Named; Rec.Descr) { Caption = 'Shiny'; }
            field(Glob; GlobN) { }
        }
    }
    var
        Shown: Boolean;
        GlobN: Integer;
        Trace: Codeunit "PG Trace";
    trigger OnOpenPage() begin Shown := true; Trace.Add('open;'); end;
    trigger OnAfterGetRecord() begin Trace.Add('agr:' + Rec."No." + ';'); GlobN := Rec.Qty * 2; end;
    trigger OnAfterGetCurrRecord() begin Trace.Add('agcr;'); end;
    local procedure Pattern(): Text
    begin
        Trace.Add('fmt;');
        if Rec."No." = 'BOOM' then Error('boom for %1', Rec."No.");
        exit('<Precision,3:3><Standard Format,0>');
    end;
    actions
    {
        area(Processing)
        {
            action(Flip) { trigger OnAction() begin Shown := not Shown; end; }
        }
    }
}

// corpus handlers/TestPageOnInit_Objects.al: OnInit appends 'I', a procedure 'S'.
page 50999 "PG Wizard"
{
    PageType = Card;
    SourceTable = "PG Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } } }
    var
        Trace: Text;
        Step: Integer;
    trigger OnInit() begin Trace += 'I'; Step := 0; end;
    trigger OnOpenPage() begin Trace += 'O'; end;
    procedure SetStep(NewStep: Integer) begin Trace += 'S'; Step := NewStep; end;
    procedure GetTrace(): Text begin exit(Trace + ':' + Format(Step)); end;
}

page 50997 "PG EditAssign"
{
    PageType = Card;
    SourceTable = "PG Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } } }
    trigger OnOpenPage() begin CurrPage.Editable := false; end;
}

page 50996 "PG EditCall"
{
    PageType = Card;
    SourceTable = "PG Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } } }
    trigger OnOpenPage() begin CurrPage.Editable(false); end;
}

page 50005 "PG Maker"
{
    PageType = List;
    SourceTable = "PG Row";
    layout { area(Content) { repeater(Lines) { field(NoCtl; Rec."No.") { } field(QtyCtl; Rec.Qty) { } } } }
    actions
    {
        area(Processing)
        {
            action(InsertFind) { trigger OnAction() begin Rec.Init(); Rec."No." := 'M'; Rec.Qty := 4; Rec.Insert(); Rec.FindFirst(); end; }
            action(FieldsOnly) { trigger OnAction() begin Rec.Qty := 9; end; }
        }
    }
}

page 50006 "PG Edit"
{
    PageType = Card;
    SourceTable = "PG Row";
    layout
    {
        area(Content)
        {
            field(NoCtl; Rec."No.") { }
            field(QtyCtl; Rec.Qty) { trigger OnValidate() begin Trace.Add('p-qty;'); end; }
            field(DescrCtl; Rec.Descr) { }
            field(Guard; GuardVar) { trigger OnValidate() begin if GuardVar = 'REFUSE' then Error('guard refused'); end; }
        }
    }
    var
        GuardVar: Text;
        Trace: Codeunit "PG Trace";
    trigger OnNewRecord(BelowxRec: Boolean) begin Trace.Add('new:' + Format(BelowxRec) + ',x=' + xRec."No." + ';'); Rec.Descr := 'seeded'; end;
    trigger OnInsertRecord(BelowxRec: Boolean): Boolean begin Trace.Add('ins:' + Rec."No." + '/' + Rec.Descr + ';'); exit(true); end;
    trigger OnModifyRecord(): Boolean begin Trace.Add('mod:' + xRec.Descr + '>' + Rec.Descr + ';'); exit(true); end;
    trigger OnClosePage() begin Trace.Add('close;'); end;
    trigger OnQueryClosePage(CloseAction: Action): Boolean begin Trace.Add('qcp:' + Format(CloseAction) + ';'); exit(not Veto); end;
    var Veto: Boolean;
}

page 50007 "PG NoInsert"
{
    PageType = Card;
    SourceTable = "PG Row";
    InsertAllowed = false;
    layout
    {
        area(Content)
        {
            field(NoCtl; Rec."No.") { }
            group(Hidden)
            {
                Visible = false;
                field(InHidden; Rec.Qty) { }
            }
            field(OwnHidden; Rec.Descr) { Visible = false; }
        }
    }
    actions { area(Processing) { action(Quiet) { Visible = false; trigger OnAction() begin Rec.Descr := 'ran'; Rec.Modify(); end; } } }
}

page 50008 "PG DelayedList"
{
    PageType = List;
    SourceTable = "PG Row";
    DelayedInsert = true;
    layout { area(Content) { repeater(Lines) { field(NoCtl; Rec."No.") { } field(DescrCtl; Rec.Descr) { } } } }
}

page 50004 "PG FormatList"
{
    PageType = List;
    SourceTable = "PG Row";
    layout
    {
        area(Content)
        {
            repeater(Lines)
            {
                field(NoCtl; Rec."No.") { }
                field(Fmt; Rec.Qty) { AutoFormatType = 10; AutoFormatExpression = Pattern(); }
            }
        }
    }
    local procedure Pattern(): Text
    begin
        if Rec.Qty < 0 then Error('boom for %1', Rec."No.");
        exit('<Precision,3:3><Standard Format,0>');
    end;
}

codeunit 50102 "PG Evt Sub"
{
    SingleInstance = true;
    EventSubscriberInstance = Manual;
    [EventSubscriber(ObjectType::Page, Page::"PG Edit", 'OnOpenPageEvent', '', false, false)]
    local procedure OnOpen(var Rec: Record "PG Row") var T: Codeunit "PG Trace"; begin T.Add('evt-open;'); end;
    [EventSubscriber(ObjectType::Page, Page::"PG Edit", 'OnModifyRecordEvent', '', false, false)]
    local procedure OnMod(var Rec: Record "PG Row"; var xRec: Record "PG Row"; var AllowModify: Boolean) var T: Codeunit "PG Trace"; begin T.Add('evt-mod:' + xRec.Descr + '>' + Rec.Descr + ';'); AllowModify := false; end;
}

enum 50010 "PG Kind"
{
    value(0; Field) { Caption = 'Fields'; }
    value(1; Block) { Caption = 'Blocks'; }
    value(2; Plain) { }
}

page 50011 "PG Kind Page"
{
    PageType = Card;
    layout { area(Content) { field(KindCtl; Kind) { Caption = 'Kind'; } } }
    var Kind: Enum "PG Kind";
}

page 50012 "PG Hidden Get"
{
    PageType = List;
    SourceTable = "PG Row";
    SourceTableView = where("No." = const('ZZZ'));
    layout { area(Content) { repeater(Lines) { field(NoCtl; Rec."No.") { } field(QtyCtl; Rec.Qty) { } } } }
    actions { area(Processing) { action(GetRow) { trigger OnAction() begin Rec.Get('A'); end; } } }
}

codeunit 50101 "PG Trace"
{
    SingleInstance = true;
    var Log: Text;
    procedure Add(T: Text) begin Log += T; end;
    procedure Take(): Text var R: Text; begin R := Log; Log := ''; exit(R); end;
}

page 50002 "PG List"
{
    PageType = List;
    SourceTable = "PG Row";
    layout
    {
        area(Content)
        {
            repeater(Lines)
            {
                field(NoCtl; Rec."No.") { }
                field(QtyCtl; Rec.Qty) { }
            }
        }
    }
}
"#;

fn run(body: &str, routine: &str) -> Result<Value, Signal> {
    let src = format!("{OBJECTS}\ncodeunit 50100 C {{ {body} }}");
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
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![(oi, ri)];
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        if let Some(why) = l.unsupported[o][r].first() {
            panic!(
                "{}.{} is blocked: {why}",
                l.program.objects[o].name, l.program.objects[o].routines[r].name
            );
        }
        stack.extend(l.calls[o][r].iter().copied());
    }
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn error_text(r: Result<Value, Signal>) -> String {
    match r {
        Err(Signal::Error(e)) => e.message,
        other => panic!("expected an error, got {other:?}"),
    }
}

fn rows(n: &[(&str, i32)]) -> String {
    n.iter()
        .map(|(k, q)| format!("R.\"No.\" := '{k}'; R.Qty := {q}; R.Insert();"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

// handlers/TestPageHandler.al:17 Page_TestPage_Open_FindsFirstRecord: a page opened on a table
// shows the first row, and a control reads as the text of its field.
#[test]
fn open_shows_the_first_row_and_controls_read_as_text() {
    let r = run(
        "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Card\"; begin
            R.\"No.\" := 'B'; R.Qty := 7; R.Insert();
            R.\"No.\" := 'A'; R.Qty := 5; R.Insert();
            TP.OpenView();
            exit(TP.NoCtl.Value() + ':' + TP.QtyCtl.Value());
        end;",
        "F",
    );
    assert_eq!(text(r), "A:5");
}

// handlers/TestPageHandler.al:28-70 Page_TestPage_Next_AdvancesToSecond / _Last_ReturnsLastRecord
#[test]
fn first_next_last_previous_walk_the_rows_in_key_order() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Card\"; T: Text; begin
                {}
                TP.OpenView();
                T := TP.NoCtl.Value();
                if TP.Next() then T += TP.NoCtl.Value();
                if TP.Next() then T += TP.NoCtl.Value();
                T += Format(TP.Next());
                if TP.Last() then T += TP.NoCtl.Value();
                if TP.Previous() then T += TP.NoCtl.Value();
                if TP.First() then T += TP.NoCtl.Value();
                exit(T);
            end;",
            rows(&[("C", 3), ("A", 1), ("B", 2)])
        ),
        "F",
    );
    assert_eq!(text(r), "ABCNoCBA");
}

// handlers/TestPageExtended.al:17,34 Page_GoToKey_ExistingRecord_ReturnsTrue / _MissingRecord_ReturnsFalse;
// handlers/TestPageGoToRecordNonKeyRefresh_Tests.al:1-15: a missing key leaves the page where it was.
#[test]
fn go_to_key_finds_a_row_and_a_missing_key_leaves_the_cursor() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Card\"; T: Text; begin
                {}
                TP.OpenView();
                T := Format(TP.GoToKey('B')) + TP.NoCtl.Value() + TP.QtyCtl.Value();
                T += Format(TP.GoToKey('Z')) + TP.NoCtl.Value() + TP.QtyCtl.Value();
                exit(T);
            end;",
            rows(&[("A", 1), ("B", 2)])
        ),
        "F",
    );
    assert_eq!(text(r), "YesB2NoB2");
}

// testpageclose/TestPageClosingActions.al:19-24 and testpageteardown/TestPageTeardown.al:227,339:
// a variable that is not open answers "The TestPage is not open."; an open one cannot be opened.
#[test]
fn closed_and_open_variables_refuse_with_the_corpus_texts() {
    let body = |stmt: &str| {
        format!(
            "procedure F(): Text var TP: TestPage \"PG Card\"; T: Text; begin
                {stmt}
            end;"
        )
    };
    let e = error_text(run(&body("T := TP.NoCtl.Value();"), "F"));
    assert_eq!(e, "The TestPage is not open.");
    let e = error_text(run(&body("TP.Close();"), "F"));
    assert_eq!(e, "The TestPage is not open.");
    let e = error_text(run(&body("TP.OpenView(); TP.OpenView();"), "F"));
    assert_eq!(e, "The TestPage is already open.");
    let e = error_text(run(&body("TP.OpenView(); TP.Close(); T := TP.NoCtl.Value();"), "F"));
    assert_eq!(e, "The TestPage is not open.");
}

// autoformat/TestAutoFormatExpressionTiming.al:5-12: the control expressions run when a row is
// populated, after OnOpenPage and before OnAfterGetRecord, even with no row at all; an error
// fails the open and leaves the variable closed.
#[test]
fn control_expressions_run_between_on_open_page_and_on_after_get_record() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Format\"; T: Codeunit \"PG Trace\"; begin
                {}
                TP.OpenView();
                exit(T.Take());
            end;",
            rows(&[("A", 1)])
        ),
        "F",
    );
    assert_eq!(text(r), "open;fmt;agr:A;agcr;");
    let r = run(
        "procedure F(): Text var TP: TestPage \"PG Format\"; T: Codeunit \"PG Trace\"; begin
            TP.OpenView();
            exit(T.Take());
        end;",
        "F",
    );
    assert_eq!(
        text(r),
        "open;fmt;",
        "an empty view populates the expressions too, but no row"
    );
}

// autoformat/TestAutoFormatExpressionTiming.al: a failing expression at open raises its own
// error and leaves the page not open; on a card, a move onto the failing row reads "not open".
#[test]
fn a_failing_expression_fails_the_open_and_tears_a_card_down_on_a_move() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Format\"; begin
                {}
                TP.OpenView();
                TP.GoToKey('BOOM');
            end;",
            rows(&[("A", 1), ("BOOM", 2)])
        ),
        "F",
    );
    assert_eq!(error_text(r), "The TestPage is not open.");
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Format\"; begin
                {}
                TP.OpenView();
            end;",
            rows(&[("BOOM", 2)])
        ),
        "F",
    );
    assert_eq!(error_text(r), "boom for BOOM");
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Format\"; T: Text; begin
                {}
                TP.OpenView();
                asserterror TP.GoToKey('BOOM');
                T := GetLastErrorText();
                asserterror T += TP.NoCtl.Value();
                exit(T + '|' + GetLastErrorText());
            end;",
            rows(&[("A", 1), ("BOOM", 2)])
        ),
        "F",
    );
    assert_eq!(text(r), "The TestPage is not open.|The TestPage is not open.");
}

// handlers/TestPageControlPropertyLiveness_Tests.al:40-73: Visible is the open-time value,
// Editable and Enabled follow the page state.
#[test]
fn visible_is_frozen_at_open_editable_and_enabled_are_live() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Format\"; T: Text; begin
                {}
                TP.OpenView();
                T := Format(TP.Hide.Visible()) + Format(TP.Hide.Enabled());
                TP.Flip.Invoke();
                T += '|' + Format(TP.Hide.Visible()) + Format(TP.Hide.Enabled());
                exit(T);
            end;",
            rows(&[("A", 1)])
        ),
        "F",
    );
    assert_eq!(text(r), "YesYes|YesNo");
}

// autoformat/TestAutoFormat.al: an AutoFormatExpression with a precision sets the places;
// handlers/TestPageFieldFormats.Page.al: page-variable controls read the variable.
#[test]
fn controls_read_through_their_format_and_binding() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Format\"; begin
                {}
                TP.OpenView();
                exit(TP.Fmt.Value() + '|' + TP.Glob.Value() + '|' + TP.Named.Caption() + '|' + TP.NoCtl.Caption());
            end;",
            rows(&[("A", 1500)])
        ),
        "F",
    );
    assert_eq!(text(r), "1500|3000|Shiny|No.");
}

// handlers/TestPageNewRowLine_Tests.al:1-30 (EditableInsertable_NextPastLastDataRow_LandsOnTheNewRowLine and
// the OpenView gating arm): an editable list carries a blank line past its data; OpenView has none.
#[test]
fn an_editable_list_has_a_blank_new_row_line_past_its_data() {
    let f = |open: &str| {
        run(
            &format!(
                "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG List\"; T: Text; begin
                    {}
                    TP.{open}();
                    T := Format(TP.First()) + TP.NoCtl.Value();
                    T += Format(TP.Next()) + '[' + TP.NoCtl.Value() + ']';
                    T += Format(TP.Next());
                    exit(T);
                end;",
                rows(&[("A", 1)])
            ),
            "F",
        )
    };
    assert_eq!(text(f("OpenEdit")), "YesAYes[]No");
    assert_eq!(text(f("OpenView")), "YesANo[A]No");
}

// handlers/TestPageNewRowLine_Tests.al: First() on an empty editable list answers false.
#[test]
fn first_on_an_empty_editable_list_is_false() {
    let r = run(
        "procedure F(): Text var TP: TestPage \"PG List\"; begin
            TP.OpenEdit();
            exit(Format(TP.First()) + Format(TP.Last()));
        end;",
        "F",
    );
    assert_eq!(text(r), "NoNo");
}

// autoformat/TestAutoFormatExpressionTiming.al, RowFormat_List_FailingRow20Of120_RaisesAtOpen /
// _FailingRow30Of120_OpensAndReadsTheFirstRow / _LastOntoAFailingRowBeyondTheWindow_RaisesItsOwnError:
// a list populates a window of rows (the corpus pins it between 20 and 24) and raises the expression's
// own error on a move that populates a new window.
#[test]
fn a_list_populates_a_window_of_rows() {
    // PG Format has the BOOM key hard-wired, so the rows are named so that BOOM falls at the wanted position.
    let body = |boom_at: i32, then: &str| {
        format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG FormatList\"; i: Integer; begin
                for i := 1 to 120 do begin
                    R.\"No.\" := 'R' + Format(1000 + i); R.Qty := i; if i = {boom_at} then R.Qty := -1; R.Insert();
                end;
                TP.OpenView();
                {then}
            end;"
        )
    };
    assert_eq!(error_text(run(&body(20, "exit('');"), "F")), "boom for R1020");
    assert_eq!(text(run(&body(30, "exit(TP.Fmt.Value());"), "F")), "1");
    assert_eq!(error_text(run(&body(120, "TP.Last();"), "F")), "boom for R1120");
}

// testpart/BnrBlankPart.al:1-30 (NoRow_Action_StoredListInsertAndFind_ShowsTheRow / _TempListFieldsOnly_StaysBlank):
// controls read blank on a page that shows no row, until page code inserts a row and stands on it.
#[test]
fn a_page_with_no_row_reads_blank_until_page_code_inserts_and_positions_a_row() {
    let r = run(
        "procedure F(): Text var TP: TestPage \"PG Maker\"; T: Text; begin
            TP.OpenView();
            T := '[' + TP.QtyCtl.Value() + ']' + Format(TP.QtyCtl.AsInteger());
            TP.FieldsOnly.Invoke();
            T += '[' + TP.QtyCtl.Value() + ']';
            TP.InsertFind.Invoke();
            T += '[' + TP.QtyCtl.Value() + ']' + TP.NoCtl.Value();
            exit(T);
        end;",
        "F",
    );
    assert_eq!(text(r), "[]0[][4]M");
}

// handlers/TestPageReadOnlySetValue.al:253-330, TestPageBlankKeyInsert.al, TestPageSaveRecordInOnValidate_Tests.al:
// SetValue validates (table trigger, then the page control's), shows at once, and reaches the
// table when the page leaves the row (Close); writing back the same value modifies nothing.
#[test]
fn set_value_validates_and_the_row_is_saved_when_the_page_closes() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Edit\"; T: Codeunit \"PG Trace\"; X: Text; begin
                {}
                TP.OpenEdit();
                TP.QtyCtl.SetValue(5);
                X := TP.QtyCtl.Value();
                R.Get('A');
                X += '|' + Format(R.Qty);
                TP.Close();
                R.Get('A');
                exit(X + '|' + Format(R.Qty) + '|' + T.Take());
            end;",
            rows(&[("A", 1)])
        ),
        "F",
    );
    // table OnValidate before the page control's; OnModifyRecord sees xRec; OnClosePage after the save.
    assert_eq!(text(r), "5|1|5|t-qty;p-qty;mod:>;qcp:Cancel;close;");
}

// handlers/TestPageSameValueSetValue_Tests.al:1-30: writing back the value the field already holds
// does not modify the row.
#[test]
fn writing_the_same_value_back_does_not_modify() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Edit\"; T: Codeunit \"PG Trace\"; begin
                {}
                TP.OpenEdit();
                TP.DescrCtl.SetValue('');
                TP.Close();
                exit(T.Take());
            end;",
            rows(&[("A", 1)])
        ),
        "F",
    );
    assert_eq!(text(r), "qcp:Cancel;close;");
}

// handlers/TestPageFieldMinMaxValue_Tests.al:20-110: MinValue and MaxValue are enforced on a control write
// and a refused write does not reach the row.
#[test]
fn min_and_max_value_refuse_a_control_write() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Edit\"; X: Text; begin
                {}
                Commit();
                TP.OpenEdit();
                asserterror TP.QtyCtl.SetValue(11);
                X := GetLastErrorText();
                TP.Close();
                R.Get('A');
                exit(X + '|' + Format(R.Qty));
            end;",
            rows(&[("A", 1)])
        ),
        "F",
    );
    let t = text(r);
    assert!(t.contains("The value must be less than or equal to 10"), "{t}");
    assert!(t.ends_with("|1"), "{t}");
}

// handlers/TestPageValidationError_Tests.al:62-250: a refusing OnValidate records one validation error
// on the control and the page (with the refresh suffix for a Rec-bound control, none for a page
// variable) and raises the wrapper.
#[test]
fn a_refused_write_is_recorded_as_a_validation_error() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Edit\"; X: Text; begin
                {}
                TP.OpenEdit();
                asserterror TP.Guard.SetValue('REFUSE');
                X := GetLastErrorText() + '|' + Format(TP.Guard.ValidationErrorCount()) + '|' + TP.Guard.GetValidationError(1);
                X += '|' + Format(TP.ValidationErrorCount()) + '|' + TP.GetValidationError(1);
                exit(X);
            end;",
            rows(&[("A", 1)])
        ),
        "F",
    );
    assert_eq!(
        text(r),
        "Validation error for Field: Guard,  Message = 'guard refused'|1|guard refused|1|guard refused"
    );
}

// handlers/TestPageBooleanSpelling_Tests.al: SetValue('False') is not an acceptable Boolean text.
// (TextBool: a Text value written to an Integer control that cannot parse it.)
#[test]
fn a_text_that_does_not_parse_is_refused_with_the_acceptable_value_message() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Edit\"; begin
                {}
                TP.OpenEdit();
                TP.QtyCtl.SetValue('abc');
            end;",
            rows(&[("A", 1)])
        ),
        "F",
    );
    assert_eq!(
        error_text(r),
        "Validation error for Field: QtyCtl,  Message = 'Your entry of 'abc' is not an acceptable value for 'Qty'. (Select Refresh to discard errors)'"
    );
}

// handlers/TestPageRecordTriggers.al:32-130, TestPageCreateModeNewRecordArgs.al:243-270: OpenNew runs OnNewRecord
// (BelowxRec true, xRec the last row) before anything is typed; the key alone does not insert a
// Card row, a non-key control does (TestPageBlankKeyInsert.al:Card_TypedKey_ThenNonKeyWrite_InsertsBeforeClose).
#[test]
fn open_new_runs_on_new_record_and_a_non_key_write_inserts_the_row() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Edit\"; T: Codeunit \"PG Trace\"; X: Text; begin
                {}
                TP.OpenNew();
                TP.NoCtl.SetValue('N');
                X := Format(R.Count());
                TP.DescrCtl.SetValue('typed');
                X += Format(R.Count());
                TP.Close();
                R.Get('N');
                exit(X + '|' + R.Descr + '|' + T.Take());
            end;",
            rows(&[("A", 1)])
        ),
        "F",
    );
    assert_eq!(
        text(r),
        "12|typed|new:Yes,x=A;ins:N/seeded;mod:seeded>typed;qcp:Cancel;close;"
    );
}

// handlers/TestPageInsertAllowed_Tests.al:70 New_OnInsertAllowedFalsePage_IsDenied
#[test]
fn new_is_denied_on_a_page_that_does_not_allow_inserts() {
    let r = run(
        "procedure F() var TP: TestPage \"PG NoInsert\"; begin TP.OpenEdit(); TP.New(); end;",
        "F",
    );
    assert_eq!(error_text(r), "New method failed because Insert is not allowed.");
}

// handlers/TestPageFieldVisibleGroup_Tests.al:1-30 and TestPageActionReachability_Tests.al:100-122: a control
// whose Visible is the literal false (or inside such a group) is not on the page; an action is, and still runs.
#[test]
fn literal_false_visible_removes_a_control_but_not_an_action() {
    let body = |stmt: &str| {
        format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG NoInsert\"; X: Boolean; begin
                {} TP.OpenEdit(); {stmt}
            end;",
            rows(&[("A", 1)])
        )
    };
    let e = error_text(run(&body("X := TP.InHidden.Visible();"), "F"));
    assert!(e.ends_with("is not found on the page."), "{e}");
    let e = error_text(run(&body("X := TP.OwnHidden.Visible();"), "F"));
    assert!(e.ends_with("is not found on the page."), "{e}");
    let r = run(
        &body("exit(Format(TP.Quiet.Visible()) + Format(TP.Quiet.Enabled()));"),
        "F",
    );
    assert_eq!(text(r), "NoYes");
}

// handlers/TestPageFailedInsert_Tests.al:1-30: a started line whose insert fails raises nothing on New(),
// stays, and counts one validation error on its key control; Previous() moves on and Close() raises.
#[test]
fn a_started_line_whose_insert_fails_is_not_raised_on_new_but_on_close_after_previous() {
    let f = |mv: &str, close: &str| {
        run(
            &format!(
                "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG DelayedList\"; X: Text; begin
                    {}
                    TP.OpenNew();
                    TP.NoCtl.SetValue('A');
                    TP.DescrCtl.SetValue('typed');
                    {mv}
                    X := TP.NoCtl.Value() + Format(TP.NoCtl.ValidationErrorCount()) + Format(TP.DescrCtl.ValidationErrorCount());
                    {close}
                    exit(X);
                end;",
                rows(&[("A", 1)])
            ),
            "F",
        )
    };
    assert_eq!(text(f("TP.New();", "TP.Close();")), "A10");
    let e = error_text(f("TP.First();", "TP.Close();"));
    assert!(e.starts_with("The record in table PG Row already exists."), "{e}");
}

// testpageclose/TestPageClosingActions.al:Card_OK_RaisesTheCloseTriggersOnce / _Cancel_IsNotOffered / _OK_RefusedByOnQueryClosePage:
// OK saves, asks OnQueryClosePage(OK), then OnClosePage; a card offers no Cancel; Close() reports Cancel.
#[test]
fn ok_closes_with_the_query_close_page_then_the_close_trigger() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Edit\"; T: Codeunit \"PG Trace\"; X: Text; begin
                {}
                TP.OpenEdit();
                TP.DescrCtl.SetValue('saved');
                TP.OK().Invoke();
                R.Get('A');
                X := R.Descr + '|' + T.Take();
                asserterror TP.NoCtl.Value();
                X += GetLastErrorText();
                asserterror TP.Cancel();
                X += '|' + GetLastErrorText();
                TP.OpenEdit();
                asserterror TP.Cancel().Invoke();
                X += '|' + GetLastErrorText();
                TP.Close();
                exit(X + '|' + T.Take());
            end;",
            rows(&[("A", 1)])
        ),
        "F",
    );
    assert_eq!(
        text(r),
        "saved|mod:>saved;qcp:OK;close;The TestPage is not open.|The TestPage is not open.|The built-in action = Cancel is not found on the page.|qcp:Cancel;close;"
    );
}

// handlers/TestPageModeSwitchAction_Tests.al:Claim 1 and 2, TestPageReadOnlySetValue.al:365: the built-in Edit
// and View switch an open card's mode in place; a page-variable control stays editable on a read-only page.
#[test]
fn edit_and_view_switch_a_card_in_place() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Edit\"; X: Text; begin
                {}
                TP.OpenView();
                X := Format(TP.Editable()) + Format(TP.QtyCtl.Editable()) + Format(TP.Guard.Editable());
                X += Format(TP.Edit().Enabled()) + Format(TP.View().Enabled());
                TP.Edit().Invoke();
                X += '|' + Format(TP.Editable()) + Format(TP.QtyCtl.Editable()) + Format(TP.Edit().Enabled()) + Format(TP.View().Enabled());
                exit(X);
            end;",
            rows(&[("A", 1)])
        ),
        "F",
    );
    assert_eq!(text(r), "NoNoYesYesNo|YesYesNoYes");
}

// handlers/TestPageTriggerEvents.al:1-20, :230-300: the platform's page events run after the page's own trigger,
// and AllowModify := false vetoes the write.
#[test]
fn page_events_follow_the_triggers_and_can_veto_a_modify() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Edit\"; T: Codeunit \"PG Trace\"; E: Codeunit \"PG Evt Sub\"; X: Text; begin
                BindSubscription(E);
                {}
                TP.OpenEdit();
                TP.DescrCtl.SetValue('x');
                TP.Close();
                R.Get('A');
                exit(R.Descr + '|' + T.Take());
            end;",
            rows(&[("A", 1)])
        ),
        "F",
    );
    assert_eq!(text(r), "|evt-open;mod:>x;evt-mod:>x;qcp:Cancel;close;");
}

// handlers/TestPageEnumVariableControl_Tests.al:50-56,86-90: an Enum control resolves a SetValue text
// by the member's caption; the bare member name of a captioned value is refused.
#[test]
fn an_enum_control_takes_the_caption_and_refuses_the_member_name() {
    let body = |set: &str| {
        format!(
            "procedure F(): Text var TP: TestPage \"PG Kind Page\"; begin
                TP.OpenEdit();
                TP.KindCtl.SetValue('{set}');
                exit(TP.KindCtl.Value());
            end;"
        )
    };
    assert_eq!(text(run(&body("Blocks"), "F")), "Blocks");
    assert_eq!(
        error_text(run(&body("Block"), "F")),
        "Validation error for Field: KindCtl,  Message = 'Your entry of 'Block' is not an acceptable value for 'Kind'.'"
    );
}

// A value that declares no caption shows, and is set by, its name (types/TestEnumCaption.al).
#[test]
fn an_enum_value_without_a_caption_is_set_by_its_name() {
    let r = run(
        "procedure F(): Text var TP: TestPage \"PG Kind Page\"; begin
            TP.OpenEdit();
            TP.KindCtl.SetValue('Plain');
            exit(TP.KindCtl.Value());
        end;",
        "F",
    );
    assert_eq!(text(r), "Plain");
}

// testpart/BnrBlankPart.al NoRow_Action_GetOfARowTheFilterHides_StaysBlank: an action that Gets a row the
// page's filter hides, on a key field of the filter, leaves the page showing nothing.
#[test]
fn a_get_of_a_row_the_view_hides_shows_nothing() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Hidden Get\"; begin
                {}
                TP.OpenView();
                TP.GetRow.Invoke();
                exit('[' + TP.QtyCtl.Value() + '][' + TP.NoCtl.Value() + ']');
            end;",
            rows(&[("A", 5)])
        ),
        "F",
    );
    assert_eq!(text(r), "[][]");
}

// handlers/TestPageOnInit_Tests.al:POI_SetterBeforeRun_RunsAfterOnInitAndSurvives (:204-217): OnInit
// runs when the Page variable's instance is constructed, before the first procedure call on it.
#[test]
fn a_procedure_call_on_a_page_variable_runs_oninit_first_and_keeps_the_instance() {
    let r = run(
        "procedure F(): Text var W: Page \"PG Wizard\"; begin
            W.SetStep(2);
            W.SetStep(3);
            exit(W.GetTrace());
        end;",
        "F",
    );
    assert_eq!(text(r), "ISS:3");
}

// handlers/TestPageEditable_Tests.al:EditableBoundToPageVariable_FollowsTheRow (:92-109): a closed
// TestPage variable that is cleared opens a fresh page.
#[test]
fn clear_of_a_closed_testpage_variable_leaves_a_fresh_closed_one() {
    let r = run(
        "procedure F(): Text var R: Record \"PG Row\"; TP: TestPage \"PG Card\"; T: Text; begin
            R.\"No.\" := 'A'; R.Qty := 1; R.Insert();
            R.\"No.\" := 'B'; R.Qty := 2; R.Insert();
            TP.OpenView();
            T := TP.NoCtl.Value();
            TP.Close();
            Clear(TP);
            TP.OpenView();
            TP.Last();
            exit(T + TP.NoCtl.Value());
        end;",
        "F",
    );
    assert_eq!(text(r), "AB");
}

// Clearing a page that is open is not pinned: refused.
#[test]
fn clear_of_an_open_testpage_variable_is_refused() {
    let r = run(
        "procedure F(): Text var TP: TestPage \"PG Card\"; begin
            TP.OpenNew();
            Clear(TP);
            exit('x');
        end;",
        "F",
    );
    match r {
        Err(Signal::Unsupported(why)) => assert!(why.contains("Clear of a TestPage that is open"), "{why}"),
        other => panic!("{other:?}"),
    }
}

// handlers/TestPageModalNewRowLine_Objects.al:69: `CurrPage.Editable := x` is the setter form of
// `CurrPage.Editable(x)`; the outcome of the method form is pinned by
// handlers/TestPageEditable_Tests.al:150-180.
#[test]
fn assigning_currpage_editable_is_the_same_as_calling_it() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; A: TestPage \"PG EditAssign\"; B: TestPage \"PG EditCall\"; X: Text; begin
                {}
                A.OpenEdit(); B.OpenEdit();
                X := Format(A.Editable()) + Format(A.NoCtl.Editable()) + '|' + Format(B.Editable()) + Format(B.NoCtl.Editable());
                exit(X);
            end;",
            rows(&[("A", 1)])
        ),
        "F",
    );
    assert_eq!(text(r), "NoNo|NoNo");
}

// handlers/TestPageModalNewRowLine_Tests.al:ReEditable_LookupModeTrue_HasNoNewRowLine (:100-114):
// `P.SetTableView(Row)` gives the page the filters of Row as its view when it runs; a trailing
// new-row line follows the filtered rows of an editable list (the `Yes` of the last Next).
#[test]
fn page_settableview_filters_the_rows_the_page_shows() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PG Row\"; V: Record \"PG Row\"; P: Page \"PG List\"; TP: TestPage \"PG List\"; X: Text; begin
                {}
                V.SetRange(Qty, 2, 3);
                P.SetTableView(V);
                TP.Trap();
                P.Run();
                X := TP.\"NoCtl\".Value();
                if TP.Next() then X += TP.\"NoCtl\".Value();
                X += Format(TP.Next());
                TP.Close();
                exit(X);
            end;",
            rows(&[("A", 1), ("B", 2), ("C", 3), ("D", 4)])
        ),
        "F",
    );
    assert_eq!(text(r), "BCYes");
}

// handlers/TestPageModalNewRowLine_Tests.al:Plain_LookupModeTrue_HasNoNewRowLine (:208-221) against
// Plain_LookupModeFalse_HasTheNewRowLine (:190-205): LookupMode(true) takes the new-row line away.
#[test]
fn lookupmode_true_takes_the_new_row_line_away_from_a_list() {
    let body = |mode: &str| {
        format!(
            "procedure F(): Text var R: Record \"PG Row\"; V: Record \"PG Row\"; P: Page \"PG List\"; TP: TestPage \"PG List\"; X: Text; begin
                {}
                V.SetRange(Qty, 2, 3);
                P.SetTableView(V);
                P.LookupMode({mode});
                TP.Trap();
                P.Run();
                X := TP.\"NoCtl\".Value();
                if TP.Next() then X += TP.\"NoCtl\".Value();
                X += Format(TP.Next());
                TP.Close();
                exit(X);
            end;",
            rows(&[("A", 1), ("B", 2), ("C", 3), ("D", 4)])
        )
    };
    assert_eq!(text(run(&body("true"), "F")), "BCNo");
    assert_eq!(text(run(&body("false"), "F")), "BCYes");
}
