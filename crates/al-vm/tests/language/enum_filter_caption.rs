//! A filter on an Enum field reads a member by its caption as well as by its name. Report Selections.CopyFilters does
//! `SetFilter(Usage, GetFilter(Usage))` and `GetFilter` gives the caption of the member (Microsoft Tests-Misc, codeunit
//! 134421 "Report Selections Tests".SalesQuoteSendByEmailWhenArchivingIsOn and 41 Object Selection tests, refused with
//! "`Sales Quote` is not a member of the enum"). Assumption `enum-filter-by-caption`.

use al_testkit::text;

const SRC: &str = r#"
enum 69710 "EFC Kind" { value(0; A) { Caption = 'Alpha one'; } value(1; B) { } value(2; "S.Quote") { Caption = 'Sales Quote'; } }
table 69711 "EFC Row" { fields { field(1; Id; Integer) { } field(2; Kind; Enum "EFC Kind") { } } keys { key(PK; Id) { } } }
codeunit 69712 C
{
    procedure F(): Text
    var R: Record "EFC Row"; R2: Record "EFC Row"; T: Text;
    begin
        R.Id := 1; R.Kind := R.Kind::"S.Quote"; R.Insert();
        R.Id := 2; R.Kind := R.Kind::A; R.Insert();
        R2.SetRange(Kind, R2.Kind::"S.Quote");
        T += R2.GetFilter(Kind) + ';';
        R.SetFilter(Kind, R2.GetFilter(Kind));
        T += Format(R.Count()) + ';';
        R.SetFilter(Kind, 'alpha ONE');
        T += Format(R.Count()) + ';';
        R.SetFilter(Kind, 'S.Quote');
        T += Format(R.Count());
        exit(T);
    end;
}
"#;

#[test]
fn a_filter_reads_an_enum_member_by_its_caption() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "F")), "Sales Quote;1;1;1");
}
