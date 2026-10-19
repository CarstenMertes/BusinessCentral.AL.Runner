//! A List read inside a `foreach` over the same list. `IndexOf` there does not count as a change: the corpus calls
//! `Ordinals.IndexOf(Index)` inside `foreach Index in Ordinals` (misc/lt-u-obsolete-pending/UObsoletePending.al:35-36) and
//! passes on real BC. The methods that change a list raise "Collection was modified" (collections/RefSemantics.al:343, :365,
//! :387, :408). The other reads are not pinned there, so the loop refuses after one of them.

use al_builtins::native::Signal;
use al_builtins::value::Value;

fn run(read: &str) -> Result<Value, Signal> {
    let src = format!(
        "codeunit 50100 C {{ procedure F(): Integer var L: List of [Integer]; R: List of [Integer]; i: Integer; x: Integer; s: Integer; begin \
        L.Add(10); L.Add(20); L.Add(30); \
        foreach i in L do begin {read} end; \
        exit(s); end; }}"
    );
    al_testkit::run_in(&src, "C", "F")
}

#[test]
fn index_of_inside_a_foreach_over_the_list_runs() {
    match run("s += L.IndexOf(i);") {
        Ok(Value::Integer(n)) => assert_eq!(n, 6),
        other => panic!("{other:?}"),
    }
}

#[test]
fn other_reads_inside_a_foreach_over_the_list_are_refused() {
    for read in [
        "s += L.Count();",
        "s += L.Get(1);",
        "if L.Get(2, x) then s += x;",
        "if L.Contains(i) then s += 1;",
        "s += L.LastIndexOf(i);",
        "R := L.GetRange(1, 2);",
    ] {
        match run(read) {
            Err(Signal::Unsupported(why)) => assert!(why.contains("IndexOf"), "{read}: {why}"),
            other => panic!("{read}: expected a refusal, got {other:?}"),
        }
    }
}
