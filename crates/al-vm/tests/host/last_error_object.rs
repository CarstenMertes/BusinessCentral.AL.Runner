//! `GetLastErrorObject` (OnPrem): the last error as a .NET exception object, served by the runtime.
//! Every pinned literal is what the real service tier answered in the corpus:
//! tests/al-language-onprem-dotnet/lt-dotnet-interop/GetLastErrorObject.al (BC 27, 28, 29; corpus issue 572).
//! The stack text of `ToString` after its first line is not pinned (only that it has `   at ` frames).

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_testkit::text;

const SRC: &str = r#"
dotnet { assembly("mscorlib") {
    type("System.Exception"; LeoExc) { }
    type("System.Type"; LeoType) { }
} }
table 50160 "LEO Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
codeunit 50161 "LEO Thrower"
{
    var Mode: Integer;
    trigger OnRun()
    var Row: Record "LEO Row"; Zero: Integer; Result: Integer; Info: ErrorInfo;
    begin
        case Mode of
            1: Error('boom from Run');
            2: Row.Get(-987654);
            3: Result := 1 div Zero;
            4: Error('boom with %1 and %2', 'arg', 42);
            5: Error(Lbl);
            6: begin Info.Message := 'info message'; Error(Info); end;
            7: Row.TestField(Id, 5);
        end;
    end;
    procedure SetMode(NewMode: Integer) begin Mode := NewMode; end;
    [TryFunction] procedure TryRaise() begin Error('boom from try'); end;
    procedure Raise() begin Error('boom from asserterror'); end;
    var Lbl: Label 'boom from label';
}
codeunit 50100 C
{
    local procedure Describe(): Text
    var Exc: DotNet LeoExc; T: DotNet LeoType; Last: Text; Out: Text;
    begin
        Last := GetLastErrorText();
        Exc := GetLastErrorObject();
        Out := Format(IsNull(Exc));
        if IsNull(Exc) then exit(Out);
        T := Exc.GetType();
        exit(Out + '|' + Exc.Message + '|' + Format(Exc.Message = Last) + '|' + T.FullName + '|' + Format(IsNull(Exc.InnerException)));
    end;
    local procedure FirstLine(Value: Text): Text
    var Lf: Text[1];
    begin
        Lf[1] := 10;
        exit(CopyStr(Value, 1, StrPos(Value + Lf, Lf) - 1));
    end;
    BODY
}
"#;

fn run(body: &str) -> Result<Value, Signal> {
    let src = SRC.replace("BODY", body);
    al_testkit::run_in(&src, "C", "F")
}

fn after_run(mode: i32) -> Result<Value, Signal> {
    run(&format!(
        "procedure F(): Text var T: Codeunit \"LEO Thrower\"; begin T.SetMode({mode}); if T.Run() then exit('ran'); exit(Describe()); end;"
    ))
}

const DIALOG: &str = "Microsoft.Dynamics.Nav.Types.Exceptions.NavNCLDialogException";

/// GetLastErrorObject.al:138-160 (Error() caught by Codeunit.Run).
#[test]
fn error_caught_by_codeunit_run_is_a_dialog_exception() {
    assert_eq!(text(after_run(1)), format!("No|boom from Run|Yes|{DIALOG}|Yes"));
}

/// GetLastErrorObject.al:162-185 (Error with substitutions).
#[test]
fn error_with_substitutions_is_a_dialog_exception() {
    assert_eq!(text(after_run(4)), format!("No|boom with arg and 42|Yes|{DIALOG}|Yes"));
}

/// GetLastErrorObject.al:187-209 (TryFunction).
#[test]
fn error_in_a_try_function_is_a_dialog_exception() {
    let r = text(run(
        "procedure F(): Text var T: Codeunit \"LEO Thrower\"; begin if T.TryRaise() then exit('ok'); exit(Describe()); end;",
    ));
    assert_eq!(r, format!("No|boom from try|Yes|{DIALOG}|Yes"));
}

/// GetLastErrorObject.al:211-230 (asserterror).
#[test]
fn error_in_asserterror_is_a_dialog_exception() {
    let r = text(run(
        "procedure F(): Text var T: Codeunit \"LEO Thrower\"; begin asserterror T.Raise(); exit(Describe()); end;",
    ));
    assert_eq!(r, format!("No|boom from asserterror|Yes|{DIALOG}|Yes"));
}

/// GetLastErrorObject.al:232-255 (Get without a row).
#[test]
fn get_without_a_row_is_a_record_not_found_exception() {
    assert_eq!(
        text(after_run(2)),
        "No|The LEO Row does not exist. Identification fields and values: Id='-987654'|Yes|Microsoft.Dynamics.Nav.Types.Exceptions.NavCSideRecordNotFoundException|Yes"
    );
}

/// GetLastErrorObject.al:257-280 (division by zero): the inner exception is not null.
#[test]
fn division_by_zero_is_an_al_exception_with_an_inner_exception() {
    assert_eq!(
        text(after_run(3)),
        "No|Attempted to divide by zero.|Yes|Microsoft.Dynamics.Nav.Types.Exceptions.NavALException|No"
    );
}

/// GetLastErrorObject.al:84-87: Format(Exc) = Exc.ToString(); the first line is `<type>: <message>`; the text has frames.
#[test]
fn to_string_starts_with_type_and_message_and_has_frames() {
    let r = text(run(
        "procedure F(): Text var T: Codeunit \"LEO Thrower\"; Exc: DotNet LeoExc; begin T.SetMode(1); if T.Run() then exit('ran');
            Exc := GetLastErrorObject();
            exit(Format(Format(Exc) = Exc.ToString()) + '|' + FirstLine(Exc.ToString()) + '|' + Format(StrPos(Exc.ToString(), '   at ') > 0)); end;",
    ));
    assert_eq!(r, format!("Yes|{DIALOG}: boom from Run|Yes"));
}

/// GetLastErrorObject.al:283-301: after ClearLastError the object is null; Format is 'null'; members raise.
#[test]
fn after_clear_last_error_the_object_is_null() {
    let r = text(run(
        "procedure F(): Text var T: Codeunit \"LEO Thrower\"; Exc: DotNet LeoExc; begin T.SetMode(1); if T.Run() then exit('ran');
            ClearLastError(); Exc := GetLastErrorObject();
            exit(Format(IsNull(Exc)) + '|' + Format(Exc)); end;",
    ));
    assert_eq!(r, "Yes|null");
}

#[test]
fn a_member_of_the_null_object_raises_the_not_instantiated_error() {
    let r = text(run(
        "procedure F(): Text var Exc: DotNet LeoExc; M: Text; begin ClearLastError(); Exc := GetLastErrorObject();
            asserterror M := Exc.Message; exit(GetLastErrorText() + '|' + GetLastErrorCode()); end;",
    ));
    assert_eq!(
        r,
        "A DotNet variable has not been instantiated. Attempting to call System.Exception.Message in CodeUnit C: F|DotNetVariableNotCreated"
    );
}

/// GetLastErrorObject.al:303-330: a later successful Codeunit.Run keeps the last error.
#[test]
fn a_successful_run_after_an_error_keeps_the_object() {
    let r = text(run(
        "procedure F(): Text var T: Codeunit \"LEO Thrower\"; begin T.SetMode(1); if T.Run() then exit('ran'); T.SetMode(0); if not T.Run() then exit('failed'); exit(Describe()); end;",
    ));
    assert_eq!(r, format!("No|boom from Run|Yes|{DIALOG}|Yes"));
}

/// Not pinned: the exception type of an error raised by Error(ErrorInfo) or TestField is refused, naming the code.
#[test]
fn an_error_kind_the_corpus_does_not_pin_is_refused() {
    for mode in [6, 7] {
        match after_run(mode) {
            Err(Signal::Unsupported(why)) => assert!(why.contains("GetLastErrorObject"), "{why}"),
            other => panic!("mode {mode}: {other:?}"),
        }
    }
}

/// Error(Label) is not the literal form GetLastErrorObject.al pins; it is the registered assumption that it is
/// the same exception.
#[test]
fn error_with_a_label_is_assumed_to_be_a_dialog_exception() {
    assert_eq!(text(after_run(5)), format!("No|boom from label|Yes|{DIALOG}|Yes"));
}

/// Not pinned: GetLastErrorObject before any error happened in the session is refused. After ClearLastError it is
/// null (GetLastErrorObject.al:283-301).
#[test]
fn before_any_error_is_refused_but_after_clear_is_null() {
    let fresh = run(
        "procedure F(): Text var Exc: DotNet LeoExc; begin Exc := GetLastErrorObject(); exit(Format(IsNull(Exc))); end;",
    );
    match fresh {
        Err(Signal::Unsupported(why)) => assert!(why.contains("before any error"), "{why}"),
        other => panic!("{other:?}"),
    }
    let cleared = text(run(
        "procedure F(): Text var Exc: DotNet LeoExc; begin ClearLastError(); Exc := GetLastErrorObject(); exit(Format(IsNull(Exc))); end;",
    ));
    assert_eq!(cleared, "Yes");
}

/// ToString and Format after the first line are not pinned (only the `   at ` marker): both mark the assumption.
#[test]
fn to_string_and_format_mark_the_stack_frame_assumption() {
    use al_builtins::assumptions::{Assumption, take_hits};
    for body in ["exit(Exc.ToString());", "exit(Format(Exc));"] {
        take_hits();
        let r = text(run(&format!(
            "procedure F(): Text var T: Codeunit \"LEO Thrower\"; Exc: DotNet LeoExc; begin T.SetMode(1); if T.Run() then exit('ran'); Exc := GetLastErrorObject(); {body} end;"
        )));
        assert!(r.starts_with(DIALOG), "{r}");
        assert!(
            take_hits().contains(&Assumption::ExceptionToStringStackFrames),
            "{body}"
        );
    }
    // The members whose text is pinned do not mark it.
    take_hits();
    let _ = after_run(1);
    assert!(!take_hits().contains(&Assumption::ExceptionToStringStackFrames));
}
