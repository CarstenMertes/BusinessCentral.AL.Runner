//! Tests-ERM refusals of issue 26: an Integer stored into an Enum, `RecordRef.Get(RecordId)` on a
//! RecordRef that is not open, and `RecordRef.GetBySystemId`. No corpus test pins any of the three
//! (`grep` of corpus/tests/al-language finds `Get(RecordId)` only on an open RecordRef and
//! `GetBySystemId` only on a Record), so each is refused with a reason that names the open question.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

fn compile(tests: &str) -> (al_sema::lower::Lowered, usize) {
    let src = format!(
        "enum 62900 \"B6 Kind\" {{ value(0; A) {{ }} value(1; B) {{ }} }}
         table 62901 \"B6 Row\" {{ fields {{ field(1; Kind; Enum \"B6 Kind\") {{ }} field(2; Name; Text[20]) {{ }} }} keys {{ key(PK; Kind) {{ }} }} }}
         codeunit 62902 T {{ Subtype = Test; {tests} }}"
    );
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src)],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
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
fn get_by_record_id_on_a_closed_record_ref_is_refused_with_its_reason() {
    let o =
        outcomes("[Test] procedure Closed() var R: Record \"B6 Row\"; RR: RecordRef; begin RR.Get(R.RecordId); end;");
    let why = not_run(&o[0]);
    assert!(
        why.contains("RecordRef.Get")
            && why.contains("not open")
            && why.contains("not pinned")
            && why.contains("probes-needed"),
        "{why}"
    );
}

#[test]
fn record_ref_get_by_system_id_is_refused_with_its_reason() {
    let (l, oi) =
        compile("procedure ById(var RR: RecordRef; Id: Guid): Boolean begin exit(RR.GetBySystemId(Id)); end;");
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == "ById")
        .unwrap();
    let why = l.unsupported[oi][ri].first().cloned().unwrap_or_default();
    assert!(
        why.contains("RecordRef.GetBySystemId") && why.contains("not pinned"),
        "{why}"
    );
    assert!(!why.contains("is not implemented"), "{why}");
}
