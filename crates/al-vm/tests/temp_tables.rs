//! A temporary record's private table lives as long as a record variable that uses it. It was kept for
//! the whole session (13 KB per variable that touched it): Tests-SCM-Service "Service Response Time" loops
//! over `Calendar Management`, which holds a temporary record in a codeunit that every call creates anew,
//! and one test took 8 GB.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const SRC: &str = r#"
table 50900 "T Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
codeunit 50101 Helper
{
    var Buffer: Record "T Row" temporary;
    procedure Fill(N: Integer) begin Buffer.Id := N; Buffer.Insert(); end;
    procedure Count(): Integer begin exit(Buffer.Count()); end;
    procedure Local(N: Integer): Integer var L: Record "T Row" temporary; begin L.Id := N; L.Insert(); exit(L.Count()); end;
}
codeunit 50100 C
{
    procedure Globals(): Integer var H: Codeunit Helper; i: Integer; begin for i := 1 to 500 do begin Clear(H); H.Fill(i); end; exit(0); end;
    procedure Locals(): Integer var H: Codeunit Helper; i: Integer; begin for i := 1 to 500 do H.Local(i); exit(0); end;
    procedure Kept(): Integer var H: Codeunit Helper; begin H.Fill(1); H.Fill(2); exit(H.Count()); end;
    procedure Shared(): Integer var A: Record "T Row" temporary; B: Record "T Row" temporary; begin A.Id := 1; A.Insert(); B.Copy(A, true); exit(B.Count()); end;
}
"#;

fn session() -> (Session, usize) {
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
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    (Session::new(Engine::new(l.program, blocked)), oi)
}

fn call(s: &mut Session, oi: usize, routine: &str) -> Result<Value, Signal> {
    let ri = s.engine.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .unwrap();
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

#[test]
fn the_temporary_table_of_a_codeunit_that_is_gone_is_freed() {
    let (mut s, oi) = session();
    call(&mut s, oi, "Globals").unwrap();
    assert!(s.temporary_tables_held() < 5, "{} held", s.temporary_tables_held());
}

#[test]
fn the_temporary_table_of_a_local_record_is_freed_when_the_call_returns() {
    let (mut s, oi) = session();
    call(&mut s, oi, "Locals").unwrap();
    assert!(s.temporary_tables_held() < 5, "{} held", s.temporary_tables_held());
}

#[test]
fn a_temporary_table_that_is_still_in_use_keeps_its_rows() {
    let (mut s, oi) = session();
    assert!(matches!(call(&mut s, oi, "Kept"), Ok(Value::Integer(2))));
    // `Copy(x, true)` shares the table: the copy sees the rows of the original (record/TestTempTableContracts.al).
    assert!(matches!(call(&mut s, oi, "Shared"), Ok(Value::Integer(1))));
}
