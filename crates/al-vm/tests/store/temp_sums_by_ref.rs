//! The shape Microsoft's costing code leans on (Cost Element Buffer, Tests-SCM TestAvgRevaluationCircularTransfer): a
//! temporary record handed on by reference, rows added through a helper that calls `Get`, `Init` and `Insert`, then
//! `CalcSums` into the record's own fields and a `Total` row written from the sum.

use al_testkit::{run_in, text};

const SRC: &str = r#"
table 50970 "Elt Buf"
{
    fields
    {
        field(1; Type; Option) { OptionMembers = Total,Direct,Indirect; }
        field(2; Cost; Decimal) { }
    }
    keys { key(PK; Type) { Clustered = true; } }

    procedure HasNew(C: Decimal): Boolean
    begin
        exit(C <> 0);
    end;

    procedure GetElt(T: Option Total,Direct,Indirect): Boolean
    begin
        if Get(T) then
            exit(true);
        Init();
        Type := T;
        exit(false);
    end;

    procedure Add(T: Option Total,Direct,Indirect; C: Decimal)
    begin
        if not HasNew(C) then begin
            GetElt(T);
            exit;
        end;
        if GetElt(T) then begin
            Cost += C;
            Modify();
        end else begin
            Cost := C;
            Insert();
        end;
    end;
}
codeunit 50971 Probe
{
    procedure Total(var B: Record "Elt Buf" temporary)
    begin
        B.CalcSums(Cost);
        B.Add(B.Type::Total, B.Cost);
    end;

    procedure Run(): Text
    var B: Record "Elt Buf" temporary; Seen: Text;
    begin
        B.Add(B.Type::Direct, -10);
        B.Add(B.Type::Indirect, 4);
        Total(B);
        B.Get(B.Type::Total);
        exit(Format(B.Cost) + '|' + Format(B.Count()));
    end;

    procedure Plain(): Text
    var B: Record "Elt Buf" temporary;
    begin
        B.Type := B.Type::Direct;
        B.Cost := -10;
        B.Insert();
        B.CalcSums(Cost);
        exit(Format(B.Cost));
    end;

    procedure ViaInit(): Text
    var B: Record "Elt Buf" temporary;
    begin
        B.Init();
        B.Type := B.Type::Direct;
        B.Cost := -10;
        B.Insert();
        B.CalcSums(Cost);
        exit(Format(B.Cost));
    end;

    procedure ViaGet(): Text
    var B: Record "Elt Buf" temporary;
    begin
        if not B.Get(B.Type::Direct) then begin
            B.Init();
            B.Type := B.Type::Direct;
        end;
        B.Cost := -10;
        B.Insert();
        B.CalcSums(Cost);
        exit(Format(B.Cost));
    end;

    procedure RunDirect(): Text
    var B: Record "Elt Buf" temporary;
    begin
        B.Add(B.Type::Direct, -10);
        B.CalcSums(Cost);
        exit(Format(B.Cost));
    end;
}
"#;

#[test]
fn calcsums_into_a_by_reference_temporary_record_feeds_the_total_row() {
    assert_eq!(text(run_in(SRC, "Probe", "Plain")), "-10");
    assert_eq!(text(run_in(SRC, "Probe", "ViaInit")), "-10");
    assert_eq!(text(run_in(SRC, "Probe", "ViaGet")), "-10");
    assert_eq!(text(run_in(SRC, "Probe", "RunDirect")), "-10");
    assert_eq!(text(run_in(SRC, "Probe", "Run")), "-6|3");
}
