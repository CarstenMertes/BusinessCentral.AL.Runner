//! A list filtered (filter group 2) to one value of a key field loses its only row to an action: whether the blank line BC shows
//! then takes the filter's value is not pinned (the pinned lists, misc/lt-probes-needed-7/PnGRefresh.al:120, have no filter), so
//! the action is refused. Tests-ERM 134002 ApplyAndPostPaymentToInvoice posts a General Journal this way.

use al_builtins::native::Signal;

const SRC: &str = r#"
table 51470 "GK Line" { fields { field(1; Batch; Code[10]) { } field(2; "Line No."; Integer) { } } keys { key(PK; Batch, "Line No.") { } } }
page 51471 "GK List"
{
    PageType = List;
    SourceTable = "GK Line";
    layout { area(Content) { repeater(R) { field(L; Rec."Line No.") { } } } }
    actions { area(Processing) { action(Post) { trigger OnAction() var L: Record "GK Line"; begin L.DeleteAll(); end; } } }
    trigger OnOpenPage() begin Rec.FilterGroup(2); Rec.SetRange(Batch, 'B1'); Rec.FilterGroup(0); end;
    trigger OnNewRecord(BelowxRec: Boolean) begin end;
}
codeunit 51472 "GK Test"
{
    procedure Run(): Text
    var L: Record "GK Line"; P: TestPage "GK List";
    begin
        L.Batch := 'B1'; L."Line No." := 10000; L.Insert();
        P.OpenEdit();
        P.Post.Invoke();
        exit('ran');
    end;
}
"#;

#[test]
fn a_key_filtered_list_that_loses_its_only_row_to_an_action_is_refused() {
    match al_testkit::run_in(SRC, "GK Test", "Run") {
        Err(Signal::Unsupported(why)) => assert!(why.contains("filtered to one key value"), "{why}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}
