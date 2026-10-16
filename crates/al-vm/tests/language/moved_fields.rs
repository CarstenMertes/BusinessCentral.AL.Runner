//! A field a table extension declares with `ObsoleteState = Moved` gives way to the live field of the same id and name
//! that another extension declares. Business Foundation's `tableextension 242` lists Source Code Setup's `Sales` ... as
//! Moved, and Base Application's `tableextension 243` declares them for real; the table had both, the loaded rows
//! filled one and the code read the other, so every Source Code Setup field read blank (73 Tests-ERM tests failed
//! with "Deleted Document must have a value in Source Code Setup").

use al_testkit::{int, run_in};

const SRC: &str = r#"
table 69830 "MF Host" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
tableextension 69831 "MF Old" extends "MF Host" { fields {
  field(50000; Sales; Code[10]) { ObsoleteState = Moved; ObsoleteReason = 'x'; ObsoleteTag = '1'; MovedTo = '437dbf0e-84ff-417a-965d-ed2bb9650972'; } } }
tableextension 69832 "MF Live" extends "MF Host" { fields { field(50000; Sales; Code[10]) { } } }
codeunit 69833 "MF Probe"
{
    procedure F(): Integer
    var R: RecordRef; i: Integer; n: Integer;
    begin
        R.Open(69830);
        for i := 1 to R.FieldCount() do
            if R.FieldIndex(i).Name = 'Sales' then n += 1;
        exit(n);
    end;
}
"#;

#[test]
fn a_moved_field_gives_way_to_the_live_field_of_the_same_name() {
    assert_eq!(int(run_in(SRC, "MF Probe", "F")), 1);
}
