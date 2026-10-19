//! Repros from outside users' issues, kept as regression tests. Each test names the issue and the corpus pin it relies on.

use al_sema::workspace::{AppKind, AppSource};
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

/// Runs the test codeunit `T` of the workspace app `objects`. `apps` are dependency apps in order, each one
/// depending on the ones before it; the workspace app depends on all of them.
fn run_with_deps(apps: &[&str], objects: &str) -> Vec<(String, Outcome)> {
    let mut sources = Vec::new();
    let mut kinds = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for (i, src) in apps.iter().chain([&objects]).enumerate() {
        let id = format!("app{i}");
        sources.push(AppSource {
            id: id.clone(),
            name: id.clone(),
            dependencies: ids.clone(),
            files: vec![(format!("{id}.al"), src.to_string())],
        });
        kinds.push(if i < apps.len() {
            AppKind::Dependency
        } else {
            AppKind::Workspace
        });
        ids.push(id);
    }
    let (ws, l) = al_sema::lower::compile_apps(sources, kinds, &Default::default());
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

fn all_pass(results: Vec<(String, Outcome)>) {
    assert!(!results.is_empty());
    for (name, outcome) in results {
        assert_eq!(outcome, Outcome::Pass, "{name}");
    }
}

/// Issue 250: the enum field that a table extension of another app adds,
/// read from a TEMPORARY record after Insert and Get, is an enum value in a Variant, also before any value is
/// assigned (that default read as no type at all before PR 260), so `IsOption` is true (the
/// type test Library Assert's AreEqual makes) and it equals the enum constant. Pins: types/TestVariantExtended.al:261
/// (an enum in a Variant reports IsOption).
#[test]
fn issue_250_enum_field_of_extension_on_temporary_record_is_an_option() {
    let base = "table 69500 \"UR Base\" { fields { field(1; \"Document No.\"; Code[20]) { } field(2; \"Line No.\"; Integer) { } } keys { key(PK; \"Document No.\", \"Line No.\") { Clustered = true; } } }";
    {
        let fixture = "
enum 69501 \"UR Kind\" { Extensible = false; value(0; Normal) { } value(1; Other) { } }
tableextension 69501 \"UR Ext\" extends \"UR Base\" { fields { field(69501; \"UR Kind\"; Enum \"UR Kind\") { } } }";
        let main = "
codeunit 69502 T { Subtype = Test;
  [Test] procedure P() var Line: Record \"UR Base\" temporary; Kind: Enum \"UR Kind\"; V: Variant; begin
    Line.\"Document No.\" := 'REPRO'; Line.\"Line No.\" := 10000; Line.Insert();
    Line.Get('REPRO', 10000);
    V := Line.\"UR Kind\"; if not V.IsOption() then Error('default field value is not an option');
    Line.\"UR Kind\" := Kind::Other; Line.Modify(); Line.Get('REPRO', 10000);
    V := Line.\"UR Kind\"; if not V.IsOption() then Error('field is not an option');
    V := Kind::Normal; if not V.IsOption() then Error('constant is not an option');
    if Line.\"UR Kind\" <> Kind::Other then Error('not equal');
  end; }";
        all_pass(run_with_deps(&[base, fixture], main));
    }
}

/// Issue 255: `Info.SystemId(Value)` in method form sets the SystemId and `Info.SystemId()` reads it back.
/// Pin: errorinfo/lt-errnotif/EnTErrorInfoProps.al:106-115 and :259-262 (a SystemId round-trips).
#[test]
fn issue_255_error_info_system_id_method_form_round_trips() {
    let main = "
codeunit 69511 T { Subtype = Test;
  local procedure RoundTrip(Value: Guid): Guid var Info: ErrorInfo; begin
    Info := ErrorInfo.Create('Repro'); Info.SystemId(Value); exit(Info.SystemId());
  end;
  [Test] procedure P() var Value: Guid; begin
    Value := CreateGuid();
    if RoundTrip(Value) <> Value then Error('The error SystemId did not round-trip.');
  end; }";
    all_pass(run_with_deps(&[], main));
}
