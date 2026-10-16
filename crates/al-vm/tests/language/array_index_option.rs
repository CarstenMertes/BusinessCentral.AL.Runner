//! An array indexed by an Option member: the member's ordinal is the index. Base Application's
//! `ServContractManagement.SetServLedgEntryAmounts` does `EntryAmount[AmountType::Amount] -= ...` with
//! `AmountType: Option ,Amount,DiscAmount,UnitPrice,UnitCost` (207 tests of Tests-SCM-Service were refused with
//! "Option member 'Amount' used as an integer"). Assumption `array-index-by-option`.

use al_testkit::text;

const SRC: &str = r#"
codeunit 69710 C
{
    var AmountType: Option ,Amount,DiscAmount,UnitPrice,UnitCost;
    procedure F(): Text
    var A: array[4] of Decimal;
    begin
        A[AmountType::UnitPrice] += 5;
        A[AmountType::Amount] -= 2;
        exit(Format(A[1]) + ';' + Format(A[2]) + ';' + Format(A[3]) + ';' + Format(A[AmountType::UnitPrice]));
    end;
}
"#;

#[test]
fn an_option_member_indexes_an_array_by_its_ordinal() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "-2;0;5;5");
}
