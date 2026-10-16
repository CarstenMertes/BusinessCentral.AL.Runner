//! `DeleteAll(true)` goes row by row and does not visit a row an earlier OnDelete deleted (Requisition Line's OnDelete
//! deletes the lines of a higher level that follow; Order Planning Mgt. PrepareRequisitionRecord DeleteAll(true);
//! Tests-SCM-Manufacturing B42538_UserIDOnReqLine; assumption delete-all-skips-rows-deleted-meanwhile).

use al_testkit::text;

const SRC: &str = r#"
table 69980 "DAC Row" { fields { field(1; Id; Integer) { } field(2; Lvl; Integer) { } } keys { key(PK; Id) { }
}
    trigger OnDelete()
    var R: Record "DAC Row";
    begin
        R.Get(Rec.Id);
        while (R.Next() <> 0) and (R.Lvl > Rec.Lvl) do
            R.Delete(true);
    end;
}
codeunit 69981 C
{
    procedure F(): Text
    var R: Record "DAC Row";
    begin
        R.Id := 1; R.Lvl := 0; R.Insert();
        R.Id := 2; R.Lvl := 1; R.Insert();
        R.Id := 3; R.Lvl := 0; R.Insert();
        R.Reset();
        R.DeleteAll(true);
        exit(Format(R.Count()));
    end;
}
"#;

#[test]
fn delete_all_does_not_visit_a_row_an_earlier_delete_trigger_removed() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "0");
}
