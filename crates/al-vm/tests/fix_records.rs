//! Record behaviour pinned by the corpus probes of cdf315a (record/TestRecordRuntimeTextsAndViews.al,
//! record/TestTableEventSequenceContracts.al, record/TestCalcFieldsTruncateUniqueContracts.al) and
//! by the local batch 2 probes (corpus branch al-runtime-rs, pending push: record/*.al in
//! corpus-probes head 74e5b30). Each test mirrors a probe test so the behaviour has a red/green
//! test here as well as in the corpus run.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

fn run(src: &str, routine: &str) -> Result<Value, Signal> {
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src.into())],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .unwrap();
    if let Some(Some(r)) = blocked.get(oi).and_then(|o| o.get(ri)) {
        return Err(Signal::Unsupported(r.clone()));
    }
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

const EVT: &str = r#"
codeunit 50300 Log
{
    SingleInstance = true;
    var L: Text;
    procedure Add(E: Text) begin L += E + ';'; end;
    procedure Take(): Text var R: Text; begin R := L; L := ''; exit(R); end;
}
table 50301 Evt
{
    fields { field(1; Id; Integer) { } field(2; Val; Text[20]) { } field(4; Pay; Blob) { } }
    keys { key(PK; Id) { } }
    trigger OnInsert() var G: Codeunit Log; begin G.Add('T.OnInsert(' + Val + ')'); end;
    trigger OnModify() var G: Codeunit Log; begin G.Add('T.OnModify(' + Val + ',' + xRec.Val + ')'); end;
    trigger OnDelete() var G: Codeunit Log; begin G.Add('T.OnDelete(' + Format(Id) + ')'); end;
    trigger OnRename() var G: Codeunit Log; begin G.Add('T.OnRename(' + Format(Id) + ',' + Format(xRec.Id) + ')'); end;
}
codeunit 50302 Sub
{
    var Hits: Integer;
    [EventSubscriber(ObjectType::Table, Database::Evt, 'OnBeforeInsertEvent', '', false, false)]
    local procedure BI(var Rec: Record Evt; RunTrigger: Boolean) var G: Codeunit Log; begin Hits += 1; G.Add('S.BI(temp=' + Format(Rec.IsTemporary()) + ',hits=' + Format(Hits) + ')'); end;
    [EventSubscriber(ObjectType::Table, Database::Evt, 'OnAfterInsertEvent', '', false, false)]
    local procedure AI(var Rec: Record Evt; RunTrigger: Boolean) var G: Codeunit Log; begin G.Add('S.AI(hits=' + Format(Hits) + ')'); end;
    [EventSubscriber(ObjectType::Table, Database::Evt, 'OnBeforeModifyEvent', '', false, false)]
    local procedure BM(var Rec: Record Evt; var xRec: Record Evt; RunTrigger: Boolean) var G: Codeunit Log; begin G.Add('S.BM(' + Rec.Val + ',' + xRec.Val + ')'); end;
    [EventSubscriber(ObjectType::Table, Database::Evt, 'OnBeforeDeleteEvent', '', false, false)]
    local procedure BD(var Rec: Record Evt; RunTrigger: Boolean) var G: Codeunit Log; begin G.Add('S.BD(' + Format(Rec.Id) + ')'); end;
    [EventSubscriber(ObjectType::Table, Database::Evt, 'OnAfterDeleteEvent', '', false, false)]
    local procedure AD(var Rec: Record Evt; RunTrigger: Boolean) var G: Codeunit Log; begin G.Add('S.AD(' + Format(Rec.Id) + ')'); end;
    [EventSubscriber(ObjectType::Table, Database::Evt, 'OnBeforeRenameEvent', '', false, false)]
    local procedure BR(var Rec: Record Evt; var xRec: Record Evt; RunTrigger: Boolean) var G: Codeunit Log; begin G.Add('S.BR(' + Format(Rec.Id) + ',' + Format(xRec.Id) + ')'); end;
}
"#;

fn with(body: &str) -> String {
    format!("{EVT}\ncodeunit 50303 C {{ {body} }}")
}

#[test]
fn failing_writes_run_before_event_and_trigger_then_raise() {
    let src = with(
        "procedure F(): Text var E: Record Evt; G: Codeunit Log; R: Text; begin
            E.Id := 1; E.Val := 'a'; E.Insert(true); G.Take();
            E.Id := 1; asserterror E.Insert(true); R += G.Take() + '|';
            E.Id := 99; E.Val := 'x'; asserterror E.Modify(true); R += G.Take() + '|';
            asserterror E.Delete(true); R += G.Take() + '|';
            asserterror E.Rename(100); R += G.Take();
            exit(R);
        end;",
    );
    assert_eq!(
        text(run(&src, "F")),
        "S.BI(temp=No,hits=1);T.OnInsert(a);|S.BM(x,x);T.OnModify(x,x);|S.BD(99);T.OnDelete(99);|S.BR(100,99);T.OnRename(100,99);"
    );
}

