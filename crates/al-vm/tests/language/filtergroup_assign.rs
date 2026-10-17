//! `Rec.FilterGroup := 2; SetRange(..); Rec.FilterGroup := 0; GetRangeMax(..)` through a `var` record parameter reads the
//! filter set in group 2 (Base Application's Req. Jnl. Management WkshTemplateSelection and OpenJnl; assumption
//! get-range-across-filter-groups; Tests-SCM-Planning OpenPlanningWorksheetOnRequisitionLine).

use al_testkit::text;

const SRC: &str = r#"
table 69740 "FGA Row" { fields { field(1; Tpl; Code[10]) { } field(2; Id; Integer) { } } keys { key(PK; Tpl, Id) { Clustered = true; } } }
codeunit 69741 C
{
    local procedure Select(var R: Record "FGA Row")
    begin
        R.FilterGroup := 2;
        R.SetRange(Tpl, 'ABC');
        R.FilterGroup := 0;
    end;

    procedure F(): Text
    var R: Record "FGA Row" temporary;
    begin
        Select(R);
        exit(R.GetRangeMax(Tpl));
    end;
}
"#;

#[test]
fn a_filter_set_in_group_2_through_a_var_parameter_is_seen_by_getrangemax() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "ABC");
}
