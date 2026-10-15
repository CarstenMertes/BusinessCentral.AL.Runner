//! Refusals that kept Microsoft's Tests-SCM-Service tests from running (issue 41).

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const TABLES: &str = r#"
table 50800 "SV Row"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; Name; Text[50]) { }
        field(3; Qty; Decimal) { }
    }
    keys { key(PK; "Entry No.") { } }
}
"#;

fn text(body: &str) -> Result<String, Signal> {
    let src =
        format!("{TABLES}codeunit 50100 C {{ procedure F(): Text var Rec: Record \"SV Row\"; begin {body} end; }}");
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src)],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l
        .program
        .objects
        .iter()
        .position(|o| o.routines.iter().any(|r| r.name == "F"))
        .expect("object");
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == "F")
        .unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    match s.call(oi, ri, inst, vec![])?.0 {
        Value::Text(t) => Ok(t.to_string()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn test_field_with_an_error_info_that_holds_does_nothing() {
    // Base Application "Gen. Jnl.-Check Line".RunCheck: `GenJnlLine.TestField("Account No.", ErrorInfo.Create())`
    // and `TestField(Blocked, false, ErrorInfo.Create())`. Documentation only: the check is TestField's,
    // and a check that holds raises nothing (docs/probes-needed.md).
    let r = text(
        "Rec.\"Entry No.\" := 1; Rec.Name := 'x'; Rec.TestField(Name, ErrorInfo.Create());
         Rec.TestField(Name, 'x', ErrorInfo.Create()); Rec.TestField(Qty, 0, ErrorInfo.Create('m')); exit('ok');",
    );
    assert_eq!(r.unwrap(), "ok");
}

#[test]
fn test_field_with_an_error_info_that_fails_is_refused() {
    // What BC raises from the ErrorInfo (its message, code, collectible) is not pinned.
    for body in [
        "Rec.TestField(Name, ErrorInfo.Create()); exit('ran');",
        "Rec.TestField(Name, 'y', ErrorInfo.Create()); exit('ran');",
    ] {
        let r = text(body);
        match r {
            Err(Signal::Unsupported(why)) => assert!(why.contains("ErrorInfo"), "{why}"),
            other => panic!("{body}: {other:?}"),
        }
    }
}
