//! A table field's OptionCaptionML gives the captions of the session language (record/lt-x-table-props/XTablePropsTests.al:
//! 219-232). The runtime does not read it, so the captions are refused, never taken from another list (hard rule 4).

use al_builtins::native::Signal;
use al_builtins::value::Value;

const SRC: &str = "table 50100 T { fields { field(1; Id; Integer) { } field(2; Op; Option) { OptionMembers = A,B,C; OptionCaptionML = ENU = 'Alpha,Beta,Gamma', DEU = 'Eins,Zwei,Drei'; } field(3; Plain; Option) { OptionMembers = A,B,C; } } } \
    codeunit 50100 C { \
    procedure Fmt(): Text var R: Record T; begin R.Op := R.Op::B; exit(Format(R.Op)); end; \
    procedure Cap(): Text var RR: RecordRef; F: FieldRef; begin RR.Open(50100); F := RR.Field(2); exit(F.OptionCaption); end; \
    procedure PlainFmt(): Text var R: Record T; begin R.Plain := R.Plain::B; exit(Format(R.Plain)); end; }";

fn refused(r: Result<Value, Signal>) {
    match r {
        Err(Signal::Unsupported(why)) => assert!(why.contains("OptionCaptionML"), "{why}"),
        other => panic!("expected an OptionCaptionML refusal, got {other:?}"),
    }
}

#[test]
fn format_of_an_option_field_with_option_caption_ml_is_refused() {
    refused(al_testkit::run_in(SRC, "C", "Fmt"));
    assert_eq!(al_testkit::text(al_testkit::run_in(SRC, "C", "PlainFmt")), "B");
}

#[test]
fn field_ref_option_caption_of_a_field_with_option_caption_ml_is_refused() {
    refused(al_testkit::run_in(SRC, "C", "Cap"));
}
