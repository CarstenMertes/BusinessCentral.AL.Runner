//! `GetLastErrorText(true)`: the text with the values of the last error redacted. errors/lt-last-error-and-events/
//! P70_02LastErrFlag.al pins eight kinds of error; the redaction is not implemented, so a pending error is refused. With no
//! error the text is empty (assumption `get-last-error-text-flag-without-error`; Microsoft's Word Template Impl.Merge reads it
//! on the success path, about 315 Tests-Marketing and Tests-Misc tests).

use al_testkit::text;

const SRC: &str = r#"
codeunit 69770 C
{
    procedure NoError(): Text
    begin
        exit('[' + GetLastErrorText(true) + ']');
    end;

    procedure Plain(): Text
    begin
        asserterror Error('Boom');
        exit(GetLastErrorText(false) + '|' + GetLastErrorText());
    end;

    procedure Redacted(): Text
    begin
        asserterror Error('Boom');
        exit(GetLastErrorText(true));
    end;
}
"#;

#[test]
fn the_flag_with_no_error_is_empty() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "NoError")), "[]");
}

#[test]
fn the_flag_false_is_the_plain_text() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "Plain")), "Boom|Boom");
}

#[test]
fn the_flag_true_with_an_error_pending_is_refused() {
    let r = al_testkit::run_in(SRC, "C", "Redacted");
    assert!(format!("{r:?}").contains("P70_02LastErrFlag"), "{r:?}");
}
