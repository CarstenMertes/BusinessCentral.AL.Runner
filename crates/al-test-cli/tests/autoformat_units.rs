//! A decimal control whose table field carries `AutoFormatType` 2 or 5 (the unit amount formats): the
//! decimals come from General Ledger Setup ("Unit-Amount Decimal Places", 2:5 by default) or the
//! currency, through Base Application's `Amount AutoFormat` codeunit. The runtime does not call it
//! (issue 17); it spelled such a value with two places, which is wrong for 110.74398.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

const OBJECTS: &str = r#"
table 62500 "AF Row"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amt; Decimal) { AutoFormatType = 1; AutoFormatExpression = ''; }
        field(3; Unit; Decimal) { AutoFormatType = 2; AutoFormatExpression = ''; }
        field(4; UnitNoCur; Decimal) { AutoFormatType = 5; AutoFormatExpression = ''; }
        field(5; Plain; Decimal) { }
    }
    keys { key(PK; "No.") { } }
}
page 62500 "AF Card"
{
    PageType = Card; SourceTable = "AF Row";
    layout { area(Content) {
        field(AmtCtl; Rec.Amt) { }
        field(UnitCtl; Rec.Unit) { }
        field(UnitNoCurCtl; Rec.UnitNoCur) { }
        field(PlainCtl; Rec.Plain) { }
    } }
}
"#;

fn outcomes(tests: &str) -> Vec<(String, Outcome)> {
    let src = format!("{OBJECTS}\ncodeunit 62501 T {{ Subtype = Test; {tests} }}");
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

const SEED: &str = "R: Record \"AF Row\"; P: TestPage \"AF Card\";";

fn open(values: &str) -> String {
    format!("R.\"No.\" := 'X'; {values} R.Insert(); P.OpenView(); P.GoToRecord(R);")
}

// autoformat/TestAutoFormat.al: AutoFormatType = 1 with a blank expression keeps two decimals, and
// 1234.5 reads 1,234.50 whatever the type.
#[test]
fn values_with_at_most_two_decimals_read_as_before() {
    let o = outcomes(&format!(
        "[Test] procedure Reads() var {SEED} begin {} \
         if P.AmtCtl.Value() <> '110.74' then Error('amt %1', P.AmtCtl.Value());
         if P.UnitCtl.Value() <> '1,234.50' then Error('unit %1', P.UnitCtl.Value());
         if P.UnitNoCurCtl.Value() <> '3.00' then Error('unit no cur %1', P.UnitNoCurCtl.Value()); end;",
        open("R.Amt := 110.74; R.Unit := 1234.5; R.UnitNoCur := 3;")
    ));
    assert_eq!(o[0].1, Outcome::Pass, "{o:?}");
}

#[test]
fn a_unit_amount_with_more_than_two_decimals_is_refused_not_rounded() {
    let o = outcomes(&format!(
        "[Test] procedure Unit() var {SEED} begin {} if P.UnitCtl.Value() = '' then; end;
         [Test] procedure UnitNoCurrency() var {SEED} begin {} if P.UnitNoCurCtl.Value() = '' then; end;
         [Test] procedure AmountAndPlainKeepTheirTwoDecimals() var {SEED} begin {} \
         if P.PlainCtl.Value() <> '110.74' then Error('plain %1', P.PlainCtl.Value()); end;",
        open("R.Unit := 110.74398;"),
        open("R.UnitNoCur := 682.12312;"),
        open("R.Plain := 110.74398;")
    ));
    for (name, outcome) in &o[..2] {
        assert!(
            matches!(outcome, Outcome::NotRun(why) if why.contains("AutoFormatType") && why.contains("not pinned")),
            "{name}: {outcome:?}"
        );
    }
    assert_eq!(o[2].1, Outcome::Pass, "{o:?}");
}
