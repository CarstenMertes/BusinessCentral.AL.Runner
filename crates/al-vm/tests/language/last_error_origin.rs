//! `GetLastErrorText(true)` answers a text that depends on how the error was raised. Each expectation cites the corpus line
//! that pins it (types/lt-evaluate-format-lasterror-setview/P86LastError.al, errors/lt-last-error-and-events/P70_02LastErrFlag.al,
//! misc/lt-probes-needed-6/PnF01LastErrorFlag.al). An origin the corpus does not pin is refused.

use al_builtins::native::Signal;
use al_testkit::{run_in, text};

const SRC: &str = r#"
table 69870 "LO Row" { fields { field(1; Id; Integer) { } field(2; Name; Text[30]) { DataClassification = CustomerContent; } field(3; Amount; Decimal) { } } keys { key(PK; Id) { } } }
codeunit 69871 "LO C"
{
    var Lbl: Label 'a label text', Locked = true;

    procedure NoError(): Text begin exit(GetLastErrorText(true)); end;

    procedure Info(): Text
    var E: ErrorInfo;
    begin
        E.Message := 'secret';
        asserterror Error(E);
        exit(GetLastErrorText(true));
    end;

    procedure Literal(): Text begin asserterror Error('plain %1', 'argument'); exit(GetLastErrorText(true)); end;
    procedure EmptyLiteral(): Text begin asserterror Error(''); exit(GetLastErrorText(true)); end;
    procedure FromLabel(): Text begin asserterror Error(Lbl); exit(GetLastErrorText(true)); end;

    procedure TestFieldBlank(): Text
    var R: Record "LO Row";
    begin
        R.Id := 1;
        asserterror R.TestField(Name);
        exit(GetLastErrorText(true));
    end;

    procedure TestFieldValue(): Text
    var R: Record "LO Row";
    begin
        R.Id := 1; R.Name := 'Jane';
        asserterror R.TestField(Name, 'John');
        exit(GetLastErrorText(true));
    end;

    procedure FieldErr(): Text
    var R: Record "LO Row";
    begin
        R.Id := 1; R.Name := 'Jane';
        asserterror R.FieldError(Name);
        exit(GetLastErrorText(true));
    end;

    procedure Missing(): Text
    var R: Record "LO Row";
    begin
        R.DeleteAll();
        asserterror R.Get(99);
        exit(GetLastErrorText(true));
    end;

    procedure Divide(): Text
    var A: Integer; Z: Integer;
    begin
        A := 1;
        asserterror A := A div Z;
        exit(GetLastErrorText(true));
    end;
}
"#;

fn flag(routine: &str) -> String {
    text(run_in(SRC, "LO C", routine))
}

#[test]
fn the_pinned_origins_answer_the_pinned_texts() {
    // P86LastError.al:410 (cleared), :189/:192 (empty Error text).
    assert_eq!(flag("NoError"), "");
    assert_eq!(flag("EmptyLiteral"), "");
    // P86LastError.al:54 and the other ErrorInfo cases.
    assert_eq!(
        flag("Info"),
        "This message had personal data removed. The original may still be in transient telemetry. Find it using the transientTelemetryId."
    );
    // P86LastError.al:189, PnF01LastErrorFlag.al:85, :103.
    assert_eq!(
        flag("Literal"),
        "Use ERROR with a text constant to improve telemetry details"
    );
    // P86LastError.al:225 (blank), :233 (value), P70_02LastErrFlag.al:123, :135.
    assert_eq!(
        flag("TestFieldBlank"),
        "Name must have a value in LO Row: <redacted>. It cannot be zero or empty."
    );
    assert_eq!(
        flag("TestFieldValue"),
        "Name must be equal to '<redacted>'  in LO Row: <redacted>. Current value is '<redacted>'."
    );
    // P86LastError.al:270.
    assert_eq!(flag("FieldErr"), "FieldError for incorrect value of LO Row.Name");
    // P86LastError.al:309, P70_02LastErrFlag.al:88.
    assert_eq!(
        flag("Missing"),
        "The LO Row does not exist. Identification fields and values: <redacted>"
    );
    // P86LastError.al:340.
    assert_eq!(flag("Divide"), "Attempted to divide by zero.");
}

#[test]
fn an_error_text_that_is_not_a_literal_is_refused() {
    match run_in(SRC, "LO C", "FromLabel") {
        Err(Signal::Unsupported(m)) => assert!(m.contains("does not pin"), "{m}"),
        other => panic!("{other:?}"),
    }
}
