//! RecordRef / FieldRef / KeyRef end to end: AL source → compile → run.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const TABLE: &str = r#"
table 50900 "ALT T"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amount; Decimal) { }
        field(3; Grp; Integer) { }
        field(4; Descr; Text[50]) { }
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

#[test]
fn open_number_and_isempty() {
    // corpus recordref/TestRecordRefOpen.al:14-37
    let r = run(
        "procedure F(): Text var RR: RecordRef; begin
            RR.Open(50900);
            exit(Format(RR.Number) + Format(RR.IsEmpty()));
        end;",
        "F",
    );
    assert_eq!(text(r), "50900Yes");
}

fn int(r: Result<Value, Signal>) -> i32 {
    match r {
        Ok(Value::Integer(i)) => i,
        other => panic!("{other:?}"),
    }
}

fn err(r: Result<Value, Signal>) -> String {
    match r {
        Err(Signal::Error(e)) => e.message,
        other => panic!("expected an AL error, got {other:?}"),
    }
}

fn unsupported(r: Result<Value, Signal>) -> String {
    match r {
        Err(Signal::Unsupported(m)) => m,
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn insert_through_fieldref_is_visible_to_record_and_ref() {
    // corpus recordref/TestRecordRefCRUD.al:16-27, TestRecordVsRecordRef.al:8-30
    let r = run(
        "procedure F(): Text var RR: RecordRef; FR: FieldRef; R: Record \"ALT T\"; begin
            RR.Open(50900);
            RR.Init();
            FR := RR.Field(1);
            FR.Value := 'a';
            RR.Field(3).Value := 42;
            RR.Insert();
            R.Get('A');
            exit(Format(RR.Count()) + '/' + Format(R.Grp));
        end;",
        "F",
    );
    assert_eq!(text(r), "1/42");
}

#[test]
fn fieldref_value_reads_the_found_row() {
    // corpus recordref/TestRecordRefCRUD.al:98-115 (FindLast then Field(1).Value)
    let r = run(
        "procedure F(): Integer var RR: RecordRef; R: Record \"ALT T\"; i: Integer; begin
            for i := 1 to 3 do begin R.\"No.\" := Format(i); R.Grp := i * 10; R.Insert(); end;
            RR.Open(50900);
            RR.FindLast();
            exit(RR.Field(3).Value());
        end;",
        "F",
    );
    assert_eq!(int(r), 30);
}

#[test]
fn findset_next_walks_a_fieldref_range() {
    // corpus recordref/TestRecordRefFilter.al:62-85
    let r = run(
        "procedure F(): Integer var RR: RecordRef; FR: FieldRef; R: Record \"ALT T\"; i: Integer; n: Integer; begin
            for i := 1 to 5 do begin R.\"No.\" := Format(i); R.Grp := i; R.Insert(); end;
            RR.Open(50900);
            FR := RR.Field(3);
            FR.SetRange(2, 4);
            if RR.FindSet() then
                repeat n += 1; until RR.Next() = 0;
            exit(n);
        end;",
        "F",
    );
    assert_eq!(int(r), 3);
}

#[test]
fn modify_delete_deleteall_and_rename_through_the_ref() {
    // corpus recordref/TestRecordRefCRUD.al:30-84, TestRecordVsRecordRef.al:141-167
    let r = run(
        "procedure F(): Text var RR: RecordRef; R: Record \"ALT T\"; s: Text; begin
            R.\"No.\" := 'A'; R.Grp := 1; R.Insert();
            R.\"No.\" := 'B'; R.Grp := 2; R.Insert();
            R.\"No.\" := 'C'; R.Grp := 3; R.Insert();
            RR.Open(50900);
            RR.Field(1).SetRange('A');
            RR.FindFirst();
            RR.Field(3).Value := 99;
            RR.Modify();
            R.Get('A'); s += Format(R.Grp);
            RR.Rename('Z');
            s += Format(R.Get('A')) + Format(R.Get('Z'));
            RR.Reset();
            RR.Field(1).SetRange('B');
            RR.FindFirst();
            RR.Delete();
            RR.Reset();
            s += Format(RR.Count());
            RR.DeleteAll();
            s += Format(RR.IsEmpty());
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "99NoYes2Yes");
}

#[test]
fn field_metadata_number_name_length_active_class() {
    // corpus recordref/TestRecordRefField.al:14-23,118-131; fieldref/TestFieldRefMetadata.al:100-136
    let r = run(
        "procedure F(): Text var RR: RecordRef; FR: FieldRef; begin
            RR.Open(50900);
            FR := RR.Field(4);
            exit(Format(FR.Number) + FR.Name + Format(FR.Length()) + Format(FR.Active()) + Format(RR.FieldCount() >= 4) + Format(RR.FieldExist(3)) + Format(RR.FieldExist(99)));
        end;",
        "F",
    );
    assert_eq!(text(r), "4Descr50YesYesYesNo");
}

#[test]
fn open_of_a_missing_table_raises_an_al_error() {
    // corpus recordref/TestRecordRefOpenMissingTable.al:31-49
    let m = err(run("procedure F() var RR: RecordRef; begin RR.Open(99991); end;", "F"));
    assert!(
        m.contains("You tried to invoke the Table object with the ID 99991 from the object"),
        "{m}"
    );
    assert!(
        m.contains("An object with that ID does not exist in the current application"),
        "{m}"
    );
}

#[test]
fn keyref_counts_and_names_key_fields() {
    // corpus recordref/TestBCMetadataContracts.al:30-66, TestRecordRefKeys.al
    let r = run(
        "procedure F(): Text var RR: RecordRef; KR: KeyRef; begin
            RR.Open(50900);
            KR := RR.KeyIndex(2);
            exit(Format(RR.KeyCount()) + Format(KR.FieldCount()) + KR.FieldIndex(1).Name() + Format(KR.FieldIndex(2).Number()) + Format(RR.KeyIndex(1).Active()));
        end;",
        "F",
    );
    assert_eq!(text(r), "22Grp1Yes");
}

#[test]
fn gettable_copies_the_record_in_settable_copies_it_out() {
    // corpus recordref/TestRecordRefGetSetTableContracts.al:30-60 (GetTable leaves the Record
    // alone, the ref takes the Record's buffer), :106-130 (SetTable: Rec is overwritten by the ref).
    let r = run(
        "procedure F(): Text var RR: RecordRef; R: Record \"ALT T\"; R2: Record \"ALT T\"; s: Text; begin
            R.\"No.\" := 'K'; R.Grp := 7; R.Insert();
            RR.Open(50900);
            RR.Field(3).Value := 5;
            RR.SetTable(R2);
            s += Format(R2.Grp);
            R2.Grp := 8;
            RR.GetTable(R2);
            s += Format(RR.Field(3).Value());
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "58");
}

#[test]
fn clear_closes_the_ref() {
    // corpus recordref/TestRecordRefOpenCompilationTarget.al:70-81 (Clear(RecRef) then Open again)
    let r = run(
        "procedure F(): Integer var RR: RecordRef; begin
            RR.Open(50900); Clear(RR); RR.Open(50900);
            exit(RR.Number);
        end;",
        "F",
    );
    assert_eq!(int(r), 50900);
}

#[test]
fn a_ref_that_is_not_open_is_refused_not_guessed() {
    let m = unsupported(run(
        "procedure F(): Integer var RR: RecordRef; begin exit(RR.Count()); end;",
        "F",
    ));
    assert!(m.contains("not open"), "{m}");
}
