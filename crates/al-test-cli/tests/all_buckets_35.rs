//! Issue 35: refusals that stop tests in every bucket get a reason that names what is missing.

use al_sema::workspace::AppSource;

fn lowered(src: &str) -> (al_sema::workspace::Workspace, al_sema::lower::Lowered) {
    al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src.into())],
        }],
        &Default::default(),
    )
}

// No corpus test calls GetLastErrorText(ExcludeCustomerContent) (the signature is only in
// scripts/al-surface.json). It is lowered as a call now (runtime refusal for a pending error, assumption get-last-error-text-flag-without-error):
// 300 Microsoft tests reach it with no error pending.
#[test]
fn get_last_error_text_with_the_flag_is_lowered_as_a_call() {
    let src = "codeunit 63500 T { procedure P(): Text begin exit(GetLastErrorText(true)); end; }";
    let (ws, l) = lowered(src);
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    assert_eq!(l.unsupported[oi][0].first().cloned(), None);
}
