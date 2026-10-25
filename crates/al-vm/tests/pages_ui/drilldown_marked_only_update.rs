//! A field's OnDrillDown sets marks and MarkedOnly on Rec and calls `CurrPage.Update(false)`: the page then lists only the
//! marked row, whether or not the page declares OnAfterGetRecord and OnAfterGetCurrRecord (al-runner issue 5844, the
//! reporter's run on real BC 29.0.54011.55407, 4/4 tests). Registered assumption drilldown-marked-only-update-refresh until the
//! corpus pins it.

use al_builtins::assumptions::{Assumption, clear_hits, take_hits};
use al_testkit::{Built, boolean, text};

fn page(id: u32, triggers: &str) -> String {
    format!(
        r#"
page {id} "Repro Operations {id}"
{{
    PageType = Worksheet;
    SourceTable = "Repro Operation 50130";
    InsertAllowed = false;
    DeleteAllowed = false;
    layout
    {{
        area(Content)
        {{
            group(Tiles)
            {{
                field(MarkedCount; MarkedCount)
                {{
                    Caption = 'Operation 10';
                    Editable = false;
                    trigger OnDrillDown()
                    begin
                        Rec.ClearMarks();
                        if Rec.Get('10') then
                            Rec.Mark(true);
                        Rec.MarkedOnly(true);
                        CurrPage.Update(false);
                    end;
                }}
            }}
            repeater(Lines)
            {{
                field("Operation No."; Rec."Operation No.") {{ }}
            }}
        }}
    }}
    {triggers}
    var
        MarkedCount: Integer;
}}
"#
    )
}

fn src() -> String {
    let both = "trigger OnAfterGetRecord() begin end; trigger OnAfterGetCurrRecord() begin end;";
    let mut s = String::from(
        r#"
table 50130 "Repro Operation 50130"
{
    fields { field(1; "Operation No."; Code[10]) { } }
    keys { key(PK; "Operation No.") { } }
}
"#,
    );
    s += &page(50131, both);
    s += &page(50133, "");
    s += &page(50134, "trigger OnAfterGetRecord() begin end;");
    s += r#"
codeunit 50135 T
{
    local procedure Setup()
    var Operation: Record "Repro Operation 50130";
    begin
        Operation.DeleteAll();
        Operation."Operation No." := '10';
        Operation.Insert();
        Operation."Operation No." := '20';
        Operation.Insert();
    end;

    procedure GoTo50131(): Boolean
    var Operation20: Record "Repro Operation 50130"; Operations: TestPage "Repro Operations 50131";
    begin
        Setup();
        Operation20.Get('20');
        Operations.OpenEdit();
        if not Operations.GoToRecord(Operation20) then
            Error('Operation 20 is not listed before the drilldown.');
        Operations.MarkedCount.Drilldown();
        exit(Operations.GoToRecord(Operation20));
    end;

    procedure List50131(): Text
    var Operations: TestPage "Repro Operations 50131"; Listed: Text;
    begin
        Setup();
        Operations.OpenEdit();
        Operations.MarkedCount.Drilldown();
        if Operations.First() then
            repeat
                Listed += ' ' + Operations."Operation No.".Value();
            until not Operations.Next();
        exit(Listed);
    end;

    procedure List50133(): Text
    var Operation20: Record "Repro Operation 50130"; Operations: TestPage "Repro Operations 50133"; Listed: Text;
    begin
        Setup();
        Operation20.Get('20');
        Operations.OpenEdit();
        Operations.MarkedCount.Drilldown();
        if Operations.First() then
            repeat
                Listed += ' ' + Operations."Operation No.".Value();
            until not Operations.Next();
        if Operations.GoToRecord(Operation20) then
            Listed += ' goto20';
        exit(Listed);
    end;

    procedure List50134(): Text
    var Operations: TestPage "Repro Operations 50134"; Listed: Text;
    begin
        Setup();
        Operations.OpenEdit();
        Operations.MarkedCount.Drilldown();
        if Operations.First() then
            repeat
                Listed += ' ' + Operations."Operation No.".Value();
            until not Operations.Next();
        exit(Listed);
    end;
}
"#;
    s
}

// Reporter tests GoToRecordAfterDrilldownMissesTheUnmarkedRow and ListAfterDrilldownShowsOnlyTheMarkedRow (page 50131).
#[test]
fn a_page_with_both_get_record_triggers_lists_only_the_marked_row_after_the_drilldown() {
    let mut b = Built::from_source(&src());
    clear_hits();
    assert!(!boolean(b.run_in("T", "GoTo50131")));
    assert!(take_hits().contains(&Assumption::DrilldownMarkedOnlyUpdateRefresh));
    assert_eq!(text(b.run_in("T", "List50131")), " 10");
}

// Reporter test OnAfterGetRecordOnlyListShowsOnlyTheMarkedRow (page 50134).
#[test]
fn a_page_with_on_after_get_record_lists_only_the_marked_row_after_the_drilldown() {
    let mut b = Built::from_source(&src());
    assert_eq!(text(b.run_in("T", "List50134")), " 10");
}

// Reporter test ControlWithoutTriggersShowsOnlyTheMarkedRow (page 50133): passed before.
#[test]
fn a_page_without_get_record_triggers_lists_only_the_marked_row_after_the_drilldown() {
    let mut b = Built::from_source(&src());
    assert_eq!(text(b.run_in("T", "List50133")), " 10");
}
