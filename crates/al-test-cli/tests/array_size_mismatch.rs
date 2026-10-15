//! An array passed to a parameter declared with more elements than the argument has (Tests-ERM
//! codeunit 134831 passes an `array[2]` variable to `var DimensionValue: array[9]`) panicked in the
//! interpreter (issue 18). What BC does is not pinned by the corpus, so an element beyond the argument is
//! refused, with the elements that exist reading and writing as before. `ArrayLen` inside the procedure is the
//! argument's length (types/W7VarArray.al), so a loop up to `ArrayLen` stays within it.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

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
        .collect();
    let engine = Engine::new(l.program.clone(), blocked);
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    run_codeunit(&engine, &l, oi)
}

#[test]
fn an_element_beyond_the_argument_is_refused_not_a_panic() {
    let o = outcomes(
        "[Test] procedure TooSmall() var Rows: array[2] of Record \"AS Row\"; H: Codeunit \"AS Helper\"; begin H.FillNine(Rows); end;
         [Test] procedure TooSmallIntegers() var Vals: array[2] of Integer; H: Codeunit \"AS Helper\"; begin H.FillIntegers(Vals); end;
         [Test] procedure SameSize() var Rows: array[2] of Record \"AS Row\"; H: Codeunit \"AS Helper\"; begin
             H.FillTwo(Rows); if (Rows[1].N <> 1) or (Rows[2].N <> 2) then Error('values'); end;",
    );
    for (name, outcome) in &o[..2] {
        assert!(
            matches!(outcome, Outcome::NotRun(why) if why.contains("array") && why.contains("not pinned") && !why.contains("panic")),
            "{name}: {outcome:?}"
        );
    }
    assert_eq!(o[2].1, Outcome::Pass, "{:?}", o[2]);
}
