//! An array passed to a parameter declared with more elements than the argument has (Tests-ERM
//! codeunit 134831 passes an `array[2]` variable to `var DimensionValue: array[9]`) panicked in the
//! interpreter (issue 18). BC raises `Index out of bounds.` (code IndexOutOfBounds) for an element beyond the argument
//! (misc/lt-probes-needed-5/PnE01Arrays.al:SmallerArray_VarParameter, SmallerArray_ByValue, Integer arrays), with the elements
//! that exist reading and writing as before. `ArrayLen` inside the procedure is the
//! argument's length (types/W7VarArray.al), so a loop up to `ArrayLen` stays within it.

use al_test_cli::report::Outcome;

const OBJECTS: &str = r#"
table 62700 "AS Row"
{
    fields { field(1; "No."; Code[20]) { } field(2; N; Integer) { } }
    keys { key(PK; "No.") { } }
}
codeunit 62702 "AS Helper"
{
    procedure FillNine(var Rows: array[9] of Record "AS Row") var i: Integer; begin for i := 1 to 9 do Rows[i].N := i; end;
    procedure FillTwo(var Rows: array[2] of Record "AS Row") var i: Integer; begin for i := 1 to ArrayLen(Rows) do Rows[i].N := i; end;
    procedure FillIntegers(var Vals: array[5] of Integer) begin Vals[1] := 1; Vals[5] := 5; end;
}
"#;

fn outcomes(tests: &str) -> Vec<(String, Outcome)> {
    let src = format!("{OBJECTS}\ncodeunit 62701 T {{ Subtype = Test; {tests} }}");
    al_testkit::outcomes_in(&src, "T")
}

#[test]
fn an_element_beyond_the_argument_raises_the_index_error_not_a_panic() {
    let o = outcomes(
        "[Test] procedure TooSmall() var Rows: array[2] of Record \"AS Row\"; H: Codeunit \"AS Helper\"; begin H.FillNine(Rows); end;
         [Test] procedure TooSmallIntegers() var Vals: array[2] of Integer; H: Codeunit \"AS Helper\"; begin H.FillIntegers(Vals); end;
         [Test] procedure SameSize() var Rows: array[2] of Record \"AS Row\"; H: Codeunit \"AS Helper\"; begin
             H.FillTwo(Rows); if (Rows[1].N <> 1) or (Rows[2].N <> 2) then Error('values'); end;",
    );
    for (name, outcome) in &o[..2] {
        assert!(
            matches!(outcome, Outcome::Fail(why) if why.contains("Index out of bounds.")),
            "{name}: {outcome:?}"
        );
    }
    assert_eq!(o[2].1, Outcome::Pass, "{:?}", o[2]);
}
