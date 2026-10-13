//! End to end: AL source → compile → run.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

fn run(src: &str, routine: &str, args: Vec<Value>) -> Result<Value, Signal> {
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
    s.call(oi, ri, inst, args).map(|(v, _)| v)
}

fn cu(body: &str) -> String {
    format!("codeunit 50100 C {{ {body} }}")
}

fn int(r: Result<Value, Signal>) -> i32 {
    match r {
        Ok(Value::Integer(i)) => i,
        other => panic!("{other:?}"),
    }
}

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

#[test]
fn exit_with_arithmetic() {
    assert_eq!(
        int(run(
            &cu("procedure F(): Integer begin exit(2 + 3 * 4); end;"),
            "F",
            vec![]
        )),
        14
    );
}

#[test]
fn named_return_value_and_defaults() {
    assert_eq!(
        int(run(&cu("procedure F() R: Integer begin R := 7; end;"), "F", vec![])),
        7
    );
    assert_eq!(
        int(run(
            &cu("procedure F(): Integer var X: Integer; begin exit(X); end;"),
            "F",
            vec![]
        )),
        0
    );
    assert_eq!(
        int(run(&cu("procedure F(): Integer begin end;"), "F", vec![])),
        0,
        "no exit returns the default"
    );
}

#[test]
fn parameters() {
    assert_eq!(
        int(run(
            &cu("procedure F(A: Integer; B: Integer): Integer begin exit(A - B); end;"),
            "F",
            vec![Value::Integer(9), Value::Integer(4)]
        )),
        5
    );
}

#[test]
fn branches_and_loops() {
    let src = cu("procedure F(): Integer var I: Integer; S: Integer; begin
        for I := 1 to 4 do S += I;
        for I := 3 downto 1 do S += 10;
        while S < 100 do S := S * 2;
        repeat S -= 1; until S mod 7 = 0;
        if S > 1000 then S := 0 else S := S + 1;
        exit(S);
    end;");
    // 1+2+3+4 = 10; +30 = 40; 80, 160; 159,...,154 (154 mod 7 = 0); +1 = 155
    assert_eq!(int(run(&src, "F", vec![])), 155);
}

#[test]
fn case_with_ranges_and_else() {
    let src =
        cu("procedure F(X: Integer): Integer begin case X of 1, 2: exit(10); 3..5: exit(20); else exit(30); end; end;");
    assert_eq!(int(run(&src, "F", vec![Value::Integer(2)])), 10);
    assert_eq!(int(run(&src, "F", vec![Value::Integer(4)])), 20);
    assert_eq!(int(run(&src, "F", vec![Value::Integer(9)])), 30);
}

#[test]
fn in_sets_ternary_and_boolean_ops() {
    let src = cu("procedure F(X: Integer): Integer begin exit((X in [1, 3..5]) and not (X = 4) ? 1 : 0); end;");
    assert_eq!(int(run(&src, "F", vec![Value::Integer(3)])), 1);
    assert_eq!(int(run(&src, "F", vec![Value::Integer(4)])), 0);
}

#[test]
fn break_leaves_the_loop() {
    let src = cu("procedure F(): Integer var I: Integer; begin for I := 1 to 10 do if I = 3 then break; exit(I); end;");
    assert_eq!(int(run(&src, "F", vec![])), 3);
}

#[test]
fn exit_from_inside_a_loop() {
    let src =
        cu("procedure F(): Integer var I: Integer; begin while true do begin I += 1; if I = 5 then exit(I); end; end;");
    assert_eq!(int(run(&src, "F", vec![])), 5);
}

#[test]
fn var_parameters_write_back() {
    let src = cu(
        "procedure Inc(var X: Integer) begin X += 1; end; procedure F(): Integer var V: Integer; begin V := 1; Inc(V); Inc(V); exit(V); end;",
    );
    assert_eq!(int(run(&src, "F", vec![])), 3);
}

#[test]
fn globals_live_on_the_instance() {
    let src = cu(
        "var G: Integer; procedure Bump() begin G += 1; end; procedure F(): Integer begin Bump(); Bump(); exit(G); end;",
    );
    assert_eq!(int(run(&src, "F", vec![])), 2);
}

#[test]
fn codeunit_variables_have_their_own_instances() {
    let src = "codeunit 50101 Counter { var N: Integer; procedure Add(): Integer begin N += 1; exit(N); end; }\n"
        .to_string()
        + &cu(
            "procedure F(): Integer var A: Codeunit Counter; B: Codeunit Counter; begin A.Add(); A.Add(); exit(A.Add() * 10 + B.Add()); end;",
        );
    assert_eq!(int(run(&src, "F", vec![])), 31);
}

#[test]
fn builtins_and_conversions() {
    assert_eq!(
        text(run(
            &cu("procedure F(): Text begin exit(Format(42, 5)); end;"),
            "F",
            vec![]
        )),
        "   42"
    );
    assert_eq!(
        int(run(
            &cu("procedure F(): Integer begin exit(StrLen('abc')); end;"),
            "F",
            vec![]
        )),
        3
    );
    assert_eq!(
        text(run(
            &cu("procedure F(): Code[10] var C: Code[10]; begin C := ' ab '; exit(C); end;"),
            "F",
            vec![]
        )),
        "AB"
    );
}

#[test]
fn error_and_asserterror() {
    let src = cu("procedure F(): Text begin asserterror Error('boom %1', 5); exit(GetLastErrorText()); end;");
    assert_eq!(text(run(&src, "F", vec![])), "boom 5");
    let src = cu("procedure F() begin Error('nope'); end;");
    assert!(matches!(run(&src, "F", vec![]), Err(Signal::Error(e)) if e.message == "nope"));
}

#[test]
fn asserterror_without_an_error_is_an_error() {
    let src = cu("procedure F() var I: Integer; begin asserterror I := 1; end;");
    assert!(matches!(run(&src, "F", vec![]), Err(Signal::Error(_))));
}

#[test]
fn a_blocked_routine_reports_unsupported() {
    let src = cu("procedure F() begin Sleep(1); end;");
    assert!(matches!(run(&src, "F", vec![]), Err(Signal::Unsupported(r)) if r.contains("Sleep")));
}

#[test]
fn enum_values_format_by_name() {
    let src = "enum 50900 E { value(0; Alpha) { } value(3; \"Beta Two\") { } }\n".to_string()
        + &cu("procedure F(): Text var X: Enum E; begin X := E::\"Beta Two\"; exit(Format(X)); end;");
    assert_eq!(text(run(&src, "F", vec![])), "Beta Two");
}

#[test]
fn runaway_recursion_is_unsupported_not_a_crash() {
    // Same stack the CLI gives the VM (al-test-cli main).
    std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(|| {
            let src = cu("procedure F(N: Integer): Integer begin exit(F(N + 1)); end;");
            assert!(matches!(run(&src, "F", vec![Value::Integer(0)]), Err(Signal::Unsupported(r)) if r.contains("call depth")));
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn try_function_used_as_a_value_catches_errors() {
    // Doc (TryFunction attribute): the return value tells whether the method ran without errors.
    let src = cu("[TryFunction] procedure T(Fail: Boolean) begin if Fail then Error('inner'); end;
        procedure F(): Text var Ok1: Boolean; Ok2: Boolean; begin Ok1 := T(false); Ok2 := T(true); exit(Format(Ok1) + '/' + Format(Ok2) + '/' + GetLastErrorText()); end;");
    assert_eq!(text(run(&src, "F", vec![])), "Yes/No/inner");
}

#[test]
fn try_function_called_as_a_statement_raises() {
    // Doc: when the return value is not used, the try method behaves like a normal method.
    let src = cu("[TryFunction] procedure T() begin Error('inner'); end; procedure F() begin T(); end;");
    assert!(matches!(run(&src, "F", vec![]), Err(Signal::Error(e)) if e.message == "inner"));
}

#[test]
fn single_instance_codeunits_share_state_across_variables() {
    let src = "codeunit 50102 Single { SingleInstance = true; var N: Integer; procedure Add(): Integer begin N += 1; exit(N); end; }\n".to_string()
        + &cu("procedure F(): Integer var A: Codeunit Single; B: Codeunit Single; begin A.Add(); exit(B.Add()); end;");
    assert_eq!(int(run(&src, "F", vec![])), 2);
}

#[test]
fn calling_an_event_publisher_is_unsupported_until_events_exist() {
    // Running the empty publisher body would silently skip every subscriber.
    let src = cu("[IntegrationEvent(false, false)] procedure OnX() begin end; procedure F() begin OnX(); end;");
    assert!(
        matches!(run(&src, "F", vec![]), Err(Signal::Unsupported(r)) if r.contains("event")),
        "{:?}",
        run(&src, "F", vec![])
    );
}
