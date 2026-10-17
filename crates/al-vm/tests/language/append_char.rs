//! `S += T[i]` (a Text and a Char) is the usual way to build a text one character at a time. It took the slow path of the
//! append: a new string every round, and a scan of the whole text for the call-stack mark every round, so a loop over
//! 400,000 characters took 8 s (Pageworks builds fonts and PDF streams this way). A Char now appends in place, as a Text
//! does, and the text keeps what is known of it (that it holds no mark).

use al_testkit::{run_in, text};

#[test]
fn appending_a_char_to_a_text_gives_the_same_text_as_before() {
    let src = r#"
codeunit 50100 C
{
    procedure F(): Text
    var S: Text; Src: Text; I: Integer; C: Char; T: Text[5];
    begin
        Src := 'héllo wörld';
        for I := 1 to StrLen(Src) do S += Src[I];
        C := 65; S += C;
        C := 57344; S += C;
        T := 'ab'; T += 'c'[1]; T += 'd'[1]; T += 'e'[1];
        exit(S + '|' + Format(StrLen(S)) + '|' + T);
    end;
    procedure Over(): Text
    var T: Text[3]; C: Char;
    begin
        T := 'abc'; C := 100; T += C; exit(T);
    end;
}
"#;
    assert_eq!(text(run_in(src, "C", "F")), "héllo wörldA\u{E000}|13|abcde");
    // A Text[3] cannot hold a fourth character: the error is the one of the slow path.
    match run_in(src, "C", "Over") {
        Err(al_builtins::native::Signal::Error(e)) => assert!(e.message.contains("3"), "{}", e.message),
        other => panic!("{other:?}"),
    }
}

#[test]
fn building_a_text_character_by_character_does_not_scan_the_whole_text_every_round() {
    let src = r#"
codeunit 50100 C
{
    procedure F(): Integer
    var S: Text; I: Integer;
    begin
        for I := 1 to 3000 do S += 'abcdefgh'[(I mod 8) + 1];
        exit(StrLen(S));
    end;
}
"#;
    let before = al_builtins::text::marker_scans();
    assert_eq!(al_testkit::int(run_in(src, "C", "F")), 3000);
    let scans = al_builtins::text::marker_scans() - before;
    assert!(scans < 100, "{scans} scans of the text for 3000 appends");
}
