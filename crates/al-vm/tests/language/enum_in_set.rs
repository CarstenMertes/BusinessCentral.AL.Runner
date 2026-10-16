//! `Enum in [Enum::"X"::A, Enum::"X"::B]` tests membership by value (Base Application's Get Demand To Reserve
//! OnPreDataItem: `if not (DemandType in [Enum::"Reservation Demand Type"::All, ...]) then CurrReport.Break()`).

use al_testkit::text;

const SRC: &str = r#"
enum 69710 "ES Kind" { value(0; A) { } value(1; B) { } value(5; C) { } }
codeunit 69711 C
{
    procedure F(): Text
    var K: Enum "ES Kind"; T: Text;
    begin
        K := K::C;
        T += Format(K in [Enum::"ES Kind"::A, Enum::"ES Kind"::B]) + ';';
        K := K::B;
        T += Format(K in [Enum::"ES Kind"::A, Enum::"ES Kind"::B]) + ';';
        exit(T);
    end;
}
"#;

#[test]
fn enum_in_a_set_of_qualified_literals() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "No;Yes;");
}
