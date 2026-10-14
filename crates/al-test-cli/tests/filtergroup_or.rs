//! `FilterGroup(-1)` is the "OR group" in BC: filters set in it on different fields are alternatives.
//! Nothing in the corpus pins it, and the runtime applies every group as an AND, which selects other
//! rows (issue 17: Base Application's `FilterBlankSalesDiscountAccounts`). It is refused, not guessed.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

const OBJECTS: &str = r#"
table 62600 "FG Row"
{
    fields { field(1; "No."; Code[20]) { } field(2; A; Code[20]) { } field(3; B; Code[20]) { } }
    keys { key(PK; "No.") { } }
}
"#;

fn outcomes(tests: &str) -> Vec<(String, Outcome)> {
    let src = format!("{OBJECTS}\ncodeunit 62601 T {{ Subtype = Test; {tests} }}");
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
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let engine = Engine::new(l.program.clone(), blocked);
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    run_codeunit(&engine, &l, oi)
}

#[test]
fn the_or_group_is_refused_and_the_other_groups_still_work() {
    let o = outcomes(
        "[Test] procedure OrGroup() var R: Record \"FG Row\"; begin R.FilterGroup(-1); R.SetRange(A, ''); end;
         [Test] procedure OtherGroups() var R: Record \"FG Row\"; begin
             R.FilterGroup(2); R.SetRange(A, 'x'); R.FilterGroup(0);
             if R.FilterGroup() <> 0 then Error('group'); end;",
    );
    assert!(
        matches!(&o[0].1, Outcome::NotRun(why) if why.contains("FilterGroup(-1)") && why.contains("not pinned")),
        "{:?}",
        o[0]
    );
    assert_eq!(o[1].1, Outcome::Pass, "{:?}", o[1]);
}
