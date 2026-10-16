//! `GetLastErrorText(true)` with no error pending answers the empty text (corpus
//! types/lt-evaluate-format-lasterror-setview/P86LastError.al:ErrorInfo_CustomerContent `cleared_excl`). Microsoft's Marketing
//! and Misc helpers call it that way. With an error pending the redaction depends on how the error was raised and stays refused.

use al_builtins::native::Signal;
use al_testkit::{run_in, text};

const SRC: &str = r#"
codeunit 69841 "LE C"
{
    procedure NoError(): Text
    begin
        exit(GetLastErrorText(true));
    end;

    procedure Pending(): Text
    begin
        asserterror Error('plain error');
        exit(GetLastErrorText(true));
    end;
}
"#;

#[test]
fn the_flag_with_no_error_pending_answers_the_empty_text() {
    assert_eq!(text(run_in(SRC, "LE C", "NoError")), "");
}

#[test]
fn the_flag_with_an_error_pending_is_refused_because_the_redaction_depends_on_the_kind() {
    match run_in(SRC, "LE C", "Pending") {
        Err(Signal::Unsupported(m)) => assert!(m.contains("how the error was raised"), "{m}"),
        other => panic!("{other:?}"),
    }
}
