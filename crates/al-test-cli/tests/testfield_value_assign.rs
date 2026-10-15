//! `TestPage.Control.Value := x` is the setter form of `SetValue` (issue 31). Pinned by corpus
//! handlers/ProbeTestPageForms.al:108-148 (Value_AssignOnEditableField_VersusSetValue,
//! Value_AssignOnNonEditableField_ErrorText), green on BC 27.5.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

const SRC: &str = r#"
table 63300 "VA Row" { fields { field(1; Id; Integer) { } field(2; Txt; Text[50]) { } field(3; ReadOnlyTxt; Text[50]) { } field(4; "Sell-to Name"; Text[50]) { } } keys { key(PK; Id) { } } }
codeunit 63301 "VA Log" { SingleInstance = true; var L: Text; procedure Add(T: Text) begin L += T; end; procedure Take(): Text var R: Text; begin R := L; L := ''; exit(R); end; }
page 63302 "VA List"
{
    PageType = List; SourceTable = "VA Row";
    layout { area(Content) { repeater(Rows) {
        field(Id; Rec.Id) { }
        field(Txt; Rec.Txt) { trigger OnValidate() var Log: Codeunit "VA Log"; begin Log.Add('OnValidate(' + Rec.Txt + ');'); end; }
        field(ReadOnlyTxt; Rec.ReadOnlyTxt) { Editable = false; }
        field("Sell-to Name"; Rec."Sell-to Name") { }
    } } }
}
codeunit 63302 T
{
    Subtype = Test;
    local procedure Seed() var R: Record "VA Row"; begin R.DeleteAll(); R.Id := 1; R.Txt := 'row1'; R.Insert(); end;

    [Test] procedure ValueAssignIsSetValue()
    var Pg: TestPage "VA List"; Log: Codeunit "VA Log";
    begin
        Seed(); Pg.OpenEdit(); Pg.GoToKey(1); Log.Take();
        Pg.Txt.Value := 'assigned';
        if Log.Take() <> 'OnValidate(assigned);' then Error('log after assign');
        if Pg.Txt.Value() <> 'assigned' then Error('value after assign');
        Pg.Txt.SetValue('viaSetValue');
        if Log.Take() <> 'OnValidate(viaSetValue);' then Error('log after SetValue');
        if Pg.Txt.Value() <> 'viaSetValue' then Error('value after SetValue');
        Pg.Close();
    end;

    [Test] procedure ValueAssignOnANonEditableFieldRaisesNothing()
    var Pg: TestPage "VA List";
    begin
        Seed(); Pg.OpenEdit(); Pg.GoToKey(1);
        Pg.ReadOnlyTxt.Value := 'x';
        Pg.ReadOnlyTxt.SetValue('x');
    end;

    // The Microsoft tests assign a Code field to the Value of a control with a quoted name.
    [Test] procedure ValueAssignOfACodeAndOnANewRow()
    var Pg: TestPage "VA List"; C: Code[20];
    begin
        C := 'ab';
        Pg.OpenNew();
        Pg."Sell-to Name".Value := C;
        if Pg."Sell-to Name".Value() <> 'AB' then Error('got %1', Pg."Sell-to Name".Value());
    end;
}
"#;

#[test]
fn assigning_to_a_control_value_is_set_value() {
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), SRC.into())],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let engine = Engine::new(l.program.clone(), blocked);
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    for (name, outcome) in run_codeunit(&engine, &l, oi) {
        assert_eq!(outcome, Outcome::Pass, "{name}");
    }
}
