//! `a and b in [..]` without parentheses (grammar/expressions.rs): the `in` belongs to the operand on its left, so
//! a Boolean and an enum value can be joined that way. Base Application's Purchase Header does it
//! (`"Self-Billing Invoice" := Vend."Self-Billing Agreement" and Rec."Document Type" in [Order, Invoice]`); no corpus
//! test pins it (docs/probes-needed.md, assumption `in-binds-tighter-than-and`).

use al_testkit::text;

const SRC: &str = r#"
enum 69500 "IP Kind" { value(0; A) { } value(1; B) { } value(2; C) { } }
codeunit 69501 C
{
    procedure F(): Text
    var Flag: Boolean; K: Enum "IP Kind"; R: Text;
    begin
        K := K::B;
        Flag := true;
        R += Format(Flag and K in [K::A, K::B]);
        K := K::C;
        R += Format(Flag and K in [K::A, K::B]);
        Flag := false;
        K := K::A;
        R += Format(Flag and K in [K::A, K::B]);
        R += Format(Flag or K in [K::A, K::B]);
        exit(R);
    end;
}
"#;

#[test]
fn an_in_after_and_belongs_to_the_right_operand() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "YesNoNoYes");
}
