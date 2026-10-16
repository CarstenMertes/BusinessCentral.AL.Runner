//! Find('=><') on a record whose current key is not the primary key finds the row itself first, when the key fields and the
//! primary key name a row that passes the filters (Base Application's Req. Wksh.-Make Order `Code` does this on the
//! Requisition Line the Carry Out Action Msg. report sorted by its own key; Tests-SCM UnitCostOnPurchLineWithCarryOutActionMsg).
//! The neighbour search for a row that is gone is pinned by corpus events/TestProbeEventContracts.al:Find_NeighbourSearch_WithFilterAndOtherKey.

use al_testkit::text;

const SRC: &str = r#"
table 69980 "FOK Row" { fields { field(1; Id; Integer) { } field(2; Name; Code[10]) { } } keys { key(PK; Id) { } key(ByName; Name) { } } }
codeunit 69981 C
{
    procedure F(): Text
    var A: Record "FOK Row"; R: Record "FOK Row";
    begin
        A.Id := 1; A.Name := 'C'; A.Insert();
        A.Id := 2; A.Name := 'A'; A.Insert();
        A.Id := 3; A.Name := 'B'; A.Insert();
        R.SetCurrentKey(Name);
        R.Id := 2; R.Name := 'A';
        exit(Format(R.Find('=><')) + ' ' + Format(R.Id));
    end;
}
"#;

#[test]
fn find_equal_greater_less_on_another_key_finds_the_row_itself() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "Yes 2");
}
