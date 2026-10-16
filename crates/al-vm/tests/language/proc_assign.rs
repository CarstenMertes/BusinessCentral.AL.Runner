//! `Proc := value` for a procedure with one value parameter is the call `Proc(value)`. Microsoft's Service Line
//! writes `SetHideWarrantyWarning := true;` (Tests-SCM T204_PostedServiceInvoiceContainsPriceCalcMethod reaches it);
//! 179 Tests-ERM tests were refused with "is not assignable". Not pinned by the corpus (docs/probes-needed.md).

use al_testkit::{int, run_in};

const SRC: &str = r#"
codeunit 69810 PA
{
    var Hidden: Boolean;
        Count: Integer;
    procedure SetHidden(Value: Boolean)
    begin
        Hidden := Value;
        Count += 1;
    end;
    procedure F(): Integer
    begin
        SetHidden := true;
        if Hidden then
            exit(Count);
        exit(-1);
    end;
}
"#;

#[test]
fn assigning_to_a_one_parameter_procedure_calls_it() {
    assert_eq!(int(run_in(SRC, "PA", "F")), 1);
}
