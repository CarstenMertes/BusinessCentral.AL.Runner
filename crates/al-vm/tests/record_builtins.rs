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

#[test]
fn ascending_current_key_and_is_temporary() {
    // corpus record/TestRecordSort.al:111-112 (default true), TestSortingKeyContracts.al:224,
    // TestCopyOntoTemporaryRecord.al:89-93 (CurrentKey is the key's field names)
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; T: Record \"ALT T\" temporary; s: Text; begin
            s := Format(R.Ascending()) + '|' + R.CurrentKey() + '|';
            R.Ascending(false);
            R.SetCurrentKey(Grp);
            s += Format(R.Ascending()) + '|' + R.CurrentKey() + '|' + Format(R.IsTemporary()) + Format(T.IsTemporary);
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "Yes|No.|No|Grp,No.|NoYes");
}

#[test]
fn filters_read_back_per_field_and_in_total() {
    // corpus record/TestCopyOntoTemporaryRecord.al:91-92 ('<>L0001', 'Included'), TestFilterAdvancedContracts.al:204-211
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; s: Text; begin
            s := Format(R.HasFilter()) + '[' + R.GetFilters() + ']';
            R.SetRange(Grp, 1, 3);
            R.SetFilter(\"No.\", '<>%1', 'A');
            s += Format(R.HasFilter()) + R.GetFilter(Grp) + '|' + R.GetFilter(\"No.\") + '|' + R.GetFilter(Descr) + '|' + R.GetFilters();
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "No[]Yes1..3|<>A||No.: <>A, Grp: 1..3");
}

#[test]
fn getview_setview_round_trip_and_setrecfilter() {
    // corpus record/TestFilterAdvancedContracts.al:125-141,144-160, TestRecordFilter.al:344-385
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; R2: Record \"ALT T\"; i: Integer; s: Text; begin
            for i := 1 to 5 do begin R.\"No.\" := Format(i); R.Grp := i; R.Insert(); end;
            R.SetRange(Grp, 2, 4);
            s := Format(R.GetView() <> '') + Format(StrPos(R.GetView(true), 'Grp') > 0);
            R2.SetView(R.GetView());
            s += Format(R2.Count()) + R2.GetView(false);
            R.Reset();
            R.Get('5');
            R.SetRecFilter();
            s += '|' + Format(R.Count()) + R.GetFilter(\"No.\") + Format(R.HasFilter());
            exit(s);
        end;",
        "F",
    );
    assert_eq!(
        text(r),
        "YesYes3SORTING(Field1) WHERE(Field3=FILTER(2..4))|1".to_string() + "5Yes"
    );
}

#[test]
fn copy_filters_and_copy_onto_a_temporary_record() {
    // corpus record/TestFilterContracts.al:137-152 (CopyFilters), TestCopyOntoTemporaryRecord.al:66-110
    let r = run(
        "procedure F(): Text var Src: Record \"ALT T\"; B: Record \"ALT T\" temporary; C: Record \"ALT T\"; i: Integer; s: Text; begin
            for i := 1 to 4 do begin B.\"No.\" := 'T' + Format(i); B.Grp := i; B.Insert(); end;
            Src.\"No.\" := 'S1'; Src.Insert();
            Src.SetCurrentKey(Grp);
            Src.Ascending(false);
            Src.SetFilter(Grp, '>1');
            B.Copy(Src);
            s := Format(B.IsTemporary) + Format(B.Count()) + B.GetFilter(Grp) + '|' + B.CurrentKey() + Format(B.Ascending());
            B.FindFirst();
            s += B.\"No.\";
            C.CopyFilters(Src);
            s += '|' + Format(C.Count());
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "Yes3>1|Grp,No.NoT4|0");
}

#[test]
fn filter_groups_combine_as_and_and_reset_clears_all() {
    // corpus record/TestFilterContracts.al:98-113,154-175, TestFilterAdvancedContracts.al:181-212,
    // TestRecordFilter.al:270-305
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; i: Integer; s: Text; begin
            for i := 1 to 5 do begin R.\"No.\" := Format(i); R.Grp := i; R.Insert(); end;
            s := Format(R.FilterGroup());
            R.FilterGroup(2);
            s += Format(R.FilterGroup());
            R.SetRange(Grp, 1, 3);
            s += '|' + R.GetFilter(Grp);
            R.FilterGroup(0);
            s += '|' + R.GetFilter(Grp) + '|' + Format(R.Count());
            R.SetRange(Grp, 2, 5);
            s += '|' + Format(R.Count()) + R.GetFilter(Grp);
            R.Reset();
            s += '|' + Format(R.Count());
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "02|1..3||3|22..5|5");
}

#[test]
fn setrecfilter_with_a_filter_on_another_field_is_refused_not_guessed() {
    let r = run(
        "procedure F() var R: Record \"ALT T\"; begin R.SetRange(Grp, 1); R.\"No.\" := 'A'; R.SetRecFilter(); end;",
        "F",
    );
    assert!(
        matches!(r, Err(Signal::Unsupported(ref m)) if m.contains("SetRecFilter")),
        "{r:?}"
    );
}

#[test]
fn insert_with_system_id_keeps_the_assigned_id_and_get_by_system_id_finds_the_row() {
    // corpus record/TestRecordInsert.al:84-110, TestRecordSystemId.al:10-60,285-330
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; F: Record \"ALT T\"; G: Guid; s: Text; begin
            G := CreateGuid();
            R.\"No.\" := 'A'; R.SystemId := G; R.Insert(false, true);
            s := Format(R.SystemId = G);
            R.\"No.\" := 'B'; R.SystemId := G; R.Insert(false, false);
            s += Format(R.SystemId = G) + Format(IsNullGuid(R.SystemId));
            s += '|' + Format(F.GetBySystemId(G)) + F.\"No.\" + Format(F.GetBySystemId(CreateGuid()));
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "YesNoNo|YesANo");
}

#[test]
fn a_duplicate_system_id_is_refused_until_the_row_is_gone() {
    // corpus record/TestRecordSystemId.al:86-168 ('unique index', the caption and the quoted value),
    // :330-400 (a deleted or deleted-all row frees its id)
    let m = error_text(
        "R.\"No.\" := 'B'; R.SystemId := CreateGuid(); R.Insert(false, true); R.\"No.\" := 'C'; R.Insert(false, true);",
    );
    assert!(m.contains("unique index"), "{m}");
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; S: Record \"ALT T\"; G: Guid; begin
            G := CreateGuid();
            R.\"No.\" := 'A'; R.SystemId := G; R.Insert(false, true);
            R.Delete();
            S.\"No.\" := 'B'; S.SystemId := G; S.Insert(false, true);
            exit(Format(R.GetBySystemId(G)) + R.\"No.\");
        end;",
        "F",
    );
    assert_eq!(text(r), "YesB");
    // The error names the field by its caption and quotes the formatted value.
    let m = text(run(
        "procedure F(): Text var R: Record \"ALT T\"; S: Record \"ALT T\"; G: Guid; begin
            G := CreateGuid();
            R.\"No.\" := 'A'; R.SystemId := G; R.Insert(false, true);
            S.\"No.\" := 'B'; S.SystemId := G; asserterror S.Insert(false, true);
            exit(GetLastErrorText() + '#' + Format(G));
        end;",
        "F",
    ));
    let (msg, g) = m.split_once('#').unwrap();
    assert!(msg.contains(&format!("the following fields: System ID='{g}'")), "{msg}");
}

#[test]
fn modify_never_changes_the_stored_system_id() {
    // corpus record/TestRecordSystemId.al:176-215,285-300: the buffer's SystemId is not written
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; F: Record \"ALT T\"; A: Guid; B: Guid; begin
            R.\"No.\" := 'A'; R.Insert(); A := R.SystemId; Clear(R);
            R.\"No.\" := 'B'; R.Insert(); B := R.SystemId;
            R.Get('A');
            R.\"No.\" := 'B'; R.Grp := 42; R.Modify();
            F.Get('B');
            exit(Format(F.SystemId = B) + Format(F.Grp) + Format(F.GetBySystemId(A)) + F.\"No.\");
        end;",
        "F",
    );
    assert_eq!(text(r), "Yes42YesA");
}

#[test]
fn a_refused_write_hands_the_buffer_no_row_version() {
    // corpus record/TestRowVersion*: 60247 RefusedInsert_/RefusedModify_LeavesRecordWithoutRowVersion
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; D: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Insert();
            D.\"No.\" := 'A';
            D.Insert();
            exit(Format(D.SystemRowVersion));
        end;",
        "F",
    );
    // Insert() without using the result raises on a duplicate; the buffer stays at 0.
    assert!(matches!(r, Err(Signal::Error(_))), "{r:?}");
    let r = run(
        "procedure F(): BigInteger var R: Record \"ALT T\"; D: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Insert();
            D.\"No.\" := 'A';
            if not D.Insert() then;
            exit(D.SystemRowVersion);
        end;",
        "F",
    );
    assert!(matches!(r, Ok(Value::BigInteger(0))), "{r:?}");
}

#[test]
fn modify_all_updates_every_filtered_row_and_refuses_system_id() {
    // corpus record/TestRecordModify.al:87-142, TestRecordSystemId.al:215-270
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; C: Record \"ALT T\"; i: Integer; begin
            for i := 1 to 5 do begin R.\"No.\" := Format(i); R.Grp := i; R.Insert(); end;
            R.SetRange(Grp, 1, 3);
            R.ModifyAll(Grp, 77, true);
            C.SetRange(Grp, 77);
            exit(Format(C.Count()) + Format(R.Count()));
        end;",
        "F",
    );
    assert_eq!(text(r), "30");
    let m = error_text("R.ModifyAll(SystemId, CreateGuid());");
    assert!(m.contains("cannot change the value of the SystemId field"), "{m}");
}

#[test]
fn get_range_min_and_max_read_the_set_range() {
    // corpus record/TestFilterContracts.al:214-235, TestRecordFilter.al:436-458
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.SetRange(Grp, 5, 10);
            exit(Format(R.GetRangeMin(Grp)) + '|' + Format(R.GetRangeMax(Grp)));
        end;",
        "F",
    );
    assert_eq!(text(r), "5|10");
    // Without a filter BC raises an error whose text the corpus does not pin: refused.
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin exit(Format(R.GetRangeMin(Grp))); end;",
        "F",
    );
    assert!(
        matches!(r, Err(Signal::Unsupported(ref m)) if m.contains("GetRangeMin")),
        "{r:?}"
    );
}

#[test]
fn marks_restrict_the_set_when_marked_only() {
    // corpus record/TestRecordFind.al:225-300, TestRecordNavContracts.al:283-302
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; s: Text; i: Integer; begin
            for i := 1 to 3 do begin R.\"No.\" := Format(i); R.Insert(); end;
            R.MarkedOnly(true);
            s := Format(R.FindFirst());
            R.MarkedOnly(false);
            R.Get('2');
            R.Mark(true);
            s += Format(R.Mark());
            R.MarkedOnly(true);
            R.FindFirst();
            s += R.\"No.\" + Format(R.Count());
            R.Mark(false);
            s += Format(R.FindFirst());
            R.MarkedOnly(false);
            R.Get('1'); R.Mark(true); R.Get('3'); R.Mark(true);
            R.MarkedOnly(true);
            s += Format(R.Count());
            R.ClearMarks();
            s += Format(R.Count());
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "NoYes21No20");
}

#[test]
fn a_table_of_table_type_temporary_is_temporary_for_every_variable_and_ref() {
    // corpus record (60198) TempOnly_IsTemporary_AlwaysTrue: TableType = Temporary without the modifier
    let src = r#"
table 50940 "Temp Only" { TableType = Temporary; fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
codeunit 50100 C { procedure F(): Text var R: Record "Temp Only"; S: Record "Temp Only"; RR: RecordRef; begin
    R.K := 1; R.Insert();
    RR.Open(50940);
    exit(Format(R.IsTemporary) + Format(S.Count()) + Format(RR.IsTemporary())); end; }
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
    assert_eq!(text(r), "Yes0Yes");
}
