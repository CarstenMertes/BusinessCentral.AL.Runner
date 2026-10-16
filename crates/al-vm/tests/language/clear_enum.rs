//! `Clear` of an Enum variable or field gives the enum's initial value, ordinal 0 (Base Application's Requisition Line
//! does `Clear("Ref. Order Status")` in `SetReplenishmentSystemFromPurchase`; 385 tests of the Planning bucket were
//! refused with "Clear of a Enum value"). The initial value is the one an unassigned variable has.

use al_testkit::text;

const SRC: &str = r#"
enum 69700 "CE Kind" { value(0; A) { } value(1; B) { } value(5; C) { } }
table 69701 "CE Row" { fields { field(1; Id; Integer) { } field(2; Kind; Enum "CE Kind") { } } keys { key(PK; Id) { } } }
codeunit 69702 C
{
    procedure F(): Text
    var K: Enum "CE Kind"; R: Record "CE Row"; T: Text;
    begin
        K := K::C;
        Clear(K);
        T += Format(K) + ';' + Format(K = K::A) + ';';
        R.Kind := R.Kind::B;
        Clear(R.Kind);
        T += Format(R.Kind) + ';';
        exit(T);
    end;
}
"#;

#[test]
fn clearing_an_enum_gives_its_initial_value() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "A;Yes;A;");
}
