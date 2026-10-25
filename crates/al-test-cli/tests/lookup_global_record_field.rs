//! TestField.Lookup on a header control bound to a field of a global record of the page, whose OnLookup sets Text and
//! returns true and whose OnValidate saves the record and calls `CurrPage.Update(false)`: the control then reads the
//! picked value and the record holds it (al-runner issue 5843, the reporter's run on real BC 29.0.54011.55407, 2/2 tests).
//! The read after the Lookup rests on assumption testpage-lookup-update-false-refresh until the corpus pins it.

use al_test_cli::report::Outcome;

const SRC: &str = r#"
table 50110 "Repro Planner Setting 50110"
{
    fields
    {
        field(1; "Primary Key"; Code[10]) { }
        field(2; "Resource No."; Code[20]) { }
    }
    keys { key(PK; "Primary Key") { } }
}
table 50111 "Repro Resource 50111"
{
    LookupPageId = "Repro Resource List 50112";
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}
page 50112 "Repro Resource List 50112"
{
    PageType = List;
    SourceTable = "Repro Resource 50111";
    Editable = false;
    layout { area(Content) { repeater(Lines) { field("No."; Rec."No.") { } } } }
}
page 50113 "Repro Schedule 50113"
{
    PageType = List;
    SourceTable = "Repro Resource 50111";
    layout
    {
        area(Content)
        {
            group(Filters)
            {
                field(ResourceNo; Setting."Resource No.")
                {
                    Caption = 'Resource No.';
                    trigger OnLookup(var Text: Text): Boolean
                    var
                        Resource: Record "Repro Resource 50111";
                    begin
                        if Page.RunModal(Page::"Repro Resource List 50112", Resource) <> Action::LookupOK then
                            exit(false);
                        Text := Resource."No.";
                        exit(true);
                    end;
                    trigger OnValidate()
                    begin
                        Setting.Modify();
                        CurrPage.Update(false);
                    end;
                }
            }
            repeater(Lines) { field("No."; Rec."No.") { } }
        }
    }
    trigger OnOpenPage()
    begin
        if not Setting.Get() then
            Setting.Insert();
    end;
    var
        Setting: Record "Repro Planner Setting 50110";
}
codeunit 50114 T
{
    Subtype = Test;
    [Test]
    [HandlerFunctions('PickSecondResource')]
    procedure LookupShowsThePickedResource()
    var
        Schedule: TestPage "Repro Schedule 50113";
    begin
        Setup();
        Schedule.OpenEdit();
        Schedule.ResourceNo.Lookup();
        if Schedule.ResourceNo.Value() <> 'R2' then
            Error('Resource No. shows [%1] after the lookup.', Schedule.ResourceNo.Value());
        Schedule.Close();
    end;

    [Test]
    [HandlerFunctions('PickSecondResource')]
    procedure LookupSavesThePickedResource()
    var
        Setting: Record "Repro Planner Setting 50110";
        Schedule: TestPage "Repro Schedule 50113";
    begin
        Setup();
        Schedule.OpenEdit();
        Schedule.ResourceNo.Lookup();
        Schedule.Close();
        Setting.Get();
        if Setting."Resource No." <> 'R2' then
            Error('Setting holds [%1] after the lookup.', Setting."Resource No.");
    end;

    [Test]
    [HandlerFunctions('PickSecondResource')]
    procedure LookupWithoutUpdateIsRefused()
    var
        Schedule: TestPage "Repro Schedule 50115";
    begin
        Setup();
        Schedule.OpenEdit();
        Schedule.ResourceNo.Lookup();
        if Schedule.ResourceNo.Value() <> 'R2' then
            Error('Resource No. shows [%1] after the lookup.', Schedule.ResourceNo.Value());
    end;

    [ModalPageHandler]
    procedure PickSecondResource(var ResourceList: TestPage "Repro Resource List 50112")
    begin
        ResourceList.GoToKey('R2');
        ResourceList.OK().Invoke();
    end;

    local procedure Setup()
    var
        Setting: Record "Repro Planner Setting 50110";
        Resource: Record "Repro Resource 50111";
    begin
        Setting.DeleteAll();
        Resource.DeleteAll();
        Resource."No." := 'R1';
        Resource.Insert();
        Resource."No." := 'R2';
        Resource.Insert();
        Resource."No." := 'R3';
        Resource.Insert();
    end;
}
page 50115 "Repro Schedule 50115"
{
    PageType = List;
    SourceTable = "Repro Resource 50111";
    layout
    {
        area(Content)
        {
            group(Filters)
            {
                field(ResourceNo; Setting."Resource No.")
                {
                    trigger OnLookup(var Text: Text): Boolean
                    var
                        Resource: Record "Repro Resource 50111";
                    begin
                        if Page.RunModal(Page::"Repro Resource List 50112", Resource) <> Action::LookupOK then
                            exit(false);
                        Text := Resource."No.";
                        exit(true);
                    end;
                    trigger OnValidate()
                    begin
                        Setting.Modify();
                    end;
                }
            }
            repeater(Lines) { field("No."; Rec."No.") { } }
        }
    }
    trigger OnOpenPage()
    begin
        if not Setting.Get() then
            Setting.Insert();
    end;
    var
        Setting: Record "Repro Planner Setting 50110";
}
"#;

fn outcome(o: &[(String, Outcome)], name: &str) -> Outcome {
    o.iter()
        .find(|(n, _)| n == name)
        .map(|(_, o)| o.clone())
        .unwrap_or_else(|| panic!("no test {name}: {o:?}"))
}

#[test]
fn a_lookup_whose_on_validate_updates_the_page_shows_the_picked_value() {
    let o = al_testkit::outcomes_in(SRC, "T");
    assert_eq!(outcome(&o, "LookupShowsThePickedResource"), Outcome::Pass, "{o:?}");
    assert_eq!(outcome(&o, "LookupSavesThePickedResource"), Outcome::Pass, "{o:?}");
    // Without CurrPage.Update(false) in the OnValidate, whether the page refreshes after the Lookup is not pinned.
    assert!(
        matches!(outcome(&o, "LookupWithoutUpdateIsRefused"), Outcome::NotRun(_)),
        "{o:?}"
    );
}
