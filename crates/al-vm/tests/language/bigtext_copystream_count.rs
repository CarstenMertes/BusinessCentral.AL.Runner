//! BigText.AddText at a position, TextPos of an empty text, GetSubText ranges, Clear and Format of a BigText
//! (streams/StTBigText.al), and CopyStream with a byte count (streams/StTCopyBlob.al).

fn run(decl: &str, body: &str) -> String {
    let src = format!(
        "table 50100 T {{ fields {{ field(1; Id; Integer) {{ }} field(2; Data; Blob) {{ }} }} keys {{ key(PK; Id) {{ }} }} }}
        codeunit 50100 C {{ procedure F(): Text var S: Text; T: Text; {decl} begin {body} end; }}"
    );
    al_testkit::text(al_testkit::run_in(&src, "C", "F"))
}

#[test]
fn big_text_forms() {
    let out = run(
        "Big: BigText; N: Integer;",
        "Big.AddText('abcdef'); Big.AddText('XY', 3); Big.AddText('Q', Big.Length() + 1);
         Big.GetSubText(T, 1); S := T + '|' + Format(Big.TextPos('')) + '|';
         N := Big.GetSubText(T, 8, 100); S += Format(N) + T + '|' + Format(Big) + '|';
         Clear(Big); S += Format(Big.Length());
         exit(S);",
    );
    assert_eq!(out, "abXYcdefQ|0|2fQ|abXYcdefQ|0");
}

#[test]
fn big_text_range_errors() {
    let err = |body: &str| {
        let src = format!(
            "codeunit 50100 C {{ procedure F() var Big: BigText; T: Text; begin Big.AddText('abc'); {body} end; }}"
        );
        al_testkit::error_message(al_testkit::run_in(&src, "C", "F"))
    };
    assert_eq!(
        err("Big.AddText('x', 0);"),
        "The value of INSSTR parameter 3 is outside of the permitted range. The current value is: 0. The permitted range is: from 1 to 2,147,483,647."
    );
    assert_eq!(
        err("Big.GetSubText(T, -3, 1);"),
        "The value of COPYSTR parameter 2 is outside of the permitted range. The current value is: -3. The permitted range is: from 1 to 2,147,483,647."
    );
    assert_eq!(
        err("Big.GetSubText(T, 1, -1);"),
        "The value of COPYSTR parameter 3 is outside of the permitted range. The current value is: -1. The permitted range is: from 0 to 2,147,483,647."
    );
}

#[test]
fn copy_stream_with_a_byte_count() {
    let out = run(
        "R: Record T; R2: Record T; O: OutStream; O2: OutStream; I: InStream; I2: InStream;",
        "R.Data.CreateOutStream(O); O.WriteText('0123456789'); R.Data.CreateInStream(I);
         R2.Data.CreateOutStream(O2); S := Format(CopyStream(O2, I, 4)) + Format(I.Position());
         S += Format(CopyStream(O2, I, 0)) + Format(I.Position());
         R2.Data.CreateInStream(I2); I2.ReadText(T); S += T;
         Clear(R); R.Data.CreateInStream(I); S += Format(CopyStream(O2, I, 5));
         exit(S);",
    );
    assert_eq!(out, "Yes5Yes50123No");
}
