//! A list whose only row an action deleted. After an action that calls CurrPage.Update the list shows its blank line and
//! raises OnAfterGetCurrRecord three times (corpus pageupdate/TestPageUpdateGoneRow.al:List_DeletedByAction_OnlyRow_ShowsNoStoredRow).
//! Without the call the same is assumed (list-gone-only-row-without-update); the corpus does not pin the count there.

use al_testkit::text;

const SRC: &str = r#"
table 69980 "GR Row" { fields { field(1; Code; Code[10]) { } } keys { key(PK; Code) { } } }
codeunit 69981 "GR Log"
{
    SingleInstance = true;
    var N: Integer;
    procedure Bump() begin N += 1; end;
    procedure Take(): Integer var R: Integer; begin R := N; N := 0; exit(R); end;
}
page 69982 "GR List"
{
    PageType = List; SourceTable = "GR Row"; ApplicationArea = All; UsageCategory = None;
    layout { area(Content) { repeater(R) { field(Code; Rec.Code) { } } } }
    actions { area(Processing) { action(Gone) { ApplicationArea = All; trigger OnAction() begin Rec.Delete(); end; } } }
    trigger OnAfterGetCurrRecord() var L: Codeunit "GR Log"; begin L.Bump(); end;
}
codeunit 69983 C
{
    procedure F(): Text
    var R: Record "GR Row"; P: TestPage "GR List"; L: Codeunit "GR Log"; Before: Integer;
    begin
        R.Code := 'A'; R.Insert();
        P.OpenEdit();
        L.Take();
        P.Gone.Invoke();
        Before := L.Take();
        P.Close();
        exit(Format(Before));
    end;
}
"#;

#[test]
fn a_list_whose_only_row_an_action_deleted_shows_the_blank_line() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "3");
}
