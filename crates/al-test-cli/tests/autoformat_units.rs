//! A decimal control with `AutoFormatType` 1, 2, 4 or 5 (on the control or on the table field). Pinned by
//! autoformat/PWAutoFormatTypes.al (BC 27.5): types 1 and 4 are amounts, 2 and 5 unit amounts. With no
//! currency (a blank expression) an amount has 2:2 places and a unit amount 2:5, whatever General Ledger
//! Setup says (:98-128, :131-175). A currency code that exists gives the places of the currency, its
//! "Amount Decimal Places" for the amounts and its "Unit-Amount Decimal Places" for the unit amounts
//! (:177-195); a currency with both places empty gives 2:2 for all four (:197-217); a code that is no
//! currency gives 2:5 for all four (:263-290).

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
    al_testkit::outcomes_in(&src, "T")
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

// :98-128: a unit amount with a blank expression keeps up to five decimals, an amount two; a plain control
// two (autoformat/TestAutoFormat.al).
#[test]
fn a_unit_amount_without_a_currency_has_two_to_five_places() {
    let o = outcomes(&format!(
        "[Test] procedure Unit() var {SEED} begin {} \
         if P.UnitCtl.Value() <> '110.74398' then Error('unit %1', P.UnitCtl.Value());
         if P.UnitNoCurCtl.Value() <> '682.12312' then Error('unit no cur %1', P.UnitNoCurCtl.Value());
         if P.AmtCtl.Value() <> '110.74' then Error('amt %1', P.AmtCtl.Value());
         if P.PlainCtl.Value() <> '110.74' then Error('plain %1', P.PlainCtl.Value()); end;",
        open("R.Unit := 110.74398; R.UnitNoCur := 682.12312; R.Amt := 110.74398; R.Plain := 110.74398;")
    ));
    assert_eq!(o[0].1, Outcome::Pass, "{o:?}");
}

const CURRENCY: &str = r#"
table 4 Currency
{
    fields
    {
        field(1; Code; Code[10]) { }
        field(15; Symbol; Text[10]) { }
        field(18; "Amount Decimal Places"; Text[5]) { }
        field(19; "Unit-Amount Decimal Places"; Text[5]) { }
    }
    keys { key(PK; Code) { } }
}
table 62510 "AF Cur Row"
{
    fields { field(1; "No."; Code[20]) { } field(2; Amount; Decimal) { } field(3; "Currency Code"; Code[10]) { } }
    keys { key(PK; "No.") { } }
}
page 62510 "AF Cur Card"
{
    PageType = Card; SourceTable = "AF Cur Row";
    layout { area(Content) {
        field(T1; Rec.Amount) { AutoFormatType = 1; AutoFormatExpression = Rec."Currency Code"; }
        field(T2; Rec.Amount) { AutoFormatType = 2; AutoFormatExpression = Rec."Currency Code"; }
        field(T4; Rec.Amount) { AutoFormatType = 4; AutoFormatExpression = Rec."Currency Code"; }
        field(T5; Rec.Amount) { AutoFormatType = 5; AutoFormatExpression = Rec."Currency Code"; }
        field(T1Blank; Rec.Amount) { AutoFormatType = 1; AutoFormatExpression = ''; }
        field(T2Blank; Rec.Amount) { AutoFormatType = 2; AutoFormatExpression = ''; }
    } }
}
"#;

fn currency_outcomes(tests: &str) -> Vec<(String, Outcome)> {
    let src = format!("{OBJECTS}{CURRENCY}\ncodeunit 62511 T {{ Subtype = Test; {tests} }}");
    let l = al_testkit::compile(&src);
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let engine = Engine::new(l.program.clone(), blocked);
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    run_codeunit(&engine, &l, oi)
}

/// A test that seeds the currency `Code` with the places, reads the page of a row with value `v`, and checks the
/// six controls (t1, t2, t4, t5, t1 blank, t2 blank).
fn arm(code: &str, amount_places: &str, unit_places: &str, row_currency: &str, v: &str, want: [&str; 6]) -> String {
    format!(
        "[Test] procedure A{code}{row_currency}() var C: Record Currency; R: Record \"AF Cur Row\"; P: TestPage \"AF Cur Card\"; begin
           C.Code := '{code}'; C.\"Amount Decimal Places\" := '{amount_places}'; C.\"Unit-Amount Decimal Places\" := '{unit_places}'; C.Insert();
           R.\"No.\" := 'X{code}'; R.Amount := {v}; R.\"Currency Code\" := '{row_currency}'; R.Insert(); P.OpenView(); P.GoToRecord(R);
           if P.T1.Value() <> '{}' then Error('t1 %1', P.T1.Value());
           if P.T2.Value() <> '{}' then Error('t2 %1', P.T2.Value());
           if P.T4.Value() <> '{}' then Error('t4 %1', P.T4.Value());
           if P.T5.Value() <> '{}' then Error('t5 %1', P.T5.Value());
           if P.T1Blank.Value() <> '{}' then Error('t1 blank %1', P.T1Blank.Value());
           if P.T2Blank.Value() <> '{}' then Error('t2 blank %1', P.T2Blank.Value()); end;",
        want[0], want[1], want[2], want[3], want[4], want[5]
    )
}

// :177-195 (cur_never): the currency PWC has amount places 3:3 and unit places 4:4.
#[test]
fn a_currency_gives_its_own_amount_and_unit_places() {
    let o = currency_outcomes(&format!(
        "{}{}",
        arm(
            "PWC",
            "3:3",
            "4:4",
            "PWC",
            "110.74398",
            ["110.744", "110.7440", "110.744", "110.7440", "110.74", "110.74398"]
        ),
        arm(
            "PWF",
            "3:3",
            "4:4",
            "PWF",
            "1234.5",
            [
                "1,234.500",
                "1,234.5000",
                "1,234.500",
                "1,234.5000",
                "1,234.50",
                "1,234.50"
            ]
        )
    ));
    for (n, r) in &o {
        assert_eq!(r, &Outcome::Pass, "{n}: {o:?}");
    }
}

// :197-217 (cur_empty_fcy_symbol): a currency with both places empty reads 2:2 for all four types.
#[test]
fn a_currency_without_places_reads_two_places() {
    let o = currency_outcomes(&arm(
        "PWD",
        "",
        "",
        "PWD",
        "110.74398",
        ["110.74", "110.74", "110.74", "110.74", "110.74", "110.74398"],
    ));
    assert_eq!(o[0].1, Outcome::Pass, "{o:?}");
}

// :263-290 (cur_unknown): a code that is no currency reads 2:5 for all four types.
#[test]
fn a_code_that_is_no_currency_reads_two_to_five_places() {
    let o = currency_outcomes(&arm(
        "PWE",
        "3:3",
        "4:4",
        "NOPE",
        "110.74398",
        [
            "110.74398",
            "110.74398",
            "110.74398",
            "110.74398",
            "110.74",
            "110.74398",
        ],
    ));
    assert_eq!(o[0].1, Outcome::Pass, "{o:?}");
}
