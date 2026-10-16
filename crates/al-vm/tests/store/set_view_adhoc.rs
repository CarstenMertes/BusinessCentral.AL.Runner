//! SetView of SORTING(...) on fields no key starts with orders the rows by those fields (assumption
//! set-view-sorting-without-a-key; Tests-SCM Posting Preview tests set the view GetView wrote after an ad hoc SetCurrentKey).

use al_testkit::text;

const SRC: &str = r#"
table 69970 "SVA Row" { fields { field(1; Id; Integer) { } field(2; Name; Code[10]) { } } keys { key(PK; Id) { } } }
codeunit 69971 C
{
    procedure F(): Text
    var A: Record "SVA Row"; B: Record "SVA Row"; T: Text;
    begin
        A.Id := 1; A.Name := 'C'; A.Insert();
        A.Id := 2; A.Name := 'A'; A.Insert();
        A.Id := 3; A.Name := 'B'; A.Insert();
        A.Reset();
        A.SetCurrentKey(Name);
        B.SetView(A.GetView());
        B.FindSet();
        repeat T += B.Name; until B.Next() = 0;
        exit(T);
    end;
}
"#;

#[test]
fn a_view_with_a_sorting_no_key_starts_with_orders_the_rows() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "ABC");
}
