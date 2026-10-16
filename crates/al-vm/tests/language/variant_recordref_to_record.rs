//! A Record variable takes the record a Variant holds when that is a RecordRef of the same table: the copy of the
//! RecordRef's current record. Electronic Document Format.GetDocumentNo does `SalesInvoiceHeader := DocumentVariant`
//! after `DocumentVariant.IsRecordRef` (Microsoft Tests-Misc, codeunit 139145 "PEPPOL BIS BillingTests".
//! ExportXmlToVerifyFileName, 43 tests refused with "conversion of RecordRef to Record"). Assumption
//! `recordref-variant-to-record`.

use al_testkit::{run_in, text};

const SRC: &str = r#"
table 69720 "VRR Row" { fields { field(1; Id; Integer) { } field(2; Name; Text[30]) { } } keys { key(PK; Id) { } } }
table 69721 "VRR Other" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
codeunit 69722 C
{
    procedure Take(V: Variant): Text
    var R: Record "VRR Row";
    begin
        R := V;
        exit(R.Name);
    end;

    procedure F(): Text
    var R: Record "VRR Row"; RR: RecordRef;
    begin
        R.Id := 7; R.Name := 'seven'; R.Insert();
        RR.GetTable(R);
        RR.FindFirst();
        exit(Take(RR));
    end;

    procedure G(): Text
    var R: Record "VRR Other"; RR: RecordRef;
    begin
        R.Id := 7; R.Insert();
        RR.GetTable(R);
        exit(Take(RR));
    end;
}
"#;

#[test]
fn a_record_takes_the_record_ref_a_variant_holds() {
    assert_eq!(text(run_in(SRC, "C", "F")), "seven");
}

#[test]
fn a_record_ref_of_another_table_is_refused() {
    let r = run_in(SRC, "C", "G");
    assert!(
        format!("{r:?}").contains("RecordRef to Record of another table"),
        "{r:?}"
    );
}
