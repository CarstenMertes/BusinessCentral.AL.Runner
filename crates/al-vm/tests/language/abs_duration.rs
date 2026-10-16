//! `Abs` of a Duration takes the Decimal overload: the Duration counts as its milliseconds and the answer is a Decimal
//! (Manufacturing code compares `Abs(StartDateTime - EndDateTime)`; Tests-SCM-Manufacturing, 218 tests were refused with
//! "Abs of Duration"; assumption abs-of-duration-is-decimal).

use al_testkit::text;

const SRC: &str = r#"
codeunit 69750 C
{
    procedure F(): Text
    var D: Duration; X: Decimal; Back: Duration;
    begin
        D := -5000;
        X := Abs(D);
        Back := Abs(D);
        exit(Format(X) + ';' + Format(Back) + ';' + Format(Abs(D) > 4999));
    end;
}
"#;

#[test]
fn abs_of_a_duration_is_its_milliseconds_as_a_decimal() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "5,000;5 seconds;Yes");
}
