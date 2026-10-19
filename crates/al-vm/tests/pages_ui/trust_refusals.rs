//! Page behaviors the test client does not implement are refused, never run as if the property were not there
//! (hard rule 4). Each test names the corpus test that pins what BC does.

use al_builtins::native::Signal;
use al_builtins::value::Value;

fn refused(r: Result<Value, Signal>, needle: &str) {
    match r {
        Err(Signal::Unsupported(why)) => assert!(why.contains(needle), "{why}"),
        other => panic!("expected a refusal naming {needle}, got {other:?}"),
    }
}

/// pageext/lt-page-ext-properties/PageFieldValuesAllowed.al:64-73: a value outside the control's ValuesAllowed is rejected.
#[test]
fn set_value_on_a_control_with_values_allowed_is_refused() {
    let src = r#"
table 69990 "VA Row" { fields { field(1; Id; Integer) { } field(2; Kind; Option) { OptionMembers = Open,Released,Closed; } } keys { key(PK; Id) { } } }
page 69991 "VA Card"
{
    PageType = Card; SourceTable = "VA Row"; ApplicationArea = All; UsageCategory = None;
    layout { area(Content) { field(Kind; Rec.Kind) { ValuesAllowed = Open, Released; } field(Id; Rec.Id) { } } }
}
codeunit 69992 C
{
    procedure F(): Text
    var R: Record "VA Row"; P: TestPage "VA Card";
    begin
        R.Id := 1; R.Insert();
        P.OpenEdit();
        P.Kind.SetValue('Closed');
        exit(P.Kind.Value());
    end;
    procedure G(): Text
    var R: Record "VA Row"; P: TestPage "VA Card";
    begin
        R.Id := 1; R.Insert();
        P.OpenEdit();
        exit(P.Kind.Value());
    end;
}
"#;
    refused(al_testkit::run_in(src, "C", "F"), "ValuesAllowed");
    assert_eq!(al_testkit::text(al_testkit::run_in(src, "C", "G")), "Open");
}

/// pages/lt-page-presentation-properties/P119LabelHideValue.al:74: `HideValue = <variable>` hides the value, so HideValue is
/// evaluated like Visible, Editable and Enabled. Another boolean property written as an expression is refused, not read as false.
#[test]
fn a_boolean_control_property_written_as_an_expression_is_evaluated_or_refused() {
    let src = r#"
table 69993 "HV Row" { fields { field(1; Id; Integer) { } field(2; Name; Text[20]) { } } keys { key(PK; Id) { } } }
page 69994 "HV Card"
{
    PageType = Card; SourceTable = "HV Row"; ApplicationArea = All; UsageCategory = None;
    layout { area(Content) { field(NameF; Rec.Name) { HideValue = HideIt; ShowMandatory = MandIt; } field(Plain; Rec.Name) { HideValue = true; } } }
    var HideIt: Boolean; MandIt: Boolean;
    trigger OnOpenPage() begin HideIt := true; MandIt := true; end;
}
codeunit 69995 C
{
    procedure F(): Text
    var R: Record "HV Row"; P: TestPage "HV Card";
    begin
        R.Id := 1; R.Name := 'Shown?'; R.Insert();
        P.OpenView();
        exit('[' + P.NameF.Value() + ']');
    end;
    procedure G(): Text
    var R: Record "HV Row"; P: TestPage "HV Card";
    begin
        R.Id := 1; R.Name := 'Shown?'; R.Insert();
        P.OpenView();
        exit(Format(P.Plain.HideValue()));
    end;
    procedure H(): Text
    var R: Record "HV Row"; P: TestPage "HV Card";
    begin
        R.Id := 1; R.Name := 'Shown?'; R.Insert();
        P.OpenView();
        exit(Format(P.NameF.ShowMandatory()));
    end;
}
"#;
    assert_eq!(al_testkit::text(al_testkit::run_in(src, "C", "F")), "[]");
    assert_eq!(al_testkit::text(al_testkit::run_in(src, "C", "G")), "Yes");
    refused(al_testkit::run_in(src, "C", "H"), "showmandatory");
}

/// record/lt-x-table-props/XTablePropsTests.al:229-230: a page control shows the session language's entry of its table
/// field's OptionCaptionML (`deu_page_op` is `Zwei`). The client does not read it and refuses.
#[test]
fn a_page_control_on_a_field_with_option_caption_ml_is_refused() {
    let src = r#"
table 69996 "OM Row" { fields { field(1; Id; Integer) { } field(2; Op; Option) { OptionMembers = A,B,C; OptionCaptionML = ENU = 'Alpha,Beta,Gamma', DEU = 'Eins,Zwei,Drei'; } } keys { key(PK; Id) { } } }
page 69997 "OM Card"
{
    PageType = Card; SourceTable = "OM Row"; ApplicationArea = All; UsageCategory = None;
    layout { area(Content) { field(Op; Rec.Op) { } } }
}
codeunit 69998 C
{
    procedure F(): Text
    var R: Record "OM Row"; P: TestPage "OM Card";
    begin
        R.Id := 1; R.Op := R.Op::B; R.Insert();
        P.OpenView();
        exit(P.Op.Value());
    end;
}
"#;
    refused(al_testkit::run_in(src, "C", "F"), "OptionCaptionML");
}

/// pages/lt-ah-pages/AhPageTests.al:986-989: Expand on a tree shows the rows below the current one. The client keeps the
/// rows collapsed, so Expand on a page whose repeater has ShowAsTree is refused.
#[test]
fn expand_on_a_tree_page_is_refused() {
    let src = r#"
table 69960 "TR Row" { fields { field(1; Id; Integer) { } field(2; Ind; Integer) { } } keys { key(PK; Id) { } } }
page 69961 "TR List"
{
    PageType = List; SourceTable = "TR Row"; ApplicationArea = All; UsageCategory = None;
    layout { area(Content) { repeater(R) { ShowAsTree = true; IndentationColumn = Rec.Ind; field(Id; Rec.Id) { } } } }
}
codeunit 69962 C
{
    procedure F(): Text
    var R: Record "TR Row"; P: TestPage "TR List";
    begin
        R.Id := 1; R.Ind := 0; R.Insert(); R.Id := 2; R.Ind := 1; R.Insert();
        P.OpenView();
        P.Expand(true);
        exit(P.Id.Value());
    end;
}
"#;
    refused(al_testkit::run_in(src, "C", "F"), "ShowAsTree");
}

/// pages/lt-page-editability-width-lookups/P115Lookups.al:120-122: Lookup on a control with its own TableRelation opens the
/// lookup page of that relation. The client reads only the table field's relation and refuses.
#[test]
fn lookup_through_a_control_level_table_relation_is_refused() {
    let src = r#"
table 69963 "LK Target" { fields { field(1; Code; Code[10]) { } } keys { key(PK; Code) { } } }
table 69964 "LK Row" { fields { field(1; Id; Integer) { } field(2; Ref; Code[10]) { } } keys { key(PK; Id) { } } }
page 69965 "LK Card"
{
    PageType = Card; SourceTable = "LK Row"; ApplicationArea = All; UsageCategory = None;
    layout { area(Content) { field(Ref; Rec.Ref) { TableRelation = "LK Target"; } } }
}
codeunit 69966 C
{
    procedure F(): Text
    var R: Record "LK Row"; P: TestPage "LK Card";
    begin
        R.Id := 1; R.Insert();
        P.OpenEdit();
        P.Ref.Lookup();
        exit(P.Ref.Value());
    end;
}
"#;
    refused(al_testkit::run_in(src, "C", "F"), "TableRelation");
}

/// events/lt-global-triggers-statements/P114GlobalTriggerEvents.al:301, :323: every caption class translation raises
/// CaptionClassTranslate of UI Helper Triggers. The runtime does not raise it and refuses while a subscriber listens.
#[test]
fn a_caption_class_translation_with_a_ui_helper_subscriber_is_refused() {
    let src = r#"
codeunit 69967 Subs
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"UI Helper Triggers", 'CaptionClassTranslate', '', false, false)]
    local procedure T(Language: Integer; CaptionExpr: Text[1024]; var Translation: Text[1024]) begin end;
}
codeunit 69968 C
{
    procedure F(): Text begin exit(CaptionClassTranslate('3,Foo')); end;
}
"#;
    refused(al_testkit::run_in(src, "C", "F"), "CaptionClassTranslate");
}

const FIND_LIST: &str = r#"
table 69950 "FL Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
page 69951 "FL List"
{
    PageType = List; SourceTable = "FL Row"; ApplicationArea = All; UsageCategory = None; Editable = false;
    layout { area(Content) { repeater(R) { field(Id; Rec.Id) { } } } }
    trigger OnOpenPage() begin end;
    trigger OnFindRecord(Which: Text): Boolean begin exit(Rec.Find(Which)); end;
    trigger OnNextRecord(Steps: Integer): Integer begin exit(Rec.Next(Steps)); end;
    trigger OnAfterGetRecord() begin end;
    trigger OnAfterGetCurrRecord() begin end;
}
codeunit 69952 C
{
    local procedure Seed(N: Integer)
    var R: Record "FL Row"; i: Integer;
    begin
        for i := 1 to N do begin R.Id := i; R.Insert(); end;
    end;
    procedure Descending(): Text
    var P: TestPage "FL List";
    begin
        Seed(3);
        P.OpenView();
        exit(P.Id.Value());
    end;
    procedure GoToKeyFar(): Text
    var P: TestPage "FL List";
    begin
        Seed(25);
        P.OpenView();
        P.GoToKey(22);
        exit(P.Id.Value());
    end;
}
"#;

/// misc/lt-probes-needed-6/PnF06FindListMoves.al:304, :318: a descending OnFindRecord list with OnAfterGetCurrRecord steps
/// back over every row it served when it opens. That walk is not implemented, so the open is refused.
#[test]
fn opening_a_descending_find_list_with_on_after_get_curr_record_is_refused() {
    let src = FIND_LIST.replace(
        "trigger OnOpenPage() begin end;",
        "trigger OnOpenPage() begin Rec.Ascending(false); end;",
    );
    refused(al_testkit::run_in(&src, "C", "Descending"), "descending");
}

/// pages/lt-ai-list-order/AiListOrderTests.al:999, :1020: GoToKey on an OnFindRecord list with a full window of 20 rows makes
/// the client ask the page for the rows around the key. That is not implemented, so it is refused.
#[test]
fn go_to_key_on_a_find_list_with_a_full_window_is_refused() {
    refused(al_testkit::run_in(FIND_LIST, "C", "GoToKeyFar"), "full window");
}
