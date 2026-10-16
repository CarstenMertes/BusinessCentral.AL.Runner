//! `const('<guid>')` in a CalcFormula on a Guid field. Base Application's Purchase Line counts the matched order lines
//! whose `Matched Rcpt./Shpt. Line SysId` is `const('00000000-0000-0000-0000-000000000000')`; every Validate that
//! calculates it was refused ("CalcFormula const(..) is not understood for a Guid field", 286 tests of one bucket). The
//! text of a Guid reads as `Evaluate` reads it (types/ Guid conversions of the corpus).

use al_testkit::int;

const SRC: &str = r#"
table 69600 "CG Line"
{
    fields { field(1; Id; Integer) { } field(2; Ref; Guid) { } }
    keys { key(PK; Id) { } }
}
table 69601 "CG Head"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Empty; Integer) { FieldClass = FlowField; CalcFormula = count("CG Line" where(Ref = const('00000000-0000-0000-0000-000000000000'))); }
        field(3; Given; Integer) { FieldClass = FlowField; CalcFormula = count("CG Line" where(Ref = const('{11111111-2222-3333-4444-555555555555}'))); }
    }
    keys { key(PK; Id) { } }
}
codeunit 69602 C
{
    procedure F(): Integer
    var H: Record "CG Head"; L: Record "CG Line"; G: Guid;
    begin
        Evaluate(G, '11111111-2222-3333-4444-555555555555');
        L.Id := 1; L.Insert();
        L.Id := 2; L.Ref := G; L.Insert();
        L.Id := 3; L.Ref := G; L.Insert();
        H.Id := 1; H.Insert();
        H.CalcFields(Empty, Given);
        exit(H.Empty * 10 + H.Given);
    end;
}
"#;

#[test]
fn a_guid_constant_in_a_calc_formula_compares_as_a_guid() {
    assert_eq!(int(al_testkit::run_in(SRC, "C", "F")), 12);
}
