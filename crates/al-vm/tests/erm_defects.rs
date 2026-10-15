//! Runtime defects found in Tests-ERM (issue 9). Each test says where the construct comes from.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const SRC: &str = r#"
table 50900 "ALT T"
{
    fields { field(1; Id; Integer) { } field(2; Descr; Text[30]) { } }
    keys { key(PK; Id) { } }
}
"#;

fn run(body: &str) -> Result<Value, Signal> {
    let src = format!("{SRC}\ncodeunit 50100 C {{ {body} }}");
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
        .collect::<Vec<Vec<Option<String>>>>();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == "F")
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

/// Tests-ERM "Discount Setup Notifications" declares `GeneralPostingSetup: array[2] of Record "General
/// Posting Setup"` and calls `GeneralPostingSetup[1].FieldNo(..)` and `.Init()`. Every element is a
/// record variable of its own, like any record variable (collections/TestArray.al:19-29 pins arrays of
/// simple values).
#[test]
fn the_elements_of_an_array_of_records_are_record_variables() {
    let r = run(
        "procedure F(): Text var A: array[2] of Record \"ALT T\"; begin
            A[1].Init(); A[1].Id := 5; A[1].Descr := 'one'; A[1].Insert();
            A[2].Id := 6; A[2].Descr := 'two'; A[2].Insert();
            exit(Format(A[1].FieldNo(Descr)) + '|' + A[2].Descr + '|' + Format(A[1].Count()) + '|' + Format(A[1].Id)); end;",
    );
    assert_eq!(text(r), "2|two|2|5");
}

#[test]
fn two_elements_of_an_array_of_records_do_not_share_their_buffer() {
    let r = run(
        "procedure F(): Text var A: array[2] of Record \"ALT T\"; begin
            A[1].Id := 1; A[2].Id := 2; A[1].SetRange(Id, 1);
            exit(Format(A[1].Id) + '|' + Format(A[2].Id) + '|' + Format(A[1].HasFilter()) + '|' + Format(A[2].HasFilter())); end;",
    );
    assert_eq!(text(r), "1|2|Yes|No");
}

/// Base Application's `Company Information.IsDigit` does `Letter: Code[1]; Letter[1] := LetterChar`. A Code
/// upper-cases the character (types/TestCharIndexAssignContracts.al:CharacterAssignedIntoAPosition).
#[test]
fn assigning_a_character_into_a_code_upper_cases_it() {
    let r = run("procedure F(): Boolean var L: Code[1]; begin L[1] := 'a'; exit(L = 'A'); end;");
    assert!(matches!(&r, Ok(Value::Boolean(true))), "{r:?}");
}

/// Tests-ERM "ERM Copy Name To Ledg. Entries".AddBlankEntry opens a table that has no row in the demo
/// data, so `FindLast` is false and the FieldRef is never assigned, then calls `FieldRef.Value(..)`.
/// types/PSUnassignedFieldRef.al pins the read members as errors; assigning Value on one is not pinned, so it is
/// refused and says why.
#[test]
fn assigning_value_on_a_field_ref_that_was_never_assigned_is_refused_as_not_pinned() {
    let r = run("procedure F() var F: FieldRef; begin F.Value(1); end;");
    assert!(
        matches!(&r, Err(Signal::Unsupported(m)) if m.contains("FieldRef.Value") && m.contains("never assigned") && m.contains("not pinned")),
        "{r:?}"
    );
}

/// types/PSUnassignedFieldRef.al UnassignedFieldRef_Members: reading Name on a never-assigned FieldRef raises.
#[test]
fn name_of_a_field_ref_that_was_never_assigned_raises_not_initialized() {
    let r = run("procedure F() var F: FieldRef; begin F.Name(); end;");
    assert!(
        matches!(&r, Err(Signal::Error(e)) if e.code == "NotInitialized" && e.message == "Microsoft.Dynamics.Nav.Runtime.NavFieldRef variable not initialized."),
        "{r:?}"
    );
}
