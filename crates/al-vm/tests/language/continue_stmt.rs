//! The `continue` statement (Base Application's Inventory Adjustment uses it in a `repeat ... until` loop). No
//! corpus test has one; the behavior here is the one every language with the statement has: skip the rest of the
//! body and go on with the next round, evaluating a loop's condition (`until`, `while`, the `for` bound) as usual
//! (docs/probes-needed.md).

use al_builtins::native::Signal;
use al_builtins::value::Value;

fn run(body: &str) -> Result<Value, Signal> {
    let src = format!(
        "codeunit 50100 C {{ procedure F(): Integer var i: Integer; s: Integer; L: List of [Integer]; x: Integer; begin {body} end; }}"
    );
    al_testkit::run_in(&src, "C", "F")
}

fn int(r: Result<Value, Signal>) -> i32 {
    match r {
        Ok(Value::Integer(n)) => n,
        other => panic!("{other:?}"),
    }
}

#[test]
fn continue_goes_on_with_the_next_round_of_every_kind_of_loop() {
    // Odd numbers from 1 to 9 add up to 25.
    assert_eq!(
        int(run(
            "repeat i += 1; if i mod 2 = 0 then continue; s += i; until i >= 9; exit(s);"
        )),
        25
    );
    assert_eq!(
        int(run(
            "while i < 9 do begin i += 1; if i mod 2 = 0 then continue; s += i; end; exit(s);"
        )),
        25
    );
    assert_eq!(
        int(run(
            "for i := 1 to 9 do begin if i mod 2 = 0 then continue; s += i; end; exit(s);"
        )),
        25
    );
    assert_eq!(
        int(run(
            "for i := 1 to 9 do L.Add(i); foreach x in L do begin if x mod 2 = 0 then continue; s += x; end; exit(s);"
        )),
        25
    );
    // A continue in an inner loop leaves the outer one alone.
    assert_eq!(
        int(run(
            "for i := 1 to 3 do begin x := 0; while x < 3 do begin x += 1; if x = 2 then continue; s += 1; end; end; exit(s);"
        )),
        6
    );
}
