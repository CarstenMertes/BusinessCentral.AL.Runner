//! A TestPage variable that trapped a page and was left open traps the next run again. Library - Error Message keeps one
//! `TestPage "Error Messages"` in a codeunit variable, traps it in every `VerifyDimErrors` and never closes it; the next test of
//! the codeunit traps it again and `Error Message.ShowErrors` runs the page (Tests-Misc Check Dimensions On Posting, 35 tests). What
//! BC does with the page left open is not probed: the new page replaces it (assumption `trapped-testpage-left-open-is-replaced`).

use al_testkit::text;

const SRC: &str = r#"
table 69980 "TR Row" { fields { field(1; K; Code[10]) { } } keys { key(PK; K) { } } }
page 69981 "TR List" { PageType = List; SourceTable = "TR Row";
  layout { area(content) { repeater(R) { field(KCtl; Rec.K) { } } } } }
codeunit 69982 C
{
    procedure F(): Text
    var R: Record "TR Row"; TP: TestPage "TR List"; S: Text;
    begin
        R.K := '1'; R.Insert();
        R.K := '2'; R.Insert();
        R.SetRange(K, '1');
        TP.Trap();
        Page.Run(Page::"TR List", R);
        S := TP.KCtl.Value();
        R.SetRange(K, '2');
        TP.Trap();
        Page.Run(Page::"TR List", R);
        exit(S + TP.KCtl.Value());
    end;
}
"#;

#[test]
fn a_trapped_page_left_open_is_replaced_by_the_next_trapped_run() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "12");
}
