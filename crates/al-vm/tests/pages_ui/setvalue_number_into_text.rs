//! `SetValue` of a whole number on a Code or Text control writes the digits (assumption `number-to-text-is-format`;
//! Microsoft's ERM Suggest Employee Payment, Tests-Resource, `StartingDocumentNo.SetValue(RandInt(10))`).

use al_testkit::text;

const SRC: &str = r#"
table 60995 "SVN Row" { fields { field(1; K; Code[10]) { } field(2; N; Code[20]) { } field(3; T; Text[20]) { } } keys { key(PK; K) { } } }
page 60996 "SVN Card" { PageType = Card; SourceTable = "SVN Row"; layout { area(Content) { field(K; Rec.K) { } field(N; Rec.N) { } field(T; Rec.T) { } } } }
codeunit 60997 C
{
    procedure F(): Text
    var P: TestPage "SVN Card";
    begin
        P.OpenNew();
        P.K.SetValue('A');
        P.N.SetValue(7);
        P.T.SetValue(-12);
        exit(P.N.Value() + ';' + P.T.Value());
    end;
}
"#;

#[test]
fn a_whole_number_set_on_a_code_or_text_control_is_written_as_digits() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "7;-12");
}
