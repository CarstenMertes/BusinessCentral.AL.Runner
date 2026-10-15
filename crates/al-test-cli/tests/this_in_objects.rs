//! `this` is the running object in every object type, not only in a codeunit (issue 32): Base Application's
//! "Calculate Inventory" report calls `this.GetLocation(..)` in `OnAfterGetRecord`. Evidence is Microsoft Learn,
//! "this keyword" (AL language version 14: "self-reference for all objects"); no corpus test uses it outside a
//! codeunit (docs/probes-needed.md).

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

#[test]
fn a_report_calls_its_own_procedure_and_reads_its_own_variable_through_this() {
    let src = r#"
codeunit 63401 "TI Log" { SingleInstance = true; var L: Text; procedure Add(T: Text) begin L += T; end; procedure Take(): Text var R: Text; begin R := L; L := ''; exit(R); end; }
table 63403 "TI Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
report 63400 "TI Report"
{
    ProcessingOnly = true; UseRequestPage = false;
    dataset { dataitem(Loop; "TI Row") {
        trigger OnAfterGetRecord() var Log: Codeunit "TI Log"; begin Log.Add(this.Name() + ':' + Format(this.Counter)); end; } }
    trigger OnPreReport() begin Counter := 7; end;
    local procedure Name(): Text begin exit('rep'); end;
    var Counter: Integer;
}
codeunit 63402 T
{
    Subtype = Test;
    [Test] procedure ThisInAReport() var Log: Codeunit "TI Log"; R: Record "TI Row"; begin R.Id := 1; R.Insert(); Report.Run(Report::"TI Report", false, false); if Log.Take() <> 'rep:7' then Error('got %1', Log.Take()); end;
}
"#;
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src.into())],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let engine = Engine::new(l.program.clone(), blocked);
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    let o = run_codeunit(&engine, &l, oi);
    assert_eq!(o[0].1, Outcome::Pass, "{:?}", o[0]);
}
