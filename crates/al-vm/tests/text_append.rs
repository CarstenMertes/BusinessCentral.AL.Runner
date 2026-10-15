//! `S += x` and `S := S + x` on a text variable append in place (a loop over a long text is linear). The values AL
//! sees must be those of the general path: a copy taken before the append keeps its text, the left operand is the
//! value before the right one runs, and a text that does not fit raises with the variable unchanged.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

fn run(body: &str) -> Result<Value, Signal> {
    let src =
        format!("codeunit 50100 C {{ var G: Text; procedure Grow(): Text begin G += 'g'; exit('r'); end; {body} }}");
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

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s.to_string(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn appending_in_a_loop_gives_the_text_and_a_copy_keeps_its_own() {
    let r = text(run("procedure F(): Text var S: Text; T: Text; i: Integer; begin
            for i := 1 to 5 do S += Format(i);
            T := S; S += 'x'; S := S + 'y';
            exit(S + '|' + T);
        end;"));
    assert_eq!(r, "12345xy|12345");
}

#[test]
fn the_left_operand_is_the_value_before_the_right_operand_runs() {
    // `G += Grow()` : Grow appends 'g' to G while the right side runs; the statement stores old G + 'r'.
    let r = text(run("procedure F(): Text begin G := 'a'; G += Grow(); exit(G); end;"));
    assert_eq!(r, "ar");
}

#[test]
fn a_text_that_does_not_fit_raises_and_leaves_the_variable_alone() {
    let r = text(run("procedure F(): Text var S: Text[5]; begin
            S := 'abc';
            asserterror S += 'defg';
            exit(S + '|' + GetLastErrorText());
        end;"));
    assert!(
        r.starts_with("abc|The length of the string is 7, but it must be less than or equal to 5 characters"),
        "{r}"
    );
}

#[test]
fn a_char_and_a_code_join_as_before() {
    let r = text(run("procedure F(): Text var S: Text; C: Code[10]; begin
            C := 'ab'; S := 'x'; S += C; S += 'é'; exit(S + Format(StrLen(S)));
        end;"));
    assert_eq!(r, "xABé4");
}
