//! The elements of an array of temporary records share one temporary table, each with its own filters (Base Application's
//! Inventory Profile Offsetting fills element 1 and plans with element 2; assumption temporary-record-array-shares-table).

use al_testkit::text;

const SRC: &str = r#"
table 69730 "TRA Row" { fields { field(1; Id; Integer) { } field(2; Flag; Boolean) { } } keys { key(PK; Id) { Clustered = true; } } }
codeunit 69731 C
{
    procedure F(): Text
    var A: array[2] of Record "TRA Row" temporary; T: Text;
    begin
        A[1].Id := 1; A[1].Flag := true; A[1].Insert();
        A[1].Id := 2; A[1].Flag := false; A[1].Insert();
        A[2].SetRange(Flag, true);
        T := Format(A[1].Count()) + ';' + Format(A[2].Count()) + ';';
        A[2].FindFirst();
        exit(T + Format(A[2].Id));
    end;
}
"#;

#[test]
fn elements_of_a_temporary_record_array_share_the_table() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "2;1;1");
}
