//! Read-only List methods inside a `foreach` over the same list do not count as a change of the list. The corpus
//! calls `Ordinals.IndexOf(Index)` inside `foreach Index in Ordinals` (misc/lt-u-obsolete-pending/UObsoletePending.al:35-36)
//! and passes on real BC; the methods that change a list do raise "Collection was modified" (collections/RefSemantics.al:343,
//! :365, :387, :408).

use al_builtins::value::Value;

#[test]
fn read_only_list_methods_inside_a_foreach_over_the_list_run() {
    let src = "codeunit 50100 C { procedure F(): Integer var L: List of [Integer]; R: List of [Integer]; i: Integer; x: Integer; s: Integer; begin \
        L.Add(10); L.Add(20); L.Add(30); \
        foreach i in L do begin \
            s += L.IndexOf(i) + L.LastIndexOf(i) + L.Count(); \
            if L.Contains(i) then s += 1; \
            s += L.Get(1); \
            if L.Get(2, x) then s += x; \
            R := L.GetRange(1, 2); \
        end; \
        exit(s); end; }";
    // Per round: IndexOf + LastIndexOf (2 * position) + 3 + 1 + 10 + 20; positions 1, 2, 3.
    match al_testkit::run_in(src, "C", "F") {
        Ok(Value::Integer(n)) => assert_eq!(n, 2 * (1 + 2 + 3) + 3 * (3 + 1 + 10 + 20)),
        other => panic!("{other:?}"),
    }
}
