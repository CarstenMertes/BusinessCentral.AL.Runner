//! An action on a list that shows no stored row (pages/actions.rs `empty_page_after_action`). Each test names the corpus test
//! of misc/lt-probes-needed-7/PnGShowRow.al that pins what BC does.

const SRC: &str = r#"
codeunit 69990 "EL Log"
{
    SingleInstance = true;
    var Entries: Text;
    procedure Add(T: Text) begin if Entries <> '' then Entries += ';'; Entries += T; end;
    procedure Take(): Text var R: Text; begin R := Entries; Entries := ''; exit(R); end;
}
table 69991 "EL Item" { fields { field(1; "No."; Code[20]) { } field(2; Name; Text[30]) { } } keys { key(PK; "No.") { } } }
page 69992 "EL List"
{
    PageType = List; SourceTable = "EL Item"; ApplicationArea = All; UsageCategory = None;
    layout { area(Content) { repeater(Rows) { field(NoCtl; Rec."No.") { } } } }
    actions
    {
        area(Processing)
        {
            action(InsertViaRec) { trigger OnAction() begin Rec.Init(); Rec."No." := 'NEW1'; Rec.Name := 'C'; Rec.Insert(); end; }
            action(InsertViaOther) { trigger OnAction() var O: Record "EL Item"; begin O."No." := 'NEW2'; O.Insert(); end; }
            action(Nothing) { trigger OnAction() begin end; }
        }
    }
    trigger OnNewRecord(BelowxRec: Boolean) var Log: Codeunit "EL Log"; begin Log.Add('N'); end;
    trigger OnAfterGetRecord() var Log: Codeunit "EL Log"; begin Log.Add('R:' + Rec."No."); end;
    trigger OnAfterGetCurrRecord() var Log: Codeunit "EL Log"; begin Log.Add('C:' + Rec."No." + '/' + xRec."No."); end;
}
codeunit 69993 C
{
    procedure Edit(Action: Integer): Text
    var P: TestPage "EL List"; Log: Codeunit "EL Log"; R: Text;
    begin
        P.OpenEdit();
        Log.Take();
        case Action of
            1: P.InsertViaRec.Invoke();
            2: P.InsertViaOther.Invoke();
            3: P.Nothing.Invoke();
        end;
        R := Log.Take() + '|' + P.NoCtl.Value();
        R += '|' + Format(P.First()) + ':' + Log.Take();
        P.Close();
        exit(R);
    end;

    procedure ViewStoredByTest(): Text
    var P: TestPage "EL List"; Item: Record "EL Item"; Log: Codeunit "EL Log"; R: Text;
    begin
        P.OpenView();
        Item."No." := 'T1';
        Item.Insert();
        Log.Take();
        R := Format(P.First()) + ':' + Log.Take();
        P.Nothing.Invoke();
        R += '|' + Log.Take() + '|' + P.NoCtl.Value();
        P.Close();
        exit(R);
    end;
}
"#;

fn edit(action: i32) -> String {
    let mut b = al_testkit::Built::from_source(SRC);
    let (oi, ri) = b.routine_in("C", "Edit");
    let (v, _) = b
        .call(oi, ri, vec![al_builtins::value::Value::Integer(action)])
        .unwrap();
    match v {
        al_builtins::value::Value::Text(t) => t.to_string(),
        other => panic!("{other:?}"),
    }
}

/// :176 QG_EmptyList_InsertViaRec_StepLog: three reads, then OnAfterGetCurrRecord with xRec on the row; First reads nothing.
#[test]
fn an_insert_through_rec_lands_on_the_row() {
    assert_eq!(edit(1), "R:NEW1;R:NEW1;R:NEW1;C:NEW1/NEW1|NEW1|Yes:");
}

/// :223 QG_EmptyList_InsertViaOtherVariable_StepLog: two reads, back to the new-row line; First reads the row twice.
#[test]
fn an_insert_through_another_variable_goes_back_to_the_new_row_line() {
    assert_eq!(edit(2), "R:NEW2;R:NEW2;C:NEW2/;N;C:/||Yes:R:NEW2;R:NEW2;C:NEW2/NEW2");
}

/// :317 QG_EmptyList_ActionStoresNothing_StepLog: the new-row line twice, as on open.
#[test]
fn an_action_that_stores_nothing_runs_the_new_row_line_twice() {
    assert_eq!(edit(3), "N;C:/;N;C:/||No:");
}

/// :459 QG_ViewEmptyList_RowStoredByTest_ThenReads and :494 _ThenFirst: First answers No; the action reads the blank record
/// first, then shows the row.
#[test]
fn a_view_list_shows_a_row_the_test_stored_only_after_an_action() {
    assert_eq!(
        al_testkit::text(al_testkit::run_in(SRC, "C", "ViewStoredByTest")),
        "No:|R:;R:T1;R:T1;C:T1/|T1"
    );
}
