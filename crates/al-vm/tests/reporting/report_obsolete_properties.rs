//! A report that carries ObsoleteState, ObsoleteReason and ObsoleteTag runs as one without them (Base Application's
//! "Calculate Subcontracts"; Tests-SCM-Manufacturing refused 34 tests with "report property obsoletereason is not supported yet").

use al_testkit::text;

const SRC: &str = r#"
table 69960 "ROP Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
report 69961 "ROP Rpt"
{
    ProcessingOnly = true; UseRequestPage = false;
    ObsoleteState = Pending; ObsoleteReason = 'Replaced'; ObsoleteTag = '27.0';
    dataset { dataitem(Item; "ROP Row") { trigger OnAfterGetRecord() begin Hits += 1; end; } }
    var Hits: Integer;
    procedure GetHits(): Integer begin exit(Hits); end;
}
codeunit 69962 C
{
    procedure F(): Text
    var R: Report "ROP Rpt"; Row: Record "ROP Row";
    begin
        Row.Id := 1; Row.Insert(); Row.Id := 2; Row.Insert();
        R.RunModal();
        exit(Format(R.GetHits()));
    end;
}
"#;

#[test]
fn a_report_with_obsolete_properties_runs() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "2");
}
