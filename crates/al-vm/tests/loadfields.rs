//! Partial records: `SetLoadFields`, `AddLoadFields`, `AreFieldsLoaded`. The only thing AL can
//! observe is `AreFieldsLoaded` (a read of an omitted field loads it), so these tests assert it.
//! Each test names the corpus test that pins what it asserts.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const TABLES: &str = r#"
table 50800 "LF Keyed"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; Name; Text[50]) { }
        field(3; Code; Code[20]) { }
        field(4; Amount; Decimal) { }
        field(5; Note; Text[50]) { }
    }
    keys { key(PK; "Entry No.") { } key(Key2; Amount) { } }
}
"#;

fn text(vars: &str, body: &str) -> Result<String, Signal> {
    let src = format!(
        "{TABLES}codeunit 50100 C {{ procedure F(): Text var Rec: Record \"LF Keyed\"; a: Text; b: Boolean; {vars} begin {body} end; }}"
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
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l
        .program
        .objects
        .iter()
        .position(|o| o.routines.iter().any(|r| r.name == "F"))
        .expect("object");
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == "F")
        .unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    match s.call(oi, ri, inst, vec![])?.0 {
        Value::Text(t) => Ok(t),
        other => panic!("{other:?}"),
    }
}

/// Seeds row 1 (Name alpha, Code A1, Amount 75, Note n1) and row 2 (beta, B2, 5, n2).
const SEED: &str = "Rec.Init(); Rec.\"Entry No.\" := 1; Rec.Name := 'alpha'; Rec.Code := 'A1'; Rec.Amount := 75; Rec.Note := 'n1'; Rec.Insert();
    Rec.\"Entry No.\" := 2; Rec.Name := 'beta'; Rec.Code := 'B2'; Rec.Amount := 5; Rec.Note := 'n2'; Rec.Insert();
    Clear(Rec);";

fn yn(b: &str) -> String {
    format!("a += Format({b});")
}

#[test]
fn set_load_fields_without_arguments_resets_to_a_full_load() {
    // record/TestRecordPartialLoad.al:61-99 (PartialLoad_SetLoadFieldsNoArgs_ResetsToFullLoad).
    let body = format!(
        "{SEED} Rec.SetLoadFields(Rec.Amount); Rec.Get(1); {} Rec.SetLoadFields(); Rec.Get(1); {} {} exit(a);",
        yn("Rec.AreFieldsLoaded(Rec.Note)"),
        yn("Rec.AreFieldsLoaded(Rec.Note)"),
        yn("Rec.Note = 'n1'")
    );
    assert_eq!(text("", &body).unwrap(), "NoYesYes");
}

#[test]
fn are_fields_loaded_is_false_before_any_fetch() {
    // record/TestRecordPartialLoad.al:102-141 (PartialLoad_AreFieldsLoaded_BeforeFetch_ReturnsFalse).
    let body = format!(
        "{SEED} Rec.SetLoadFields(Rec.Amount); {} {} Rec.Get(2); {} exit(a);",
        yn("Rec.AreFieldsLoaded(Rec.Amount)"),
        yn("Rec.AreFieldsLoaded(Rec.\"Entry No.\")"),
        yn("Rec.AreFieldsLoaded(Rec.Amount)")
    );
    assert_eq!(text("", &body).unwrap(), "NoNoYes");
}

#[test]
fn reading_an_omitted_field_loads_its_real_value() {
    // record/TestRecordPartialLoad.al:144-190 (PartialLoad_ReadOmittedField_JitLoadsRealValue).
    let body = format!(
        "{SEED} Rec.SetLoadFields(Rec.Amount); Rec.Get(1); {} a += Rec.Name; {} {} exit(a);",
        yn("Rec.AreFieldsLoaded(Rec.Name)"),
        yn("Rec.AreFieldsLoaded(Rec.Name)"),
        yn("Rec.AreFieldsLoaded(Rec.Note)")
    );
    assert_eq!(text("", &body).unwrap(), "NoalphaYesNo");
}

#[test]
fn the_fields_of_the_current_key_are_loaded_too() {
    // record/TestRecordPartialLoad.al:193-232 (PartialLoad_CurrentKeyFields_AreForceLoaded).
    let body = format!(
        "{SEED} Rec.SetCurrentKey(\"Entry No.\"); Rec.SetLoadFields(Rec.Name); Rec.FindFirst(); {}
         Clear(Rec); Rec.SetCurrentKey(Amount); Rec.SetLoadFields(Rec.Name); Rec.FindFirst(); {} {} exit(a);",
        yn("Rec.AreFieldsLoaded(Rec.Amount)"),
        yn("Rec.AreFieldsLoaded(Rec.Amount)"),
        yn("Rec.Amount")
    );
    assert_eq!(text("", &body).unwrap(), "NoYes5");
}

#[test]
fn the_fields_of_an_active_filter_are_loaded_too() {
    // record/TestRecordPartialLoad.al:235-285 (PartialLoad_FilteredFields_AreForceLoaded).
    let body = format!(
        "{SEED} Rec.SetLoadFields(Rec.Name); Rec.SetRange(\"Entry No.\", 2); Rec.FindFirst(); {}
         Clear(Rec); Rec.SetLoadFields(Rec.Name); Rec.SetRange(Code, 'B2'); Rec.FindFirst(); {} a += Rec.Code; exit(a);",
        yn("Rec.AreFieldsLoaded(Rec.Code)"),
        yn("Rec.AreFieldsLoaded(Rec.Code)")
    );
    assert_eq!(text("", &body).unwrap(), "NoYesB2");
}

#[test]
fn narrowing_after_a_full_fetch_keeps_the_buffer_in_hand() {
    // record/TestRecordPartialLoadNarrowAfterFetch.al:56-85 (..._AfterFetch_NoRefetch_ReportsBufferInHand).
    let body = format!(
        "{SEED} Rec.Get(1); {} Rec.SetLoadFields(Rec.Amount); {} a += Rec.Note; exit(a);",
        yn("Rec.AreFieldsLoaded(Rec.Note)"),
        yn("Rec.AreFieldsLoaded(Rec.Note)")
    );
    assert_eq!(text("", &body).unwrap(), "YesYesn1");
}

#[test]
fn widening_without_a_fetch_loads_nothing() {
    // record/TestRecordPartialLoadNarrowAfterFetch.al:88-118 (..._AfterNarrowFetch_WidenNoRefetch_StaysUnloaded).
    let body = format!(
        "{SEED} Rec.SetLoadFields(Rec.Amount); Rec.Get(1); {} Rec.SetLoadFields(); {} exit(a);",
        yn("Rec.AreFieldsLoaded(Rec.Note)"),
        yn("Rec.AreFieldsLoaded(Rec.Note)")
    );
    assert_eq!(text("", &body).unwrap(), "NoNo");
}

#[test]
fn a_refetch_after_narrowing_keeps_what_a_full_fetch_loaded() {
    // record/TestRecordPartialLoadNarrowAfterFetch.al:121-185 (..._AfterFetch_WithRefetch_KeepsTheFieldLoaded),
    // including the negative half on a row nobody fetched wide.
    let body = format!(
        "{SEED} Rec.Get(1); Rec.SetLoadFields(Rec.Amount); Rec.Get(1); {} a += Rec.Note;
         Clear(Rec); Rec.SetLoadFields(Rec.Amount); Rec.Get(2); {} exit(a);",
        yn("Rec.AreFieldsLoaded(Rec.Note)"),
        yn("Rec.AreFieldsLoaded(Rec.Note)")
    );
    assert_eq!(text("", &body).unwrap(), "Yesn1No");
}

#[test]
fn are_fields_loaded_takes_several_fields_and_needs_all_of_them() {
    let body = format!(
        "{SEED} Rec.SetLoadFields(Rec.Amount, Rec.Name); Rec.Get(1); {} {} exit(a);",
        yn("Rec.AreFieldsLoaded(Rec.Amount, Rec.Name)"),
        yn("Rec.AreFieldsLoaded(Rec.Amount, Rec.Note)")
    );
    assert_eq!(text("", &body).unwrap(), "YesNo");
}

#[test]
fn add_load_fields_extends_a_partial_load_for_the_next_fetch() {
    // record/TestRecordExtended.al:44-65 only call it; the documented effect is that the next
    // fetch also loads the added fields (probes-needed: the return value).
    let body = format!(
        "{SEED} Rec.SetLoadFields(Rec.Amount); Rec.AddLoadFields(Rec.Name); Rec.Get(1); {} {} exit(a);",
        yn("Rec.AreFieldsLoaded(Rec.Name)"),
        yn("Rec.AreFieldsLoaded(Rec.Note)")
    );
    assert_eq!(text("", &body).unwrap(), "YesNo");
}

#[test]
fn add_load_fields_on_a_full_load_does_nothing_and_answers_false() {
    // record/TestRecordExtended.al:44-65 (callable; the result is not asserted).
    let body = format!(
        "{SEED} b := Rec.AddLoadFields(Rec.Name); a := Format(b); Rec.Get(1); {} exit(a);",
        yn("Rec.AreFieldsLoaded(Rec.Note)")
    );
    assert_eq!(text("", &body).unwrap(), "NoYes");
}

#[test]
fn unpinned_cases_are_refused() {
    // A primary key field under a secondary current key, a record that was written but never
    // fetched, and SetBaseLoadFields are not pinned.
    let r = text(
        "",
        &format!(
            "{SEED} Rec.SetCurrentKey(Amount); Rec.SetLoadFields(Rec.Name); Rec.FindFirst(); exit(Format(Rec.AreFieldsLoaded(Rec.\"Entry No.\")));"
        ),
    );
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
    let r = text(
        "",
        "Rec.\"Entry No.\" := 9; exit(Format(Rec.AreFieldsLoaded(Rec.\"Entry No.\")));",
    );
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
    let r = text(
        "",
        &format!("{SEED} Rec.SetLoadFields(Rec.Name); exit(Format(Rec.SetBaseLoadFields()));"),
    );
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}

#[test]
fn set_base_load_fields_on_a_full_load_is_callable_and_answers_false() {
    // record/TestRecordExtended.al:71-80 (Record_SetBaseLoadFields_IsCallable); the result is
    // the test's comment, not an assertion (probes-needed).
    assert_eq!(text("", "exit(Format(Rec.SetBaseLoadFields()));").unwrap(), "No");
}
