//! What a page control shows and how TestPage compares it: AssertEquals on a Code control, ClosingDates in the
//! control text, and the caption from CaptionClass.

use al_builtins::native::Signal;
use al_testkit::text;

const CODE_SRC: &str = r#"
table 69990 "CT Row" { fields { field(1; K; Integer) { } field(2; Cde; Code[30]) { } } keys { key(PK; K) { } } }
page 69991 "CT Card"
{
    PageType = Card; SourceTable = "CT Row"; ApplicationArea = All; UsageCategory = None;
    layout { area(Content) { field(Cde; Rec.Cde) { } } }
}
codeunit 69992 C
{
    procedure Lower(): Text
    var R: Record "CT Row"; P: TestPage "CT Card";
    begin
        R.K := 1; R.Cde := 'ABC'; R.Insert();
        P.OpenEdit();
        P.Cde.AssertEquals('abc');
        exit('ok');
    end;
    procedure LowerMismatch(): Text
    var R: Record "CT Row"; P: TestPage "CT Card";
    begin
        R.K := 1; R.Cde := 'ABC'; R.Insert();
        P.OpenEdit();
        P.Cde.AssertEquals('abd');
        exit('ok');
    end;
}
"#;

#[test]
fn assert_equals_on_a_code_control_ignores_the_case_of_the_expected_text() {
    // corpus pages/lt-testpage-assertequals-option/TestPageAssertEqualsOption.al:167, :179.
    assert_eq!(text(al_testkit::run_in(CODE_SRC, "C", "Lower")), "ok");
}

#[test]
fn a_mismatch_on_a_code_control_shows_the_expected_text_as_written() {
    // corpus handlers/TestRequestPageControlErrorName.al:281, :397.
    assert_eq!(
        al_testkit::error_message(al_testkit::run_in(CODE_SRC, "C", "LowerMismatch")),
        "AssertEquals for Field: Cde Expected = 'abd', Actual = 'ABC'"
    );
}

const CLOSING_SRC: &str = r#"
table 69993 "CT Dates"
{
    fields { field(1; K; Integer) { } field(2; OnField; Date) { ClosingDates = true; } field(3; Plain; Date) { } }
    keys { key(PK; K) { } }
}
page 69994 "CT Dates Card"
{
    PageType = Card; SourceTable = "CT Dates"; ApplicationArea = All; UsageCategory = None;
    layout
    {
        area(Content)
        {
            field(OnField; Rec.OnField) { }
            field(OnControl; Rec.Plain) { ClosingDates = true; }
            field(Plain; Rec.Plain) { }
            field(OffOverField; Rec.OnField) { ClosingDates = false; }
        }
    }
}
codeunit 69995 C
{
    procedure Shown(): Text
    var R: Record "CT Dates"; P: TestPage "CT Dates Card";
    begin
        R.K := 1; R.OnField := ClosingDate(DMY2Date(31, 12, 2025)); R.Plain := R.OnField; R.Insert();
        P.OpenEdit();
        exit(P.OnField.Value() + '|' + P.OnControl.Value() + '|' + P.Plain.Value());
    end;
    procedure OffOverField(): Text
    var R: Record "CT Dates"; P: TestPage "CT Dates Card";
    begin
        R.K := 1; R.OnField := ClosingDate(DMY2Date(31, 12, 2025)); R.Insert();
        P.OpenEdit();
        exit(P.OffOverField.Value());
    end;
}
"#;

#[test]
fn a_closing_date_shows_its_mark_where_the_field_or_the_control_sets_closing_dates() {
    // corpus record/lt-x-table-props/XTablePropsTests.al:523-524, pageext/lt-z-modify-controls/ZModifyControls.al:290-291,
    // pages/lt-page-presentation-properties/P119ReportFields.al:174.
    assert_eq!(
        text(al_testkit::run_in(CLOSING_SRC, "C", "Shown")),
        "C12/31/2025|C12/31/2025|12/31/2025"
    );
}

#[test]
fn a_control_that_turns_closing_dates_off_over_its_field_is_refused() {
    let r = al_testkit::run_in(CLOSING_SRC, "C", "OffOverField");
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}

const CAPTION_SRC: &str = r#"
table 69996 "CT Cap Row"
{
    fields
    {
        field(1; K; Integer) { }
        field(2; Descr; Text[30]) { }
        field(3; Classed; Integer) { Caption = 'Plain caption'; CaptionClass = '3,Class caption'; }
    }
    keys { key(PK; K) { } }
}
codeunit 69997 "CT Cap Log"
{
    SingleInstance = true;
    var Calls: Text;
    procedure Take(): Text var R: Text; begin R := Calls; Calls := ''; exit(R); end;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Caption Class", 'OnResolveCaptionClass', '', true, true)]
    local procedure OnResolve(CaptionArea: Text; CaptionExpr: Text; Language: Integer; var Caption: Text; var Resolved: Boolean)
    begin
        if CaptionArea <> 'CT' then exit;
        Calls += 'resolve(' + CaptionExpr + ')';
        Caption := 'resolved:' + CaptionExpr;
        Resolved := true;
    end;
}
page 69998 "CT Cap Card"
{
    PageType = Card; SourceTable = "CT Cap Row"; ApplicationArea = All; UsageCategory = None;
    layout
    {
        area(Content)
        {
            field(Claimed; Rec.Descr) { CaptionClass = 'CT,Hello'; }
            field(FromVariable; Extra) { CaptionClass = 'CT,' + Suffix; }
            field(SetSuffix; Suffix) { }
            field(Classed; Rec.Classed) { }
            field(CaptionOverClass; Rec.Classed) { Caption = 'Own'; }
        }
    }
    var Extra: Text; Suffix: Text;
}
codeunit 69999 C
{
    procedure ReadTwice(): Text
    var R: Record "CT Cap Row"; P: TestPage "CT Cap Card"; L: Codeunit "CT Cap Log"; Out: Text;
    begin
        R.K := 1; R.Insert();
        L.Take();
        P.OpenEdit();
        Out := L.Take() + '|';
        Out += P.Claimed.Caption() + '|' + L.Take() + '|' + P.Claimed.Caption() + '|' + L.Take();
        exit(Out);
    end;
    procedure FollowsTheVariable(): Text
    var R: Record "CT Cap Row"; P: TestPage "CT Cap Card"; L: Codeunit "CT Cap Log"; Out: Text;
    begin
        R.K := 1; R.Insert();
        P.OpenEdit();
        L.Take();
        Out := P.FromVariable.Caption() + '|' + L.Take() + '|';
        P.SetSuffix.SetValue('World');
        Out += L.Take() + '|' + P.FromVariable.Caption() + '|' + L.Take();
        exit(Out);
    end;
    procedure FieldClass(): Text
    var R: Record "CT Cap Row"; P: TestPage "CT Cap Card";
    begin
        R.K := 1; R.Insert();
        P.OpenEdit();
        exit(P.Classed.Caption());
    end;
    procedure CaptionOverClass(): Text
    var R: Record "CT Cap Row"; P: TestPage "CT Cap Card";
    begin
        R.K := 1; R.Insert();
        P.OpenEdit();
        exit(P.CaptionOverClass.Caption());
    end;
}
"#;

#[test]
fn a_control_caption_class_is_resolved_when_the_page_fills_and_read_from_there() {
    // corpus misc/W2CaptionClass.al:92 (at open), :113-116 (no event at either read).
    assert_eq!(
        text(al_testkit::run_in(CAPTION_SRC, "C", "ReadTwice")),
        "resolve(Hello)resolve()|resolved:Hello||resolved:Hello|"
    );
}

#[test]
fn a_caption_class_expression_is_resolved_again_only_when_its_text_changes() {
    // corpus misc/W2CaptionClass.al:174-178: after SetValue only the changed expression raises the event.
    assert_eq!(
        text(al_testkit::run_in(CAPTION_SRC, "C", "FollowsTheVariable")),
        "resolved:||resolve(World)|resolved:World|"
    );
}

#[test]
fn a_control_without_caption_shows_its_field_caption_class() {
    // corpus pageext/lt-page-ext-properties/TableExtCaptionClass.al:69.
    assert_eq!(
        text(al_testkit::run_in(CAPTION_SRC, "C", "FieldClass")),
        "Class caption"
    );
}

#[test]
fn a_control_caption_over_a_field_caption_class_is_refused() {
    let r = al_testkit::run_in(CAPTION_SRC, "C", "CaptionOverClass");
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}
