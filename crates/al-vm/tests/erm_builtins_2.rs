//! Builtins that kept about 280 Tests-ERM tests from running (issue 14). No corpus test calls any of
//! them, so each is refused with a reason that says so (hard rules 3 and 4).

use al_sema::workspace::AppSource;

const SRC: &str = r#"
report 50300 "ALT RQ"
{
    ProcessingOnly = true;
    requestpage { layout { area(Content) { field(OptF; Opt) { ApplicationArea = All; } } } }
    var Opt: Integer;
    trigger OnPreReport() begin RequestOptionsPage.Update(false); end;
    procedure IsPreview(): Boolean begin exit(CurrReport.Preview()); end;
}
codeunit 50100 C
{
    procedure Translate(): Text begin exit(CaptionClassTranslate('3,Foo')); end;
    procedure PageEditable(var P: TestRequestPage "ALT RQ") begin if P.Editable() then; end;
}
"#;

/// Why `routine` of object `object` cannot be lowered.
fn refusal(object: &str, routine: &str) -> String {
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), SRC.into())],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let oi = l.program.objects.iter().position(|o| o.name == object).unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name.eq_ignore_ascii_case(routine))
        .unwrap();
    l.unsupported[oi][ri].first().cloned().unwrap_or_default()
}

#[test]
fn request_page_update_and_report_preview_are_refused_with_their_reasons() {
    for (object, routine, name) in [
        ("ALT RQ", "OnPreReport", "RequestPage.Update"),
        ("ALT RQ", "IsPreview", "Report.Preview"),
    ] {
        let why = refusal(object, routine);
        assert!(why.contains(name), "{name}: {why}");
        assert!(why.contains("not pinned"), "{name}: {why}");
        assert!(!why.contains("is not implemented"), "{name}: {why}");
    }
}

// Issue 33: CaptionClassTranslate (44 Tests-ERM tests) and the page-level TestRequestPage.Editable (21) are
// in no corpus test (the corpus pins CaptionClass resolution of a page caption, TestPageCaptionClass.al:90,
// and the control-level Editable, TestReqPageBoundVisibility.al, not these two calls).
#[test]
fn caption_class_translate_and_request_page_editable_are_refused_with_their_reasons() {
    for (object, routine, name) in [
        ("C", "Translate", "CaptionClassTranslate"),
        ("C", "PageEditable", "TestRequestPage.Editable"),
    ] {
        let why = refusal(object, routine);
        assert!(why.contains(name), "{name}: {why}");
        assert!(why.contains("not pinned"), "{name}: {why}");
        assert!(!why.contains("is not implemented"), "{name}: {why}");
    }
}
