//! `CurrFieldNo := FieldNo(X)` in table code (Base Application's Sales Header `SelltoContactLookup` and Purchase
//! Header `BuyfromContactLookup` do it before they validate a field). The corpus pins the *reading* of CurrFieldNo
//! (handlers/TestPageCurrFieldNo_Tests.al); that an assignment is kept for the rest of the code on that record is not
//! pinned (docs/probes-needed.md).

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

fn run(body: &str) -> Result<Value, Signal> {
    let src = format!(
        "table 60990 \"CFN\" {{ fields {{ field(1; K; Code[10]) {{ }} field(2; V; Integer) {{ }} }} keys {{ key(PK; K) {{ }} }}
           procedure Before(): Integer begin exit(CurrFieldNo); end;
           procedure Set(): Integer begin CurrFieldNo := FieldNo(V); exit(CurrFieldNo); end; }}
         codeunit 50100 C {{ procedure F(): Integer var R: Record CFN; begin {body} end; }}"
    );
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
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == "F")
        .unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

#[test]
fn an_assignment_to_curr_field_no_is_read_back_on_the_same_record_only() {
    // Set() assigns FieldNo(V) = 2 and reads it back; another record variable still reads 0.
    let v = run("R.K := 'A'; exit(R.Set() * 10 + R.Before());");
    assert!(matches!(v, Ok(Value::Integer(22))), "{v:?}");
    let v = run("R.K := 'A'; exit(R.Before());");
    assert!(matches!(v, Ok(Value::Integer(0))), "{v:?}");
}
