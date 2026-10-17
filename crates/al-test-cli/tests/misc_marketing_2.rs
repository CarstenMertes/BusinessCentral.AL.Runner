//! Behaviours Microsoft's Tests-Misc and Tests-Marketing reach. Each test says what pins it; docs/probes-needed.md has the rows
//! for what nothing pins.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

const OBJECTS: &str = "
table 69400 \"MM Head\" { fields { field(1; No; Code[20]) { } } keys { key(PK; No) { Clustered = true; } } }
table 69401 \"MM Line\" { fields { field(1; No; Code[20]) { } field(2; Line; Integer) { } field(3; Sub; Code[10]) { } }
  keys { key(PK; No, Line, Sub) { Clustered = true; } } }
";

fn run(tests: &str) -> Vec<(String, Outcome)> {
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![(
                "f.al".into(),
                format!("{OBJECTS} codeunit 69402 T {{ Subtype = Test; {tests} }}"),
            )],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let engine = Engine::new(l.program.clone(), blocked);
    run_codeunit(&engine, &l, oi)
}

fn all_pass(tests: &str) {
    for (name, outcome) in run(tests) {
        assert_eq!(outcome, Outcome::Pass, "{name}");
    }
}

// A Variant that holds a record of one table takes a record of another table whole (Library - Variable Storage.Dequeue into one
// variable twice, Tests-Marketing AddContactsToSegments). Before: the second record was copied into the first one's buffer.
#[test]
fn a_variant_holding_a_record_takes_a_record_of_another_table() {
    all_pass(
        "var Q: array[2] of Variant;
         [Test] procedure Replace() var H: Record \"MM Head\"; L: Record \"MM Line\"; V: Variant; RR: RecordRef; begin
             Q[1] := H; Q[2] := L;
             V := Q[1]; RR.GetTable(V); if RR.Number <> 69400 then Error('first %1', RR.Number);
             V := Q[2]; Clear(RR); RR.GetTable(V); if RR.Number <> 69401 then Error('second %1', RR.Number);
         end;",
    );
}

// errors/lt-ms-refusals-128/P128EvalRecordId.al pins a single-field key; the composite key is the registered assumption
// evaluate-recordid-composite-key (Library - Error Message.LoadErrorMessages).
#[test]
fn evaluate_into_a_record_id_of_a_composite_key_reads_one_value_per_field() {
    all_pass(
        "[Test] procedure Composite() var L: Record \"MM Line\"; Rid: RecordId; begin
             L.No := 'A'; L.Line := 10; L.Sub := 'X'; L.Insert();
             if not Evaluate(Rid, Format(L.RecordId)) then Error('eval');
             if Format(Rid) <> Format(L.RecordId) then Error('text %1', Format(Rid));
             if Rid <> L.RecordId then Error('equal');
             if Evaluate(Rid, 'MM Line: A,10') then Error('too few');
             if Evaluate(Rid, 'MM Line: A,x,X') then Error('bad integer');
         end;",
    );
}
