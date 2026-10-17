//! `Xmlport.Run` (issue 28): no corpus test runs an xmlport through `Xmlport.Run`, whose target (a file
//! on the service tier) a test cannot read, so both forms are refused with a reason.

#[test]
fn xmlport_run_is_refused_with_its_reason() {
    let src = "xmlport 63100 X { schema { textelement(Root) { } } }
         codeunit 63101 T { procedure Static() begin Xmlport.Run(63100, false, false); end;
                            procedure Inst() var P: XmlPort X; begin P.Run(); end; }";
    let l = al_testkit::compile(src);
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    // The static form raises its errors when it runs (branch ms-top-refusals-1); the instance form is refused here.
    for routine in ["Inst"] {
        let ri = l.program.objects[oi]
            .routines
            .iter()
            .position(|r| r.name == routine)
            .unwrap();
        let why = l.unsupported[oi][ri].first().cloned().unwrap_or_default();
        assert!(
            why.contains("Xmlport.Run") && why.contains("not pinned"),
            "{routine}: {why}"
        );
        assert!(!why.contains("is not implemented"), "{routine}: {why}");
    }
}
