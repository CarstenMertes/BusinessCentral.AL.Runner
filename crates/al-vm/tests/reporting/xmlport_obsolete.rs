//! The Obsolete properties of an xmlport change nothing the runtime can observe. Base Application's obsolete PEPPOL
//! xmlports ("Sales Invoice - PEPPOL BIS 3.0": ObsoleteState = Pending) refused 37 Tests-Misc tests with "xmlport
//! property obsoletestate is not supported yet". The other object kinds already take them (al-sema queries.rs).

const SRC: &str = r#"
table 69752 "OBS Blob" { fields { field(1; K; Code[10]) { } field(2; Data; Blob) { } } keys { key(PK; K) { } } }
xmlport 69750 "OBS Port"
{
    Direction = Export; Format = Xml; UseRequestPage = false;
    ObsoleteState = Pending; ObsoleteReason = 'Replaced'; ObsoleteTag = '29.0';
    schema { textelement(Root) { XmlName = 'R'; tableelement(Row; "OBS Blob") { XmlName = 'Row'; fieldelement(Key; Row.K) { } } } }
}
codeunit 69751 C
{
    procedure F(): Text
    var P: XmlPort "OBS Port"; B: Record "OBS Blob"; OutS: OutStream; InS: InStream; T: Text;
    begin
        B.K := 'A'; B.Insert();
        B.Data.CreateOutStream(OutS);
        P.SetDestination(OutS);
        P.Export();
        B.Modify();
        B.CalcFields(Data);
        B.Data.CreateInStream(InS);
        InS.ReadText(T);
        exit(T);
    end;
}
"#;

#[test]
fn an_obsolete_xmlport_runs() {
    let t = al_testkit::text(al_testkit::run_in(SRC, "C", "F"));
    assert!(t.starts_with("<?xml"), "{t}");
}
