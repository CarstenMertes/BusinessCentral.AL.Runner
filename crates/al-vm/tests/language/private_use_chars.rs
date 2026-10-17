//! A text that holds a character of the Unicode private use area is an ordinary text. The runtime marks a call stack
//! with an untracked frame (and an error whose wording is not pinned) with a character inside the text, and refuses
//! every text function on such a text; the marks used U+E000 and U+E001, which real code uses too: Pageworks' barcode
//! encoder writes the glyphs of its barcode font as U+E000 plus the digit, and every test that builds one was refused
//! with "text argument 0 of type Text". The marks are now noncharacters (U+FDD0, U+FDD1), which no text holds.

use al_testkit::{run_in, text};

fn src() -> String {
    r#"
codeunit 50100 C
{
    procedure Glyph(): Text
    var T: Text; I: Integer; Parts: Text;
    begin
        // U+E000 + digit, as a barcode font addresses its glyphs.
        for I := 0 to 3 do begin
            T[I + 1] := 57344 + I;
        end;
        Parts := Format(StrLen(T)) + '|' + CopyStr(T, 2, 2) + '|' + Format(T.Contains(CopyStr(T, 1, 1))) + '|' + Format(StrPos(T, CopyStr(T, 3, 1)));
        exit(Parts);
    end;

    procedure OneGlyph(): Text
    var T: Text;
    begin
        T[1] := 57344;
        exit(Format(StrLen(T)) + '|' + Format(T = '') + '|' + UpperCase(T) + '|' + Format(T.ToUpper() = T));
    end;
}
"#
    .to_string()
}

#[test]
fn a_text_with_characters_of_the_private_use_area_is_read_by_the_text_functions() {
    let r = text(run_in(&src(), "C", "Glyph"));
    assert_eq!(r, "4|\u{E001}\u{E002}|Yes|3");
}

#[test]
fn a_single_private_use_character_is_not_empty_and_is_not_changed_by_case_functions() {
    let r = text(run_in(&src(), "C", "OneGlyph"));
    assert_eq!(r, "1|No|\u{E000}|Yes");
}
