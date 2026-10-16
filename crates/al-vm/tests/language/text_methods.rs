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
        run("exit(Format(Src.Contains('World')) + '|' + Format(Src.LastIndexOf('o')) + '|' + Src.ToUpper() + '|' + Src.PadLeft(14, '*') + '|' + Src.Replace('l', 'L') + '|' + Src.Substring(8, 3) + '|' + Src.ToLower());"),
        "Yes|9|HELLO, WORLD|**Hello, World|HeLLo, WorLd|Wor|hello, world"
    );
}

#[test]
fn text_remove_is_one_based_with_an_optional_count() {
    // LTP89TextConst.al:Edit_1: remove_t `Hello`, remove_count_t `Helloorld`.
    assert_eq!(run("exit(T.Remove(6) + '|' + T.Remove(6, 3) + '|' + Src.Remove(6, 3));"), "Hello|Helloorld|Helloorld");
}

#[test]
fn index_of_any_takes_a_one_based_start_index() {
    // LTP89TextConst.al:IndexOfAny_Overloads: `[o, ,]` finds 5 from the start and 6 from position 6.
    assert_eq!(
        run("Chars.Add('o'); Chars.Add(','); exit(Format(T.IndexOfAny(Chars)) + '|' + Format(Src.IndexOfAny(Chars, 6)) + '|' + Format(T.IndexOfAny(Chars, 7)));"),
        "5|6|9"
    );
}
