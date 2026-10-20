//! CurrPage.Update(false) from an action on a list after a GoToKey, with and without OnAfterGetCurrRecord
//! (pages/lt-ai-list-order/AiListOrderTests.al). AL source -> compile -> run as a test codeunit.

use al_test_cli::report::Outcome;

const OBJECTS: &str = r#"
table 62500 "LUW Row" { fields { field(1; "No."; Code[10]) { } field(2; Qty; Integer) { } } keys { key(PK; "No.") { } } }
codeunit 62501 "LUW Log" { SingleInstance = true; var T: Text;
    procedure Clear() begin T := ''; end;
    procedure Add(S: Text) begin if T <> '' then T += ';'; T += S; end;
    procedure Dump(): Text begin exit(T); end;
    procedure Seed() var R: Record "LUW Row"; I: Integer; begin R.DeleteAll(); for I := 1 to 6 do begin R."No." := 'R100' + Format(I); R.Qty := I; R.Insert(); end; end; }
page 62502 "LUW Plain" { PageType = List; SourceTable = "LUW Row"; Editable = false;
    layout { area(Content) { repeater(G) { field(NoCtl; Rec."No.") { } } } }
    actions { area(Processing) {
        action(KeepFilter) { trigger OnAction() var L: Codeunit "LUW Log"; begin L.Add('A:filter'); Rec.SetFilter(Qty, '<=%1', 4); CurrPage.Update(false); L.Add('A:done'); end; }
        action(Drop) { trigger OnAction() var L: Codeunit "LUW Log"; begin L.Add('A:filter'); Rec.SetFilter(Qty, '>%1', Rec.Qty); CurrPage.Update(false); L.Add('A:done'); end; } } }
    trigger OnAfterGetRecord() var L: Codeunit "LUW Log"; begin L.Add('R:' + Rec."No."); end; }
page 62503 "LUW Plain Curr" { PageType = List; SourceTable = "LUW Row"; Editable = false;
    layout { area(Content) { repeater(G) { field(NoCtl; Rec."No.") { } } } }
    actions { area(Processing) {
        action(Drop) { trigger OnAction() var L: Codeunit "LUW Log"; begin L.Add('A:filter'); Rec.SetFilter(Qty, '>%1', Rec.Qty); CurrPage.Update(false); L.Add('A:done'); end; } } }
    trigger OnAfterGetRecord() var L: Codeunit "LUW Log"; begin L.Add('R:' + Rec."No."); end;
    trigger OnAfterGetCurrRecord() var L: Codeunit "LUW Log"; begin L.Add('C:' + Rec."No."); end; }
"#;

fn passes(tests: &str) {
    let src = format!("{OBJECTS}\ncodeunit 62510 T {{ Subtype = Test; {tests} }}");
    let o = al_testkit::outcomes_in(&src, "T");
    assert!(!o.is_empty());
    for (n, r) in &o {
        assert_eq!(r, &Outcome::Pass, "{n}");
    }
}

// AiListOrderTests.al:233 Keep_Plain: two reads of the row before the action (one is the GoToKey's), the rows from it on, the way
// back, and nothing more without OnAfterGetCurrRecord.
#[test]
fn update_on_a_plain_list_without_current_record_trigger_after_goto_key() {
    passes(
        r#"[Test] procedure Keep() var L: Codeunit "LUW Log"; P: TestPage "LUW Plain"; begin
            L.Seed(); P.OpenView(); L.Clear(); P.GoToKey('R1002');
            if L.Dump() <> '' then Error('goto %1', L.Dump());
            P.KeepFilter.Invoke();
            if L.Dump() <> 'R:R1002;R:R1002;A:filter;A:done;R:R1002;R:R1002;R:R1003;R:R1004;R:R1002;R:R1001' then Error('update %1', L.Dump());
        end;"#,
    );
}

// AiListOrderTests.al:335 DropCurrent_RowsAfter_Plain and :369 DropCurrent_RowsAfter_PlainCurr: the filter leaves the row, the list
// settles on the next row of the view, and First stays on it.
#[test]
fn update_after_the_filter_left_the_row_settles_on_the_next_row() {
    passes(
        r#"[Test] procedure Plain() var L: Codeunit "LUW Log"; P: TestPage "LUW Plain"; begin
            L.Seed(); P.OpenView(); P.GoToKey('R1002'); L.Clear();
            P.Drop.Invoke();
            if L.Dump() <> 'R:R1002;R:R1002;A:filter;A:done;R:R1003;R:R1003;R:R1004;R:R1005;R:R1006;R:R1003' then Error('update %1', L.Dump());
            L.Clear(); P.First(); if P.NoCtl.Value() <> 'R1003' then Error('first %1', P.NoCtl.Value()); if L.Dump() <> '' then Error('after first %1', L.Dump());
        end;
        [Test] procedure PlainCurr() var L: Codeunit "LUW Log"; P: TestPage "LUW Plain Curr"; begin
            L.Seed(); P.OpenView(); P.GoToKey('R1002'); L.Clear();
            P.Drop.Invoke();
            if L.Dump() <> 'R:R1002;A:filter;A:done;R:R1003;R:R1003;R:R1004;R:R1005;R:R1006;R:R1003;R:R1003;C:R1003' then Error('update %1', L.Dump());
            L.Clear(); P.First(); if P.NoCtl.Value() <> 'R1003' then Error('first %1', P.NoCtl.Value()); if L.Dump() <> '' then Error('after first %1', L.Dump());
        end;"#,
    );
}
