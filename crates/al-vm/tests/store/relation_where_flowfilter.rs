//! A where() condition of a TableRelation on a FlowFilter field of the related table does not select rows: it is a flow filter
//! (Item Journal Line's "Bin Code" relation `Bin.Code where("Location Code" = field(..), "Item Filter" = field("Item No."),
//! "Variant Filter" = field("Variant Code"))`; Tests-SCM Invt. Doc. Pst Preview TestInventoryShipmentPreviewWithBin; assumption
//! relation-where-on-a-flowfilter-of-the-target).

use al_testkit::text;

const SRC: &str = r#"
table 69990 "RWF Bin" { fields { field(1; Loc; Code[10]) { } field(2; Code; Code[10]) { } field(3; "Item Filter"; Code[20]) { FieldClass = FlowFilter; } } keys { key(PK; Loc, Code) { } } }
table 69991 "RWF Line" { fields { field(1; Id; Integer) { } field(2; Loc; Code[10]) { } field(3; ItemNo; Code[20]) { }
  field(4; BinCode; Code[10]) { TableRelation = "RWF Bin".Code where(Loc = field(Loc), "Item Filter" = field(ItemNo)); } } keys { key(PK; Id) { } } }
codeunit 69992 C
{
    procedure F(): Text
    var B: Record "RWF Bin"; L: Record "RWF Line";
    begin
        B.Loc := 'L1'; B.Code := 'B1'; B.Insert();
        L.Id := 1; L.Loc := 'L1'; L.ItemNo := 'I1';
        L.Validate(BinCode, 'B1');
        exit(L.BinCode);
    end;
}
"#;

#[test]
fn a_where_on_a_flowfilter_of_the_related_table_does_not_select_rows() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "B1");
}
