//! Calls in report extension code resolve against the extension's own variables and labels.
//! The Manufacturing report extensions of Microsoft's Tests-ERM bucket call `Window.Open(..)` on a
//! Dialog declared in the extension; the lowerer used to refuse that call with an unresolved callee.

use al_sema::workspace::AppSource;

const SRC: &str = r#"
table 69100 "RE Tab" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
report 69101 "RE Base"
{
    UsageCategory = None;
    dataset { dataitem(Item; "RE Tab") { column(Id; Id) { } } }
    var BaseText: Text; BaseLbl: Label 'Base';
}
reportextension 69102 "RE Ext" extends "RE Base"
{
    dataset
    {
        modify(Item)
        {
            trigger OnPreDataItem()
            begin
                Window.Open(WindowTxt + Lbl);
                Window.Update(1, 5);
                Window.Close();
            end;
        }
    }
    trigger OnPreReport()
    begin
        Buf.Init();
        Buf.Id := StrLen(Lbl);
        Txt := Lbl + WindowTxt;
        Txt := CopyStr(Txt, 1, 2);
        Names.Add(Txt);
        Clear(Names);
        Item.SetRange(Id, 1);
    end;
    procedure P(): Integer
    begin
        Names.Add('x');
        exit(Names.Count());
    end;
    var
        Window: Dialog;
        WindowTxt: Label 'Working #1', Locked = true;
        Lbl: Label 'Lbl';
        Buf: Record "RE Tab";
        Txt: Text;
        Names: List of [Text];
}
"#;

#[test]
fn every_call_on_a_variable_of_a_report_extension_is_lowered() {
    let apps = vec![AppSource {
        id: "a".into(),
        name: "a".into(),
        dependencies: vec![],
        files: vec![("f.al".into(), SRC.into())],
    }];
    let (ws, l) = al_sema::lower::compile(apps, &Default::default());
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let mut reasons = Vec::new();
    for (oi, o) in l.unsupported.iter().enumerate() {
        for (ri, r) in o.iter().enumerate() {
            if let Some(r) = r.first() {
                reasons.push(format!(
                    "{}.{}: {r}",
                    l.program.objects[oi].name, l.program.objects[oi].routines[ri].name
                ));
            }
        }
    }
    assert!(reasons.is_empty(), "{reasons:#?}");
}
