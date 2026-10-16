//! Tests-ERM refusals of issue 26: an Integer stored into an Enum and `RecordRef.Get(RecordId)` on a
//! RecordRef that is not open. No corpus test pins either (`grep` of corpus/tests/al-language finds
//! `Get(RecordId)` only on an open RecordRef), so each is refused with a reason that names the open
//! question. `RecordRef.GetBySystemId`, refused here before, is probed now (al-vm
//! tests/record_builtins.rs).

use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

fn compile(tests: &str) -> (al_sema::lower::Lowered, usize) {
    let src = format!(
        "enum 62900 \"B6 Kind\" {{ value(0; A) {{ }} value(1; B) {{ }} }}
         table 62901 \"B6 Row\" {{ fields {{ field(1; Kind; Enum \"B6 Kind\") {{ }} field(2; Name; Text[20]) {{ }} }} keys {{ key(PK; Kind) {{ }} }} }}
         codeunit 62902 T {{ Subtype = Test; {tests} }}"
    );
    let l = al_testkit::compile(&src);
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    (l, oi)
}

fn outcomes(tests: &str) -> Vec<(String, Outcome)> {
    let (l, oi) = compile(tests);
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let engine = Engine::new(l.program.clone(), blocked);
    run_codeunit(&engine, &l, oi)
}

fn not_run(o: &(String, Outcome)) -> String {
    match &o.1 {
        Outcome::NotRun(why) => why.clone(),
        other => panic!("{}: {other:?}", o.0),
    }
}

#[test]
fn an_integer_key_for_an_enum_field_is_refused_with_its_reason() {
    let o =
        outcomes("[Test] procedure IntegerKey() var R: Record \"B6 Row\"; N: Integer; begin N := 1; R.Get(N); end;");
    let why = not_run(&o[0]);
    assert!(
        why.contains("Integer") && why.contains("Enum") && why.contains("not pinned") && why.contains("probes-needed"),
        "{why}"
    );
}

#[test]
fn a_call_on_a_closed_record_ref_is_refused_with_its_reason() {
    // `Get(RecordId)` on a closed RecordRef opens the table now (assumption recordref-get-unopened); the other calls stay refused.
    let o = outcomes("[Test] procedure Closed() var RR: RecordRef; N: Integer; begin N := RR.Count(); end;");
    let why = not_run(&o[0]);
    assert!(
        why.contains("RecordRef.Count")
            && why.contains("not open")
            && why.contains("not pinned")
            && why.contains("probes-needed"),
        "{why}"
    );
}

/// `Database.SID()` without a user name: the probe (corpus branch al-runtime-rs-staging, ae3fe88,
/// tests/al-language-onprem/session/PXDatabaseSid.al) passes a name every time, so whose SID the
/// call without one answers is not pinned. Refused when it runs, with its reason.
#[test]
fn database_sid_without_a_user_name_is_refused_with_its_reason() {
    let o = outcomes("[Test] procedure NoName() var T: Text; begin T := Database.SID(); end;");
    let why = not_run(&o[0]);
    assert!(why.contains("Database.SID") && why.contains("not pinned"), "{why}");
}
