//! A rename follows a TableRelation arm whose condition is `Field = filter(x)` (assumption
//! rename-follows-filter-condition-arms; Tests-ERM price list tests rename an Item, which Purchase Line."No." reaches through
//! `Document Type = filter(<> "Credit Memo" & <> "Return Order")`).

use al_testkit::text;

const SRC: &str = r#"
table 69960 "RFA Parent" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 69961 "RFA Child"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Kind; Option) { OptionMembers = A,B,C; }
        field(3; Ref; Code[20]) { TableRelation = if (Kind = filter(<> C)) "RFA Parent".Code; }
    }
    keys { key(PK; Id) { } }
}
codeunit 69962 C
{
    procedure F(): Text
    var P: Record "RFA Parent"; C1: Record "RFA Child"; C2: Record "RFA Child";
    begin
        P.Code := 'OLD'; P.Insert();
        C1.Id := 1; C1.Kind := C1.Kind::A; C1.Ref := 'OLD'; C1.Insert();
        C2.Id := 2; C2.Kind := C2.Kind::C; C2.Ref := 'OLD'; C2.Insert();
        P.Rename('NEW');
        C1.Get(1); C2.Get(2);
        exit(C1.Ref + '|' + C2.Ref);
    end;
}
"#;

#[test]
fn a_rename_reaches_the_rows_that_pass_a_filter_condition() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "NEW|OLD");
}
