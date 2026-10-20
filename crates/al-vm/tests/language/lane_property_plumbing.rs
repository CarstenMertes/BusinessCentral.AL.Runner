//! Table, field and table extension properties as the running code reads them (lane L6b of the 2026-10-19 failure triage).

use al_testkit::text;

/// `Caption = '..', MaxLength = n` cuts the caption to n characters, on a table and on a field
/// (record/lt-u-table-field-props/UTableTextSearch.al:41-44).
#[test]
fn caption_max_length_cuts_the_table_and_field_caption() {
    let src = "table 50100 T { Caption = 'Text search caption', MaxLength = 5; \
        fields { field(1; Id; Integer) { } field(3; Txt; Text[30]) { Caption = 'Text field caption', MaxLength = 4; } } } \
        codeunit 50100 C { procedure F(): Text var R: Record T; begin exit(R.TableCaption() + '|' + R.FieldCaption(Txt)); end; }";
    assert_eq!(text(al_testkit::run_in(src, "C", "F")), "Text |Text");
}

/// `Format(Row.Opt::Two)` shows the field's OptionCaption entry (misc/lt-probes-needed-2/PnBRecordMisc.al:96, :113).
#[test]
fn format_of_an_option_member_of_a_field_shows_its_caption() {
    let src = "table 50100 T { fields { field(1; Id; Integer) { } field(2; Opt; Option) { OptionMembers = One,Two; OptionCaption = 'Uno,Dos'; } } } \
        codeunit 50100 C { procedure F(): Text var Row: Record T; begin exit(Format(Row.Opt::Two)); end; }";
    assert_eq!(text(al_testkit::run_in(src, "C", "F")), "Dos");
}

/// Evaluate of an empty or blank text into an Option field whose first member is blank selects that member
/// (errors/lt-ms-refusals-131/P131EvalOption.al:107-108); without a blank member it is rejected (:146).
#[test]
fn evaluate_of_a_blank_text_selects_the_blank_member() {
    let src = "table 50100 T { fields { field(1; Id; Integer) { } \
        field(2; Opt; Option) { OptionMembers = \" \",Alpha,Beta,Gamma; OptionCaption = ' ,Alpha cap,Beta cap,Gamma cap'; } \
        field(3; Opt2; Option) { OptionMembers = A,B,C; OptionCaption = 'One,Two,Three'; } } } \
        codeunit 50100 C { \
        procedure E1(Txt: Text): Text var R: Record T; Ok: Boolean; Ord: Integer; begin R.Opt := R.Opt::Gamma; Ok := Evaluate(R.Opt, Txt); Ord := R.Opt; exit(Format(Ok) + '|' + Format(Ord)); end; \
        procedure E2(Txt: Text): Text var R: Record T; Ok: Boolean; Ord: Integer; begin R.Opt2 := R.Opt2::C; Ok := Evaluate(R.Opt2, Txt); Ord := R.Opt2; exit(Format(Ok) + '|' + Format(Ord)); end; \
        procedure F(): Text begin exit(E1('') + ';' + E1(' ') + ';' + E2('')); end; }";
    assert_eq!(text(al_testkit::run_in(src, "C", "F")), "Yes|0;Yes|0;No|2");
}

/// A table extension's DataCaptionFields is the table's (record/lt-x-table-props/XTablePropsTests.al:609, `LT X Props Card - 1 ∙ 1M`).
#[test]
fn table_extension_data_caption_fields_show_in_the_page_caption() {
    let src = "table 50100 T { fields { field(1; Id; Integer) { } field(2; Txt; Text[30]) { } } keys { key(PK; Id) { } } } \
        tableextension 50101 TX extends T { DataCaptionFields = Id, Txt; } \
        page 50102 P { PageType = Card; SourceTable = T; layout { area(content) { field(IdC; Rec.Id) { } } } } \
        codeunit 50100 C { procedure F(): Text var R: Record T; TP: TestPage P; S: Text; \
        begin R.Id := 1; R.Txt := '1M'; R.Insert(); TP.OpenEdit(); S := TP.Caption(); TP.Close(); exit(S); end; }";
    assert_eq!(text(al_testkit::run_in(src, "C", "F")), "P - 1 ∙ 1M");
}
