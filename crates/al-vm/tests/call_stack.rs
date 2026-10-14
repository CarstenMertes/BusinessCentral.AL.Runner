//! `GetLastErrorCallStack`: the AL frames an error unwound through, innermost first.
//!
//! What the corpus pins (codeunit/TestCodeunitAlCallStack.al): the stack names the frames'
//! objects as `(CodeUnit <id>)`, carries ` line `, ` by ` and ` version `, marks a trigger
//! frame `OnRun(Trigger)` and a procedure frame without `(Trigger)`, and is empty when no error
//! was raised. The rest of the frame text, and the line numbers, follow AL Runner v2, which
//! measured them on real BC (`docs/probes-needed.md`, "Call stacks").

use al_builtins::native::{Signal, UNTRACKED_CALL_STACK};
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

fn run(src: &str, routine: &str) -> Result<Value, Signal> {
    let (ws, mut l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "Stack App".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src.into())],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    l.program.apps[0].publisher = "Tester".into();
    l.program.apps[0].version = "1.2.3.4".into();
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

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

const APP: &str = "Stack App by Tester version 1.2.3.4";

const TWO_FRAMES: &str = r#"codeunit 50100 "Stack Test"
{
    procedure Caller(): Text
    var
        Helper: Codeunit "Stack Helper";
    begin
        asserterror Helper.Fail();
        exit(GetLastErrorCallStack());
    end;
}

codeunit 50101 "Stack Helper"
{
    procedure Fail()
    begin
        Error('boom');
    end;
}
"#;

// codeunit/TestCodeunitAlCallStack.al:CallStack_AfterAssertError_ContainsALFrames: the frame of the
// test and the frame of the helper, with their object ids, ` line `, ` by ` and ` version `.
#[test]
fn an_error_through_a_helper_lists_both_frames_innermost_first() {
    assert_eq!(
        text(run(TWO_FRAMES, "Caller")),
        format!(
            "\"Stack Helper\"(CodeUnit 50101).Fail line 2 - {APP}\n\"Stack Test\"(CodeUnit 50100).Caller line 4 - {APP}"
        )
    );
}

// codeunit/TestCodeunitAlCallStack.al:CallStack_WhenNoError_ReturnsEmpty
#[test]
fn without_an_error_the_stack_is_empty() {
    let src = r#"codeunit 50100 C { procedure F(): Text begin exit(GetLastErrorCallStack()); end; }"#;
    assert_eq!(text(run(src, "F")), "");
}

// codeunit/TestCodeunitAlCallStack.al:CallStack_ErrorInsideOnRunTrigger_MarksTheFrameAsATrigger and
// :CallStack_ErrorInsideAProcedure_DoesNotMarkTheFrameAsATrigger
#[test]
fn a_trigger_frame_is_marked_and_a_procedure_frame_is_not() {
    let src = r#"codeunit 50100 "Stack Test"
{
    procedure Caller(): Text
    var
        T: Codeunit "Trigger Helper";
    begin
        asserterror T.Run();
        exit(GetLastErrorCallStack());
    end;
}

codeunit 50102 "Trigger Helper"
{
    trigger OnRun()
    begin
        Error('boom');
    end;
}
"#;
    let stack = text(run(src, "Caller"));
    assert!(
        stack.contains("\"Trigger Helper\"(CodeUnit 50102).OnRun(Trigger) line 2 - "),
        "{stack}"
    );
    assert!(
        stack.contains("\"Stack Test\"(CodeUnit 50100).Caller line 4 - "),
        "{stack}"
    );
    assert!(!stack.contains("Caller(Trigger)"), "{stack}");
}

// The line is the statement that was running, counted from the `procedure` line: an error in a
// nested statement names that statement (AL Runner v2 InlineScopeEmitTests: OnInsert line 3).
#[test]
fn the_line_is_the_innermost_statement_counted_from_the_procedure_line() {
    let src = r#"codeunit 50100 C
{
    [Scope('OnPrem')]
    procedure F(): Text
    var
        X: Integer;
    begin
        X := 1;
        if X = 1 then begin
            // a comment
            X := 2;
            Error('boom');
        end;
    end;

    procedure Caller(): Text
    begin
        asserterror F();
        exit(GetLastErrorCallStack());
    end;
}
"#;
    let stack = text(run(src, "Caller"));
    assert_eq!(
        stack,
        format!("\"C\"(CodeUnit 50100).F line 8 - {APP}\n\"C\"(CodeUnit 50100).Caller line 2 - {APP}")
    );
}

#[test]
fn a_table_trigger_frame_names_the_table() {
    let src = r#"table 50103 "Stack Row"
{
    fields { field(1; PK; Integer) { } }
    keys { key(PK; PK) { Clustered = true; } }

    trigger OnInsert()
    begin
        if PK = 0 then
            Error('PK needs a value');
    end;
}

codeunit 50100 C
{
    procedure Caller(): Text
    var
        R: Record "Stack Row";
    begin
        asserterror R.Insert(true);
        exit(GetLastErrorCallStack());
    end;
}
"#;
    let stack = text(run(src, "Caller"));
    assert_eq!(
        stack,
        format!(
            "\"Stack Row\"(Table 50103).OnInsert(Trigger) line 3 - {APP}\n\"C\"(CodeUnit 50100).Caller line 4 - {APP}"
        )
    );
}

#[test]
fn a_try_function_error_lists_the_try_frame_and_the_frame_that_caught_it() {
    let src = r#"codeunit 50100 C
{
    [TryFunction]
    procedure Try()
    begin
        Error('boom');
    end;

    procedure Caller(): Text
    begin
        if not Try() then
            exit(GetLastErrorCallStack());
    end;
}
"#;
    assert_eq!(
        text(run(src, "Caller")),
        format!("\"C\"(CodeUnit 50100).Try line 2 - {APP}\n\"C\"(CodeUnit 50100).Caller line 2 - {APP}")
    );
}

// An error that comes back round a loop condition names the loop statement, not the last
// statement of the body.
#[test]
fn a_loop_condition_error_names_the_loop_line() {
    let src = r#"codeunit 50100 C
{
    procedure Bad(i: Integer): Boolean
    begin
        if i > 1 then
            Error('boom');
        exit(true);
    end;

    procedure Caller(): Text
    var
        i: Integer;
    begin
        asserterror
            while Bad(i) do
                i += 1;
        exit(GetLastErrorCallStack());
    end;
}
"#;
    let stack = text(run(src, "Caller"));
    assert!(
        stack.ends_with(&format!("\"C\"(CodeUnit 50100).Caller line 5 - {APP}")),
        "{stack}"
    );
}

// A frame whose text BC's format leaves open is not guessed: the stack stays unreadable.
#[test]
fn a_frame_the_format_is_not_pinned_for_makes_the_stack_unreadable() {
    let src = r#"table 50104 "Field Row"
{
    fields
    {
        field(1; PK; Integer) { }
        field(2; Name; Text[30])
        {
            trigger OnValidate()
            begin
                Error('boom');
            end;
        }
    }
    keys { key(PK; PK) { Clustered = true; } }
}

codeunit 50100 C
{
    procedure Caller(): Text
    var
        R: Record "Field Row";
    begin
        asserterror R.Validate(Name, 'x');
        exit(GetLastErrorCallStack());
    end;
}
"#;
    let stack = text(run(src, "Caller"));
    assert!(stack.contains(UNTRACKED_CALL_STACK), "{stack:?}");
}
