//! `TableRelation = if (Type = const(" ")) A else if (Type = const(Real)) B` on an Option field whose first member is the
//! blank `" "` (Finance Charge Memo Line."No."; Tests-ERM ERM Finance Charge Memo Apply IssueFinanceChargeMemoWithInvoiceRounding).
//! The blank member reads like the enum case of a CalcFormula/relation const (`Standard Service Line."No."`).

use al_testkit::text;

const SRC: &str = r#"
table 69990 "ROB Std" { fields { field(1; Code; Code[10]) { } } keys { key(PK; Code) { } } }
table 69991 "ROB Acc" { fields { field(1; No; Code[10]) { } } keys { key(PK; No) { } } }
table 69992 "ROB Line"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Type; Option) { OptionMembers = " ","Account"; }
        field(3; No; Code[10]) { TableRelation = if (Type = const(" ")) "ROB Std" else if (Type = const(Account)) "ROB Acc"; }
    }
    keys { key(PK; Id) { } }
}
codeunit 69993 C
{
    procedure F(): Text
    var S: Record "ROB Std"; A: Record "ROB Acc"; L: Record "ROB Line";
    begin
        S.Code := 'S1'; S.Insert();
        A.No := 'A1'; A.Insert();
        L.Id := 1;
        L.Validate(No, 'S1');
        L.Type := L.Type::Account;
        L.Validate(No, 'A1');
        exit(L.No);
    end;
}
"#;

#[test]
fn an_option_relation_condition_on_the_blank_member_picks_its_arm() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "A1");
}
