//! An action that puts another row (another primary key) into Rec with changed fields: whether the client saves that row, the row
//! it showed, or nothing is not pinned (the pinned actions change the shown row: record/PWTableGlobals.al:408-431). Refused.
//! Tests-ERM 134918 CustApplyAndPostPaymentInv: Apply Customer Entries' Post Application sets `Rec := TempApplyingCustLedgEntry`.

use al_builtins::native::Signal;

const SRC: &str = r#"
table 51490 "MR Row" { fields { field(1; Id; Integer) { } field(2; Amt; Decimal) { } } keys { key(PK; Id) { } } }
page 51491 "MR List"
{
    PageType = List;
    SourceTable = "MR Row";
    layout { area(Content) { repeater(R) { field(A; Rec.Amt) { } } } }
    actions
    {
        area(Processing)
        {
            action(Move)
            {
                trigger OnAction()
                var Other: Record "MR Row";
                begin
                    Other.Get(2); Other.Amt := 99; Rec := Other;
                end;
            }
        }
    }
}
codeunit 51492 "MR Test"
{
    procedure Run(): Text
    var R: Record "MR Row"; P: TestPage "MR List";
    begin
        R.Id := 1; R.Insert(); R.Id := 2; R.Insert();
        P.OpenEdit();
        P.First();
        P.Move.Invoke();
        exit('ran');
    end;
}
"#;

#[test]
fn an_action_that_leaves_another_changed_row_in_rec_is_refused() {
    match al_testkit::run_in(SRC, "MR Test", "Run") {
        Err(Signal::Unsupported(why)) => assert!(why.contains("another primary key"), "{why}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}
