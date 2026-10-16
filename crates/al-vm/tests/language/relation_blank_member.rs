//! `TableRelation = "Service Item" where(Blocked = filter(" "))` (Service Contract Line): the filter names the enum member
//! whose name is one space. The relation text lost its quotes and was an empty filter term, which refused 265 Tests-SCM-Service
//! tests ("where() of a relation: empty filter term").

use al_testkit::{int, run_in};

const SRC: &str = r#"
enum 69850 "RB Blk" { Extensible = true; value(0; " ") { } value(1; Yes) { } }
table 69851 "RB Item" { fields { field(1; No; Code[10]) { } field(2; Blocked; Enum "RB Blk") { } } keys { key(PK; No) { } } }
table 69852 "RB Line" { fields { field(1; Id; Integer) { } field(2; ItemNo; Code[10]) { TableRelation = "RB Item" where(Blocked = filter(" ")); } } keys { key(PK; Id) { } } }
codeunit 69853 "RB C"
{
    procedure F(): Integer
    var I: Record "RB Item"; L: Record "RB Line"; Hits: Integer;
    begin
        I.No := 'OPEN'; I.Insert();
        I.No := 'BLOCKED'; I.Blocked := I.Blocked::Yes; I.Insert();
        L.Id := 1;
        L.Validate(ItemNo, 'OPEN');
        Hits += 1;
        asserterror L.Validate(ItemNo, 'BLOCKED');
        if GetLastErrorText() <> '' then Hits += 10;
        exit(Hits);
    end;
}
"#;

#[test]
fn a_relation_filter_naming_the_blank_enum_member_selects_the_unblocked_rows() {
    assert_eq!(int(run_in(SRC, "RB C", "F")), 11);
}
