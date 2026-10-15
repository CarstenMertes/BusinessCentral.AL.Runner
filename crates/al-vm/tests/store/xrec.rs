//! The before-image of an outer `Validate` (xRec) is kept per record while its OnValidate runs. The entry must not
//! survive an error that unwinds out of the OnValidate: a later record, even one that gets the same memory address,
//! must see its own before-image (address-keys audit; corpus record/TestXRecContracts.al:133 for the nesting rule).

use al_builtins::value::Value;

#[test]
fn an_error_out_of_on_validate_leaves_no_before_image_behind() {
    let src = "table 60991 \"XR\" { fields { field(1; K; Code[10]) { } field(2; V; Integer) {
             trigger OnValidate() begin W := xRec.V; if V = 99 then Error('boom'); end; }
           field(3; W; Integer) { } } keys { key(PK; K) { } } }
         codeunit 50100 C {
           procedure Fail() var R: Record XR; begin R.K := 'A'; R.V := 7; asserterror R.Validate(V, 99); end;
           procedure Read(): Integer var R: Record XR; begin R.K := 'B'; R.V := 5; R.Validate(V, 6); exit(R.W); end;
           procedure F(): Integer var I: Integer; begin
             for I := 1 to 200 do begin Fail(); if Read() <> 5 then exit(I); end;
             exit(0); end; }";
    let v = al_testkit::run_in(src, "C", "F");
    assert!(matches!(v, Ok(Value::Integer(0))), "{v:?}");
}
