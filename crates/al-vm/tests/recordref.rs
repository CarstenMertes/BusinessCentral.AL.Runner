//! RecordRef / FieldRef / KeyRef end to end: AL source → compile → run.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const TABLE: &str = r#"
enum 50901 "ALT E"
{
    value(0; Zero) { Caption = 'Nothing'; }
    value(5; Five) { }
    value(10; Ten) { Caption = 'Tenner'; }
}
table 50900 "ALT T"
{
    Caption = 'The Table';
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amount; Decimal) { Caption = 'The Amount'; }
        field(3; Grp; Integer) { trigger OnValidate() begin Descr := 'validated ' + Format(Grp); end; }
        field(4; Descr; Text[50]) { }
        field(5; Kind; Enum "ALT E") { }
        field(6; Opt; Option) { OptionMembers = A,B,C; OptionCaption = 'Alpha,Beta,Gamma'; }
        field(7; Flag; Boolean) { }
        field(8; Tot; Decimal) { FieldClass = FlowField; }
    }
    keys { key(PK; "No.") { } key(ByGrp; Grp, "No.") { } }
}
table 50902 "ALT Counter"
{
    fields
    {
        field(1; Id; Code[20]) { }
        field(3; Cnt; Integer) { FieldClass = FlowField; CalcFormula = count("ALT T"); }
    }
    keys { key(PK; Id) { } }
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
fn keyref_field_index_past_the_key_raises_index_out_of_bounds() {
    // types/TestTypeSystemContracts.al:240-262 pins the fragment (both operands of `and` run, so
    // FieldIndex(2) on a one-field key raises); recordref/TestBCSumIndexFieldContracts.al:188-198.
    let r = run(
        "procedure F(): Text var RR: RecordRef; KR: KeyRef; m: Boolean; begin
            RR.Open(50900);
            KR := RR.KeyIndex(1);
            asserterror m := (KR.FieldCount() = 2) and (KR.FieldIndex(2).Name() = 'x');
            exit(GetLastErrorText() + Format(m));
        end;",
        "F",
    );
    assert_eq!(text(r), "Index out of boundsNo");
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

#[test]
fn recordref_metadata_caption_company_duplicate_system_field_numbers() {
    // corpus recordref/TestRecordRefGetSet.al:16-30,53-72; TestRecordRefExtended.al; TestRecordVsRecordRef.al:333-352
    let r = run(
        "procedure F(): Text var RR: RecordRef; D: RecordRef; R: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Insert(); R.\"No.\" := 'B'; R.Insert();
            RR.Open(50900);
            D := RR.Duplicate();
            D.Field(1).SetRange('A');
            exit(RR.Caption + '|' + Format(RR.CurrentCompany() <> '') + '|' + Format(RR.Count()) + Format(D.Count()) + '|' + Format(RR.SystemIdNo()) + '|' + Format(RR.SystemModifiedAtNo()) + '|' + Format(RR.IsTemporary()));
        end;",
        "F",
    );
    assert_eq!(text(r), "The Table|Yes|21|2000000000|2000000003|No");
}

#[test]
fn views_filters_and_has_filter_on_a_recordref() {
    // corpus recordref/TestRecordRefFilter.al:14-60, 98-121; TestRecordVsRecordRef.al:150-180
    let r = run(
        "procedure F(): Text var RR: RecordRef; D: RecordRef; R: Record \"ALT T\"; i: Integer; s: Text; begin
            for i := 1 to 5 do begin R.\"No.\" := Format(i); R.Grp := i; R.Insert(); end;
            RR.Open(50900);
            s += Format(RR.HasFilter());
            RR.SetView('SORTING(Grp) WHERE(Grp=FILTER(1..3))');
            s += Format(RR.HasFilter()) + Format(RR.Count()) + Format(RR.GetFilters() <> '') + Format(RR.GetView() <> '');
            D.Open(50900);
            D.SetView(RR.GetView());
            s += Format(D.Count());
            RR.Reset();
            s += Format(RR.Count()) + Format(RR.HasFilter());
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "NoYes3YesYes3".to_string() + "5No");
}

#[test]
fn setview_sorting_picks_the_key() {
    // docs: SORTING(...) is SetCurrentKey; corpus recordref/TestRecordRefFilter.al:14-30 uses it with the primary key
    let r = run(
        "procedure F(): Text var RR: RecordRef; R: Record \"ALT T\"; s: Text; begin
            R.\"No.\" := 'A'; R.Grp := 2; R.Insert(); R.\"No.\" := 'B'; R.Grp := 1; R.Insert();
            RR.Open(50900);
            RR.SetView('SORTING(Grp,No.)');
            RR.FindFirst();
            exit(Format(RR.Field(1).Value()));
        end;",
        "F",
    );
    assert_eq!(text(r), "B");
}

#[test]
fn fieldref_caption_validate_and_getfilter() {
    // corpus fieldref/TestFieldRefMetadata.al:36-54, TestFieldRefValidate.al:10-30,70-90, TestFieldRefFilter.al:127-145
    let r = run(
        "procedure F(): Text var RR: RecordRef; FR: FieldRef; s: Text; begin
            RR.Open(50900);
            s := RR.Field(2).Caption() + '|' + RR.Field(3).Caption() + '|';
            FR := RR.Field(3);
            FR.Validate(4);
            s += Format(FR.Value()) + RR.Field(4).Value() + '|';
            FR.SetRange(1, 3);
            s += FR.GetFilter();
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "The Amount|Grp|4validated 4|1..3");
}

#[test]
fn fieldref_enum_and_option_metadata() {
    // corpus fieldref/TestFieldRefEnumOrdinalByIndex.al (index/ordinal/caption/name, out of range),
    // TestFieldRefEnum.al:112-124, TestPrecompiledFieldOptionCaption.al:34-48
    let r = run(
        "procedure F(): Text var RR: RecordRef; E: FieldRef; O: FieldRef; s: Text; begin
            RR.Open(50900);
            E := RR.Field(5);
            O := RR.Field(6);
            s := Format(E.IsEnum()) + Format(O.IsEnum()) + Format(RR.Field(3).IsEnum()) + '|' + Format(E.EnumValueCount()) + '|';
            s += Format(E.GetEnumValueOrdinal(2)) + E.GetEnumValueName(3) + E.GetEnumValueCaption(1) + E.GetEnumValueCaption(2) + '|';
            s += Format(E.GetEnumValueOrdinal(0)) + Format(E.GetEnumValueOrdinal(4)) + '[' + E.GetEnumValueCaption(4) + ']|';
            s += E.GetEnumValueNameFromOrdinalValue(10) + E.GetEnumValueCaptionFromOrdinalValue(10) + '|';
            s += O.OptionCaption() + '/' + O.OptionMembers() + '/' + O.OptionString() + '|' + Format(O.GetEnumValueOrdinal(3)) + O.GetEnumValueCaption(3);
            exit(s);
        end;",
        "F",
    );
    assert_eq!(
        text(r),
        "YesNoNo|3|5TenNothingFive|-1-1[]|TenTenner|Alpha,Beta,Gamma/A,B,C/A,B,C|2Gamma"
    );
}

#[test]
fn fieldref_testfield_and_fielderror() {
    // corpus fieldref/TestFieldRefFieldError.al:10-100
    let m = err(run(
        "procedure F() var RR: RecordRef; begin RR.Open(50900); RR.Field(3).TestField(); end;",
        "F",
    ));
    assert!(m.starts_with("Grp must have a value in The Table"), "{m}");
    let m = err(run(
        "procedure F() var RR: RecordRef; begin RR.Open(50900); RR.Field(3).Value := 1; RR.Field(3).TestField(2); end;",
        "F",
    ));
    assert!(m.starts_with("Grp must be 2 in The Table"), "{m}");
    let m = err(run(
        "procedure F() var RR: RecordRef; begin RR.Open(50900); RR.Field(3).FieldError('Custom msg'); end;",
        "F",
    ));
    assert!(m.contains("Grp") && m.contains("Custom msg"), "{m}");
}

#[test]
fn fieldref_record_gives_the_same_buffer_back() {
    let r = run(
        "procedure F(): Integer var RR: RecordRef; B: RecordRef; begin
            RR.Open(50900);
            RR.Field(3).Value := 9;
            B := RR.Field(3).Record();
            exit(B.Field(3).Value());
        end;",
        "F",
    );
    assert_eq!(int(r), 9);
}

#[test]
fn record_id_table_no_equality_format_and_variant() {
    // corpus record/TestRecordRecordId.al:14-56, recordref/TestBCMetadataContracts.al:98-150,
    // types/TestVariantExtended.al:123-130
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; A: RecordId; B: RecordId; V: Variant; s: Text; begin
            R.\"No.\" := 'X1'; R.Grp := 1; R.Insert();
            R.\"No.\" := 'X2'; R.Insert();
            R.Get('X1'); A := R.RecordId();
            R.Get('X1'); B := R.RecordId();
            s := Format(A = B) + Format(A.TableNo());
            R.Get('X2'); B := R.RecordId();
            s += Format(A = B) + '|' + Format(A);
            V := A;
            s += '|' + Format(V.IsRecordId());
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "Yes50900No|ALT T: X1|Yes");
}

#[test]
fn recordref_recordid_and_get_by_recordid() {
    // corpus recordref/TestRecordRefKeys.al:35-52 (RecRef.Get(Rec.RecordId())), TestRecordRefGetSet.al:32-50
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; RR: RecordRef; s: Text; begin
            R.\"No.\" := 'X1'; R.Grp := 11; R.Insert();
            R.\"No.\" := 'X2'; R.Grp := 22; R.Insert();
            RR.Open(50900);
            s := Format(RR.Get(R.RecordId())) + Format(RR.Field(3).Value());
            R.Get('X1');
            s += Format(RR.RecordId() = R.RecordId());
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "Yes22No");
}

#[test]
fn field_type_and_class_are_system_options() {
    // corpus fieldref/TestFieldRefMetadata.al:56-98 (Type non-empty, Class = Normal),
    // TestFieldRefEnumKeywordCase.al:95-106 (an enum field's Type formats as 'Option'),
    // tableextensiontrigger/TestTableExtTargetNamespace_Tests.al:119-121 (FieldType::Integer / ::Text)
    let r = run(
        "procedure F(): Text var RR: RecordRef; ft: FieldType; fc: FieldClass; s: Text; begin
            RR.Open(50900);
            ft := RR.Field(3).Type();
            fc := RR.Field(3).Class();
            s := Format(ft) + '/' + Format(fc) + '/' + Format(ft = FieldType::Integer) + Format(fc = FieldClass::Normal) + Format(ft = FieldType::Text);
            s += '|' + Format(RR.Field(5).Type()) + Format(RR.Field(1).Type()) + Format(RR.Field(4).Type()) + Format(RR.Field(7).Type());
            s += '|' + Format(RR.Field(8).Class()) + Format(RR.Field(8).Class() = FieldClass::FlowField);
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "Integer/Normal/YesYesNo|OptionCodeTextBoolean|FlowFieldYes");
}

#[test]
fn gettable_works_on_a_ref_that_was_never_opened_even_through_a_variant() {
    // corpus _fixtures/Assert.al:95-130 (Assert.RecordIsEmpty(RecVariant): `RecRef.GetTable(RecVariant)` on a fresh ref)
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; V: Variant; RR: RecordRef; s: Text; begin
            R.\"No.\" := 'A'; R.Insert();
            R.SetRange(Grp, 5);
            V := R;
            RR.GetTable(V);
            s := Format(RR.Number) + Format(RR.IsEmpty()) + Format(RR.HasFilter()) + RR.GetFilters();
            exit(s);
        end;",
        "F",
    );
    assert_eq!(text(r), "50900YesYesGrp: 5");
}

#[test]
fn field_ref_calc_field_calculates_a_flow_field() {
    // fieldref/TestRelationTargetNamespaceScope.al:63-75: `RecRef.Field(3).CalcField()` makes the
    // FlowField read its count.
    let r = run(
        "procedure F(): Text var T: Record \"ALT T\"; RR: RecordRef; a: Text; begin
            T.\"No.\" := 'A'; T.Insert(); T.\"No.\" := 'B'; T.Insert();
            RR.Open(50902); RR.Init(); RR.Field(1).Value := 'X';
            a := Format(RR.Field(3).Value);
            RR.Field(3).CalcField();
            exit(a + '|' + Format(RR.Field(3).Value));
        end;",
        "F",
    );
    assert_eq!(text(r), "0|2");
}

#[test]
fn field_ref_calc_field_of_a_stored_field_is_refused() {
    let r = run(
        "procedure F(): Text var RR: RecordRef; begin RR.Open(50900); RR.Field(2).CalcField(); exit(''); end;",
        "F",
    );
    assert!(
        matches!(&r, Err(Signal::Unsupported(m)) if m.contains("not a FlowField")),
        "{r:?}"
    );
}
