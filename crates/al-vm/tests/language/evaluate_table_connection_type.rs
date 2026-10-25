//! `Evaluate` into a TableConnectionType: an ordinal of a member evaluates, 5 to 10 answer false
//! (misc/W5TableConnections.al:117-136, TableConnectionType_Members).

use al_builtins::native::Signal;

#[test]
fn evaluate_into_table_connection_type_reads_an_ordinal_and_rejects_one_past_the_members() {
    let src = "codeunit 50100 C { procedure F(): Text var K: TableConnectionType; i: Integer; M: Text; begin \
               for i := 0 to 10 do if Evaluate(K, Format(i)) then M += Format(i) + '=' + Format(K) + ';'; exit(M); end; }";
    assert_eq!(
        al_testkit::text(al_testkit::run_with(src, "C", "F", vec![]).map(|(v, _)| v)),
        "0=CRM;1=ExternalSQL;2=Exchange;3=MicrosoftGraph;4=Query;"
    );
}

#[test]
fn evaluate_into_table_connection_type_of_an_unpinned_text_is_refused() {
    // A number past 10 is not pinned.
    for text in ["11"] {
        let src = format!(
            "codeunit 50100 C {{ procedure F(): Boolean var K: TableConnectionType; begin exit(Evaluate(K, '{text}')); end; }}"
        );
        match al_testkit::run_with(&src, "C", "F", vec![]) {
            Err(Signal::Unsupported(_)) => {}
            other => panic!("{text}: {other:?}"),
        }
    }
}
