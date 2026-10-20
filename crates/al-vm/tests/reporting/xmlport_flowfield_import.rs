//! An Xml-format import skips a field element bound to a FlowField: the field keeps its value and the element's
//! OnAfterAssignField does not run (corpus xmlport/lt-ay-xmlport/AyXmlPortLookupFlowField.al:Import_AutoCalc, Import_NoAutoCalc
//! for Lookup; xmlport/lt-xmlport-flowfield-sum-count/XmlPortFlowFieldSumCount.al:326-347 for Sum with OnAfterAssignField, and
//! :350-372 for Sum and Count). Other FlowField methods are refused.

use al_builtins::native::Signal;
use al_testkit::run;

fn src(calc: &str, trigger: &str) -> String {
    format!(
        r#"
table 69960 "XF Blob" {{ fields {{ field(1; Code; Code[10]) {{ }} field(2; Data; Blob) {{ }} }} keys {{ key(PK; Code) {{ }} }} }}
table 69961 "XF Master" {{ fields {{ field(1; Code; Code[10]) {{ }} field(2; Name; Text[30]) {{ }} field(3; Amt; Decimal) {{ }} }} keys {{ key(PK; Code) {{ }} }} }}
table 69962 "XF Row"
{{
    fields
    {{
        field(1; Id; Integer) {{ }}
        field(2; MasterCode; Code[10]) {{ }}
        field(3; Flow; Text[30]) {{ FieldClass = FlowField; CalcFormula = {calc}; }}
    }}
    keys {{ key(PK; Id) {{ }} }}
}}
xmlport 69963 "XF Port"
{{
    Format = Xml; Direction = Import; UseRequestPage = false;
    schema
    {{
        textelement(Root)
        {{
            tableelement(Rw; "XF Row")
            {{
                XmlName = 'Row';
                fieldelement(Id; Rw.Id) {{ }}
                fieldelement(MasterCode; Rw.MasterCode) {{ }}
                fieldelement(Flow; Rw.Flow) {{ {trigger} }}
                trigger OnBeforeInsertRecord() begin Seen += 'before=' + Format(Rw.Flow) + ';'; end;
            }}
        }}
    }}
    var Seen: Text;
    procedure GetSeen(): Text begin exit(Seen); end;
}}
codeunit 69964 C
{{
    procedure F(): Text
    var B: Record "XF Blob" temporary; R: Record "XF Row"; P: XmlPort "XF Port"; OutS: OutStream; InS: InStream;
    begin
        B.Code := 'X'; B.Insert();
        B.Data.CreateOutStream(OutS);
        OutS.WriteText('<Root><Row><Id>9</Id><MasterCode>A</MasterCode><Flow>Typed</Flow></Row></Root>');
        B.Data.CreateInStream(InS);
        P.SetSource(InS); P.Import();
        R.Get(9);
        exit(P.GetSeen() + R.MasterCode);
    end;
}}
"#
    )
}

const LOOKUP: &str = r#"lookup("XF Master".Name where(Code = field(MasterCode)))"#;

#[test]
fn an_import_does_not_assign_a_lookup_flowfield() {
    assert_eq!(al_testkit::text(run(&src(LOOKUP, ""), "F")), "before=;A");
}

#[test]
fn an_import_does_not_run_the_trigger_of_a_sum_flowfield_element() {
    let s = src(
        r#"sum("XF Master".Amt where(Code = field(MasterCode)))"#,
        "trigger OnAfterAssignField() begin Seen += 'assigned;'; end;",
    )
    .replace("field(3; Flow; Text[30])", "field(3; Flow; Decimal)")
    .replace("<Flow>Typed</Flow>", "<Flow>123.5</Flow>");
    assert_eq!(al_testkit::text(run(&s, "F")), "before=0;A");
}

#[test]
fn an_import_into_an_exist_flowfield_is_refused() {
    let s = src(r#"exist("XF Master" where(Code = field(MasterCode)))"#, "")
        .replace("field(3; Flow; Text[30])", "field(3; Flow; Boolean)")
        .replace("<Flow>Typed</Flow>", "<Flow>true</Flow>");
    match run(&s, "F") {
        Err(Signal::Unsupported(r)) => assert!(r.contains("FlowField"), "{r}"),
        other => panic!("{other:?}"),
    }
}
