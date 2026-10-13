//! Record builtins that need table metadata: AL source → compile → run.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const TABLE: &str = r#"
table 50900 "ALT T"
{
    Caption = 'The Table';
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amount; Decimal) { Caption = 'The Amount'; }
        field(3; Grp; Integer) { }
        field(4; Descr; Text[50]) { }
        field(5; Flag; Boolean) { }
        field(6; Total; Decimal) { FieldClass = FlowField; }
    }
    keys { key(PK; "No.") { } key(ByGrp; Grp, "No.") { } }
}
"#;

fn run(body: &str, routine: &str) -> Result<Value, Signal> {
    let src = format!("{TABLE}\ncodeunit 50100 C {{ {body} }}");
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
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .unwrap();
    if let Some(Some(r)) = blocked.get(oi).and_then(|o| o.get(ri)) {
        panic!("{routine} is blocked: {r}");
    }
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

/// The message of the AL error a routine raises.
fn error_text(body: &str) -> String {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"ALT T\"; begin R.\"No.\" := 'A'; R.Amount := 0; R.Grp := 7; R.Insert(); asserterror begin {body} end; exit(GetLastErrorText()); end;"
        ),
        "F",
    );
    text(r)
}

#[test]
fn field_and_table_metadata() {
    // corpus fieldref/TestPrecompiledFieldOptionCaption.al:100-125 (Caption property, else the name),
    // record/TestRecordTableCaption.al, recordref/TestRecordRefField.al:118-131
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            exit(R.FieldCaption(Amount) + '|' + R.FieldCaption(Grp) + '|' + R.FieldName(Amount) + '|' + Format(R.FieldNo(Grp)) + '|' + R.TableCaption() + '|' + R.TableName());
        end;",
        "F",
    );
    assert_eq!(text(r), "The Amount|Grp|Amount|3|The Table|ALT T");
}

#[test]
fn testfield_on_an_empty_field_names_field_table_and_key() {
    // corpus record/TestRecordTestFieldLookupPageId_Tests.al:54-72 (exact text)
    assert_eq!(
        error_text("R.TestField(Amount);"),
        "The Amount must have a value in The Table: No.=A. It cannot be zero or empty."
    );
}

#[test]
fn testfield_passes_on_a_populated_field_and_on_a_matching_value() {
    // corpus record/TestRecordTestField.al:25-40, :60-72
    let r = run(
        "procedure F(): Integer var R: Record \"ALT T\"; begin
            R.Grp := 7; R.TestField(Grp); R.TestField(Grp, 7); R.Descr := 'x'; R.TestField(Descr, 'x');
            exit(R.Grp);
        end;",
        "F",
    );
    assert_eq!(text_or_int(r), 7);
}

fn text_or_int(r: Result<Value, Signal>) -> i32 {
    match r {
        Ok(Value::Integer(i)) => i,
        other => panic!("{other:?}"),
    }
}

#[test]
fn testfield_with_a_wrong_value_names_the_field() {
    // corpus error-handling/TestErrorTextContracts.al:68-79 pins only the field caption; the
    // rest of the wording is Assert.al's documented format (docs/probes-needed.md).
    let m = error_text("R.TestField(Grp, 99);");
    assert!(m.contains("Grp"), "{m}");
    assert!(m.contains("99") && m.contains("7"), "{m}");
}

#[test]
fn testfield_rejects_every_kind_of_empty_value() {
    // corpus record/TestRecordTestField.al:42-52 (Text), :246-262 (Code), BigInteger, Decimal, Boolean
    let m = error_text("R.TestField(Descr);");
    assert!(m.starts_with("Descr must have a value in"), "{m}");
    let m = error_text("R.TestField(\"No.\"); R.Flag := false; R.TestField(Flag);");
    assert!(m.starts_with("Flag must have a value in"), "{m}");
}

#[test]
fn truncate_deletes_every_row_and_returns_true() {
    // corpus record/TestRecordDelete.al:169-188, TestBCSystemFieldContracts.al:180-205
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; ok: Boolean; begin
            R.\"No.\" := 'A'; R.Insert(); R.\"No.\" := 'B'; R.Insert();
            ok := R.Truncate(false);
            exit(Format(ok) + Format(R.IsEmpty()));
        end;",
        "F",
    );
    assert_eq!(text(r), "YesYes");
}

#[test]
fn truncate_on_an_empty_table_does_not_throw() {
    // corpus record/TestBCSystemFieldContracts.al:207-215
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin R.Truncate(); exit('ok'); end;",
        "F",
    );
    assert_eq!(text(r), "ok");
}

#[test]
fn truncate_refuses_a_filter_on_a_flowfield() {
    // corpus record/TestRecordTruncateEventAndFilterGuards.al:112-125 (exact text)
    assert_eq!(
        error_text("R.SetRange(Total, 0, 100); R.Truncate();"),
        "Truncate does not support filters on FlowFields."
    );
}

#[test]
fn truncate_inside_a_try_function_is_refused_even_nested() {
    // corpus record/TestRecordTruncateTryScope.al:36-110 (exact text, direct and nested)
    let r = run(
        "[TryFunction] local procedure Direct() var R: Record \"ALT T\"; begin R.Truncate(); end;
         local procedure Helper() var R: Record \"ALT T\"; begin R.Truncate(); end;
         [TryFunction] local procedure Nested() begin Helper(); end;
         procedure F(): Text var s: Text; begin
            if Direct() then s := 'direct ok';
            s += GetLastErrorText();
            if Nested() then s += 'nested ok';
            s += '/' + GetLastErrorText();
            Helper();
            exit(s);
        end;",
        "F",
    );
    assert_eq!(
        text(r),
        "Truncate is not supported in try functions./Truncate is not supported in try functions."
    );
}

#[test]
fn truncate_with_a_normal_filter_or_reset_counter_is_refused_not_guessed() {
    // corpus record/TestRecordTruncateEventAndFilterGuards.al:140-160 only pins a filter that
    // matches every row, so whether the filter limits the delete is not known.
    let r = run(
        "procedure F() var R: Record \"ALT T\"; begin R.SetRange(Grp, 1); R.Truncate(); end;",
        "F",
    );
    assert!(
        matches!(r, Err(Signal::Unsupported(ref m)) if m.contains("filter")),
        "{r:?}"
    );
}

#[test]
fn rename_is_refused_when_an_else_arm_of_a_conditional_relation_points_at_the_table() {
    // corpus record/TestRenamePropagationConditional.al:71-90: a rename must follow the else
    // arm's table too, so with re-keying unimplemented the rename of that table is refused.
    let src = r#"
table 50910 "Parent B" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50911 "Parent A" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50912 "Child" {
    fields {
        field(1; Id; Integer) { }
        field(2; Kind; Option) { OptionMembers = A,B; }
        field(3; Ref; Code[20]) { TableRelation = if (Kind = const(A)) "Parent A".Code else "Parent B".Code; }
    }
    keys { key(PK; Id) { } }
}
codeunit 50100 C { procedure F() var P: Record "Parent B"; begin P.Code := 'OLD'; P.Insert(); P.Rename('NEW'); end; }
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
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    let r = s.call(oi, 0, inst, vec![]);
    assert!(
        matches!(r, Err(Signal::Unsupported(ref m)) if m.contains("re-keying")),
        "{r:?}"
    );
}

#[test]
fn system_field_captions_only_where_the_corpus_pins_them() {
    // corpus record/TestRecordSystemId.al:178 pins SystemId's caption; the other system fields'
    // captions are not pinned, so asking for them is refused rather than answered with the name.
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin exit(R.FieldCaption(SystemId)); end;",
        "F",
    );
    assert_eq!(text(r), "System ID");
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin exit(R.FieldCaption(SystemCreatedAt)); end;",
        "F",
    );
    assert!(
        matches!(r, Err(Signal::Unsupported(ref m)) if m.contains("SystemCreatedAt")),
        "{r:?}"
    );
}

#[test]
fn a_tableextension_modify_changes_the_caption_the_ref_and_the_record_answer() {
    // corpus tableextension/TestTableExtFieldModify.al:40-100
    let src = r#"
table 50930 T { fields { field(1; A; Integer) { Caption = 'Orig A'; } field(2; B; Integer) { Caption = 'Orig B'; } } keys { key(PK; A) { } } }
tableextension 50931 X extends T { fields { modify(A) { Caption = 'New A'; } } }
codeunit 50100 C { procedure F(): Text var R: Record T; RR: RecordRef; begin RR.Open(50930); exit(R.FieldCaption(A) + '|' + R.FieldCaption(B) + '|' + RR.Field(1).Caption()); end; }
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
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    let r = s.call(oi, 0, inst, vec![]).map(|(v, _)| v);
    assert_eq!(text(r), "New A|Orig B|New A");
}
