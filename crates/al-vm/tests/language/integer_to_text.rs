//! `Integer.ToText` and `BigInteger.ToText` write no thousands separator
//! (misc/lt-probe-requests-140/P140IntegerToText.al).

use al_builtins::native::Signal;

fn run(src: &str) -> Result<String, Signal> {
    al_testkit::run_with(src, "C", "F", vec![]).map(|(v, _)| al_testkit::text(Ok(v)))
}

#[test]
fn integer_and_big_integer_to_text_have_no_thousands_separator() {
    let src = "codeunit 50100 C { procedure F(): Text var I: Integer; B: BigInteger; R: Text; begin \
               I := -1000; R := I.ToText(); I := 1000000; R += '|' + I.ToText(); B := 2147483647; B := B * 4 + 3; R += '|' + B.ToText(); \
               GlobalLanguage(1031); I := 1234567; R += '|' + I.ToText(); GlobalLanguage(1036); R += '|' + I.ToText(); \
               exit(R); end; }";
    assert_eq!(run(src).unwrap(), "-1000|1000000|8589934591|1234567|1234567");
}

#[test]
fn to_text_of_a_large_number_in_an_unpinned_language_is_refused() {
    for body in [
        "var I: Integer; begin I := 1234; GlobalLanguage(1043); exit(I.ToText()); end;",
        "var B: BigInteger; begin B := 1234; GlobalLanguage(1031); exit(B.ToText()); end;",
    ] {
        let src = format!("codeunit 50100 C {{ procedure F(): Text {body} }}");
        match run(&src) {
            Err(Signal::Unsupported(_)) => {}
            other => panic!("{body}: {other:?}"),
        }
    }
}
