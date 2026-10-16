//! A Lookup FlowField into an Enum field of another enum type copies the ordinal into its own enum. Base Application's
//! VAT Return Period."VAT Return Status" (Enum "VAT Return Status") is `lookup("VAT Report Header".Status)` (Enum "VAT
//! Report Status"); 25 Tests-Misc tests ("VAT Return Period UT") were refused with "min, max or lookup into a field of
//! another type". No corpus test pins it (assumption `flowfield-lookup-other-enum`).

use al_testkit::text;

const SRC: &str = r#"
enum 69760 "FEO Source" { value(0; Open) { Caption = 'Open'; } value(1; Released) { Caption = 'Released'; } value(2; Closed) { } }
enum 69761 "FEO Target" { value(0; Open) { Caption = 'Open'; } value(1; Released) { Caption = 'Released'; } value(2; Closed) { } }
table 69762 "FEO Header" { fields { field(1; "No."; Code[10]) { } field(2; Status; Enum "FEO Source") { } } keys { key(PK; "No.") { } } }
table 69763 "FEO Period" {
    fields {
        field(1; "No."; Code[10]) { }
        field(2; "Header No."; Code[10]) { }
        field(3; "Header Status"; Enum "FEO Target") { FieldClass = FlowField; CalcFormula = lookup("FEO Header".Status where("No." = field("Header No."))); }
    }
    keys { key(PK; "No.") { } }
}
codeunit 69764 C
{
    procedure F(): Text
    var H: Record "FEO Header"; P: Record "FEO Period";
    begin
        H."No." := 'H1'; H.Status := H.Status::Released; H.Insert();
        P."No." := 'P1'; P."Header No." := 'H1'; P.Insert();
        P.CalcFields("Header Status");
        exit(Format(P."Header Status") + ';' + Format(P."Header Status" = P."Header Status"::Released));
    end;
}
"#;

#[test]
fn a_lookup_into_an_enum_of_another_type_copies_the_ordinal() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "Released;Yes");
}
