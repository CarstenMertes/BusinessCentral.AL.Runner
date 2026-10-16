//! Text methods on a Label or TextConst, `Text.Remove`, and `IndexOfAny` with a start index
//! (corpus types/lt-variant-is-text-streams/LTP89TextConst.al, text/lt-label-textbuilder).

use al_testkit::text;

fn run(body: &str) -> String {
    let src = format!(
        "codeunit 50100 C {{ var Src: Label 'Hello, World'; procedure F(): Text var T: Text; Chars: List of [Char]; begin T := Src; {body} end; }}"
    );
    text(al_testkit::run_with(&src, "C", "F", vec![]).map(|(v, _)| v))
}

#[test]
fn a_label_has_the_text_methods() {
    // LTP89TextConst.al:Search_1, Edit_1, Edit_2, Trim_Family: a Label answers as the Text it holds.
    assert_eq!(
        run(
            "exit(Format(Src.Contains('World')) + '|' + Format(Src.LastIndexOf('o')) + '|' + Src.ToUpper() + '|' + Src.PadLeft(14, '*') + '|' + Src.Replace('l', 'L') + '|' + Src.Substring(8, 3) + '|' + Src.ToLower());"
        ),
        "Yes|9|HELLO, WORLD|**Hello, World|HeLLo, WorLd|Wor|hello, world"
    );
}

#[test]
fn text_remove_is_one_based_with_an_optional_count() {
    // LTP89TextConst.al:Edit_1: remove_t `Hello`, remove_count_t `Helloorld`.
    assert_eq!(
        run("exit(T.Remove(6) + '|' + T.Remove(6, 3) + '|' + Src.Remove(6, 3));"),
        "Hello|Helloorld|Helloorld"
    );
}

#[test]
fn index_of_any_takes_a_one_based_start_index() {
    // LTP89TextConst.al:IndexOfAny_Overloads: `[o, ,]` finds 5 from the start and 6 from position 6.
    assert_eq!(
        run(
            "Chars.Add('o'); Chars.Add(','); exit(Format(T.IndexOfAny(Chars)) + '|' + Format(Src.IndexOfAny(Chars, 6)) + '|' + Format(T.IndexOfAny(Chars, 7)));"
        ),
        "5|6|9"
    );
}

#[test]
fn variant_knows_in_streams_out_streams_and_never_binary() {
    // types/lt-variant/VaTHelper.al Flags (InStream, OutStream) and lt-variant-is-text-streams/LTP89IsMisc.al:Binary_OnManyVariants
    // (IsBinary is No for every value, a stream and a Blob field's value included).
    let src = "table 50101 T { fields { field(1; Id; Integer) { } field(2; Data; Blob) { } } keys { key(PK; Id) { } } }
        codeunit 50100 C { procedure F(): Text var R: Record T; Ins: InStream; Outs: OutStream; V: Variant; S: Text; begin
          R.Data.CreateInStream(Ins); R.Data.CreateOutStream(Outs);
          V := Ins; S += Format(V.IsInStream()) + Format(V.IsOutStream()) + Format(V.IsBinary()) + '|';
          V := Outs; S += Format(V.IsInStream()) + Format(V.IsOutStream()) + Format(V.IsBinary()) + '|';
          V := 5; S += Format(V.IsInStream()) + Format(V.IsOutStream()) + Format(V.IsBinary());
          exit(S); end; }";
    assert_eq!(
        text(al_testkit::run_with(src, "C", "F", vec![]).map(|(v, _)| v)),
        "YesNoNo|NoYesNo|NoNoNo"
    );
}

#[test]
fn a_placeholder_without_an_argument_stays_as_written() {
    // types/lt-variant/VaTTextConst.al:TextConst_StrSubstNo.
    let src = "codeunit 50100 C { procedure F(): Text var W: Text; begin W := 'Value %1 and %2';
        exit(StrSubstNo(W, 'a') + '|' + StrSubstNo(W) + '|' + StrSubstNo('%10 and %1', 'a') + '|' + StrSubstNo('%0 and %1', 'a') + '|' + StrSubstNo('%5', 'a') + '|' + StrSubstNo('end%', 'a') + '|' + StrSubstNo(W, '%2', 'x') + '|' + StrSubstNo('%01', 'a') + '|' + StrSubstNo('%%1', 'a')); end; }";
    assert_eq!(
        text(al_testkit::run_with(src, "C", "F", vec![]).map(|(v, _)| v)),
        "Value a and %2|Value %1 and %2|%10 and a|%0 and a|%5|end%|Value %2 and x|%01|%a"
    );
}

#[test]
fn a_hash_field_in_a_template_is_refused_not_written_as_text() {
    // BC reads `#1####` as a field of width six (SubstNoHash.al, P86StrSubstNo.al:Sub_FieldWidth); until it is implemented the
    // template is refused.
    let src = "codeunit 50100 C { procedure F(): Text begin exit(StrSubstNo('#1####', 'a')); end; }";
    assert!(matches!(
        al_testkit::run_with(src, "C", "F", vec![]),
        Err(al_builtins::native::Signal::Unsupported(_))
    ));
}

#[test]
fn data_transfer_update_audit_fields_is_accepted_after_set_tables_outside_install_code() {
    // misc/W5DataTransferOutside.al:SetTables_Twice_AndUpdateAuditFields (`DT.UpdateAuditFields := false`),
    // misc/lt-probes-needed-2/PnBErrorsCollections.al:DataTransfer_OutsideInstall op6 (`UpdateAuditFields(true)`).
    let src = "table 50101 T { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
        codeunit 50100 C { procedure F(): Text var DT: DataTransfer; begin DT.SetTables(Database::T, Database::T); DT.UpdateAuditFields := false; DT.UpdateAuditFields(true); exit('ok'); end; }";
    assert_eq!(text(al_testkit::run_with(src, "C", "F", vec![]).map(|(v, _)| v)), "ok");
}

#[test]
fn a_one_character_literal_in_a_variant_is_a_char_and_clear_empties_a_variant() {
    // types/lt-variant/VaTIs*.al:Is_Text_Code_Char_Byte (`V := 'a'` is Char; `'ab'`, `''` and a Text variable are Text),
    // :Is_Empty_Variant (`V := 5; Clear(V)` leaves no type); LTP89IsMisc.al:WideChar_Variants and NearTypes (Abs keeps its type).
    let src = "codeunit 50100 C { procedure F(): Text var V: Variant; T: Text; I: Integer; S: Text; begin
        V := 'a'; S += Format(V.IsChar()) + Format(V.IsText()) + Format(V.IsWideChar()) + '|';
        V := 'ab'; S += Format(V.IsChar()) + Format(V.IsText()) + '|';
        T := 'a'; V := T; S += Format(V.IsChar()) + Format(V.IsText()) + '|';
        V := 5; Clear(V); S += Format(V.IsInteger()) + '|';
        I := -3; V := Abs(I); S += Format(V.IsInteger()) + Format(V.IsDecimal());
        exit(S); end; }";
    assert_eq!(
        text(al_testkit::run_with(src, "C", "F", vec![]).map(|(v, _)| v)),
        "YesNoYes|NoYes|NoYes|No|YesNo"
    );
}

#[test]
fn a_text_builder_has_a_capacity_of_16_that_can_be_set_and_that_clear_keeps() {
    // text/lt-label-textbuilder/TxCTextBuilderBasics.al:Capacity_Defaults, Capacity_Setter, Clear_Basics.
    let src = "codeunit 50100 C { procedure F(): Text var TB: TextBuilder; S: Text; begin
        S += Format(TB.Capacity()); TB.Append('abc'); S += '|' + Format(TB.Capacity());
        TB.Capacity(100); S += '|' + Format(TB.Capacity()); TB.Clear(); S += '|' + Format(TB.Capacity()) + '|' + TB.ToText() + '|' + Format(TB.Length());
        exit(S); end; }";
    assert_eq!(
        text(al_testkit::run_with(src, "C", "F", vec![]).map(|(v, _)| v)),
        "16|16|100|100||0"
    );
}
