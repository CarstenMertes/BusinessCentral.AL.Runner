//! `Rec."File Name"` passed to a `var` parameter in a codeunit with a TableNo (Base Application's "Read Data Exch.
//! from File": `OnBeforeFileImport(TempBlob, Rec."File Name")`) and an assignment to a bare field of the implicit Rec there (the
//! test library's "Save Data Exch. Blob Sample": `"File Name" := ...`); 89 Tests-ERM tests were refused with "is not assignable".

use al_testkit::{run_in, text};

const SRC: &str = r#"
table 69820 "RV Table" { fields { field(1; Id; Integer) { } field(2; "File Name"; Text[250]) { } } keys { key(PK; Id) { } } }
codeunit 69821 "RV Reader"
{
    TableNo = "RV Table";
    trigger OnRun() begin Fill(Rec."File Name"); "File Name" := 'bare ' + "File Name"; end;
    local procedure Fill(var Name: Text) begin Name := 'filled'; end;
}
codeunit 69822 "RV Caller"
{
    procedure F(): Text
    var T: Record "RV Table"; R: Codeunit "RV Reader";
    begin
        R.Run(T);
        exit(T."File Name");
    end;
}
"#;

#[test]
fn a_field_of_rec_in_a_table_no_codeunit_can_be_passed_by_reference() {
    assert_eq!(text(run_in(SRC, "RV Caller", "F")), "bare filled");
}
