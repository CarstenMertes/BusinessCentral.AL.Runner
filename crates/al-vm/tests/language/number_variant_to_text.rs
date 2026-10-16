//! A number held by a Variant, handed to a Text or Code, is its default `Format` text. Microsoft tests need it in three
//! places: `TextBuilder.Append(RecRef.FieldIndex(i).Value())` of Assert.RecordCount (Tests-Misc AssertTest
//! RecordCountFailedNoFilter, Tests-Marketing MarketingCampaignSegments: "text argument 0 of type Integer"),
//! `StartingDocumentNo.SetValue(RandInt(10))` on a Code[20] request page control (Tests-Resource ERM Suggest Employee
//! Payment: "conversion of Integer 1 to Code[20]") and a Decimal into a Text (Tests-Resource ResourceGroupAllocatedPerJob).
//! No corpus test pins it (assumption `number-to-text-is-format`, like `date-to-text-is-format`).

use al_testkit::text;

const SRC: &str = r#"
codeunit 69740 C
{
    procedure Put(V: Variant): Text
    var T: Text; C: Code[20];
    begin
        T := V;
        C := V;
        exit(T + ';' + C);
    end;

    procedure Build(V: Variant): Text
    var B: TextBuilder;
    begin
        B.Append(V);
        B.Append(';');
        exit(B.ToText());
    end;

    procedure G(): Text
    begin
        exit(Build(42) + Build(-1));
    end;

    procedure F(): Text
    var D: Decimal; B: BigInteger;
    begin
        D := 65.03;
        B := 5000000000L;
        exit(Put(7) + '|' + Put(-12) + '|' + Put(D) + '|' + Put(B));
    end;
}
"#;

#[test]
fn a_number_in_a_variant_assigned_to_text_or_code_is_its_format_text() {
    assert_eq!(
        text(al_testkit::run_in(SRC, "C", "F")),
        "7;7|-12;-12|65.03;65.03|5000000000;5000000000"
    );
}

#[test]
fn a_number_in_a_variant_appended_to_a_text_builder_is_its_format_text() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "G")), "42;-1;");
}
