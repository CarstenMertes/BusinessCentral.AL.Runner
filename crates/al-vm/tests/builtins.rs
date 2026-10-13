//! End to end: single builtins that need the compiled program (enum metadata) or session state.
//! Each test names the corpus test that pins the behaviour it asserts.

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
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l
        .program
        .objects
        .iter()
        .position(|o| o.routines.iter().any(|r| r.name == routine))
        .expect("object");
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

const STATUS: &str = "enum 50101 \"ALT Status\" { value(0; \" \") { Caption = ' '; } value(1; Draft) { Caption = 'Draft'; } value(5; Closed) { Caption = 'Closed'; } }\n";

/// Runs `procedure F(): Integer var <vars> begin <body> end;` next to the enum.
fn int_of(vars: &str, body: &str) -> Result<i32, Signal> {
    let src = format!(
        "{STATUS}codeunit 50100 C {{ procedure F(): Integer var {vars} begin {body} end; }}"
    );
    match run(&src, "F")? {
        Value::Integer(i) => Ok(i),
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------- Enum

#[test]
fn enum_names_lists_every_declared_name() {
    // types/TestEnumNamesContracts.al:19-60: `"ALT Status".Names()` holds the names, blank ordinal included.
    let v = "N: List of [Text];";
    assert_eq!(int_of(v, "N := \"ALT Status\".Names(); exit(N.Count());").unwrap(), 3);
    assert_eq!(int_of(v, "N := \"ALT Status\".Names(); if N.Contains('Draft') and N.Contains('Closed') then exit(1); exit(0);").unwrap(), 1);
}

#[test]
fn enum_names_works_on_a_variable() {
    // types/TestEnumNamesContracts.al:77-85 (`S.Names()`).
    let v = "S: Enum \"ALT Status\"; N: List of [Text];";
    assert_eq!(int_of(v, "S := \"ALT Status\"::Draft; N := S.Names(); exit(N.Count());").unwrap(), 3);
}

#[test]
fn enum_ordinals_lists_every_declared_value() {
    // session/TestSystemExtended.al:381-390 and types/TestLanguageEdgeCases.al:91-105.
    let v = "O: List of [Integer];";
    assert_eq!(int_of(v, "O := \"ALT Status\".Ordinals(); exit(O.Count());").unwrap(), 3);
    assert_eq!(int_of(v, "O := \"ALT Status\".Ordinals(); if O.Contains(0) and O.Contains(1) and O.Contains(5) then exit(1); exit(0);").unwrap(), 1);
}

#[test]
fn names_and_ordinals_line_up() {
    // types/TestEnumNamesContracts.al:62-70: equal counts; the Nth name belongs to the Nth ordinal.
    let v = "N: List of [Text]; O: List of [Integer];";
    assert_eq!(
        int_of(v, "N := \"ALT Status\".Names(); O := \"ALT Status\".Ordinals(); if (N.Get(2) = 'Draft') and (O.Get(2) = 1) then exit(1); exit(0);").unwrap(),
        1
    );
}

#[test]
fn enum_as_integer_returns_the_ordinal() {
    // types/TestEnum.al:27-40.
    let v = "S: Enum \"ALT Status\";";
    assert_eq!(int_of(v, "S := \"ALT Status\"::Closed; exit(S.AsInteger());").unwrap(), 5);
    assert_eq!(int_of(v, "exit(S.AsInteger());").unwrap(), 0);
}

#[test]
fn enum_from_integer_carries_any_ordinal() {
    // types/TestEnum.al:15-22 (a declared ordinal), :83-91 and types/TestEnumDefaultImplTests.Codeunit.al:70
    // (an undeclared one keeps its number); types/TestEnumShippedOrdinalTests.Codeunit.al:91 spells the
    // receiver `Enum::"Name"`.
    let v = "S: Enum \"ALT Status\";";
    assert_eq!(int_of(v, "S := \"ALT Status\".FromInteger(5); if S = \"ALT Status\"::Closed then exit(1); exit(0);").unwrap(), 1);
    assert_eq!(int_of(v, "S := \"ALT Status\".FromInteger(99); exit(S.AsInteger());").unwrap(), 99);
    assert_eq!(int_of(v, "S := Enum::\"ALT Status\".FromInteger(1); exit(S.AsInteger());").unwrap(), 1);
}
