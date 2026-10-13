//! XmlPort objects, end to end (Format = Xml). Each test names the corpus test that pins it.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const FIXTURES: &str = r#"
table 60000 "ALT Universal"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(3; "Integer Field"; Integer) { }
        field(5; "Decimal Field"; Decimal) { }
        field(6; "Text Field"; Text[100]) { }
        field(17; "Description Field"; Text[250]) { }
    }
    keys { key(PK; "Entry No.") { } }
}

table 60004 "ALT Parent"
{
    fields { field(1; "Entry No."; Integer) { } field(2; "Code"; Code[20]) { } }
    keys { key(PK; "Entry No.") { } }
}

table 60005 "ALT Child"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; "Parent Entry No."; Integer) { }
        field(3; "Code"; Code[20]) { }
        field(4; "Amount"; Decimal) { }
    }
    keys { key(PK; "Entry No.") { } }
}

table 60008 "ALT Blob"
{
    fields { field(1; "Code"; Code[20]) { } field(3; Data; Blob) { } }
    keys { key(PK; "Code") { } }
}

// corpus _fixtures/xmlports/ALTUniversalXmlPort.al
xmlport 60023 "ALT Universal XmlPort"
{
    Direction = Both; Format = Xml; UseRequestPage = false;
    schema
    {
        textelement(Root)
        {
            XmlName = 'Universals';
            tableelement(Universal; "ALT Universal")
            {
                XmlName = 'Universal';
                fieldelement(EntryNo; Universal."Entry No.") { }
                fieldelement(IntegerValue; Universal."Integer Field") { }
                fieldelement(TextValue; Universal."Text Field") { }
            }
        }
    }
}

// corpus _fixtures/xmlports/ALTParentChildXmlPort.al
xmlport 60024 "ALT Parent Child XmlPort"
{
    Direction = Both; Format = Xml; UseRequestPage = false;
    schema
    {
        textelement(Parents)
        {
            tableelement(Parent; "ALT Parent")
            {
                XmlName = 'Parent';
                fieldelement(ParentEntryNo; Parent."Entry No.") { }
                fieldelement(ParentCode; Parent.Code) { }
                tableelement(Child; "ALT Child")
                {
                    LinkTable = Parent;
                    LinkFields = "Parent Entry No." = FIELD("Entry No.");
                    XmlName = 'Child';
                    fieldattribute(Amount; Child.Amount) { }
                    fieldelement(ChildEntryNo; Child."Entry No.") { }
                    fieldelement(ChildCode; Child.Code) { }
                }
            }
        }
    }
}

// corpus _fixtures/xmlports/ALTVariableXmlPort.al
xmlport 60025 "ALT Variable XmlPort"
{
    Direction = Both; Format = Xml; UseRequestPage = false;
    schema
    {
        textelement(Universals)
        {
            tableelement(Universal; "ALT Universal")
            {
                XmlName = 'Universal';
                textattribute(NoteAttr)
                {
                    trigger OnBeforePassVariable()
                    begin
                        NoteAttr := CopyStr(UpperCase(Universal."Text Field"), 1, MaxStrLen(NoteAttr));
                    end;
                    trigger OnAfterAssignVariable()
                    begin
                        Universal."Description Field" := NoteAttr;
                    end;
                }
                fieldelement(EntryNo; Universal."Entry No.") { }
                fieldelement(IntegerValue; Universal."Integer Field")
                {
                    trigger OnAfterAssignField()
                    begin
                        Universal."Integer Field" := Universal."Integer Field" * 10;
                    end;
                }
                textelement(DisplayText)
                {
                    trigger OnBeforePassVariable()
                    begin
                        DisplayText := StrSubstNo('TXT:%1', Universal."Text Field");
                    end;
                    trigger OnAfterAssignVariable()
                    begin
                        Universal."Text Field" := CopyStr(CopyStr(DisplayText, 5), 1, MaxStrLen(Universal."Text Field"));
                    end;
                }
            }
        }
    }
}

// corpus _fixtures/xmlports/ALTUniversalUpdateXmlPort.al
xmlport 60026 "ALT Universal Update XmlPort"
{
    Direction = Import; Format = Xml; UseRequestPage = false;
    schema
    {
        textelement(Universals)
        {
            tableelement(Universal; "ALT Universal")
            {
                AutoUpdate = true;
                XmlName = 'Universal';
                fieldelement(EntryNo; Universal."Entry No.") { }
                fieldelement(IntegerValue; Universal."Integer Field") { }
            }
        }
    }
}

// corpus _fixtures/xmlports/ALTUniversalReplaceXmlPort.al
xmlport 60027 "ALT Universal Replace XmlPort"
{
    Direction = Import; Format = Xml; UseRequestPage = false;
    schema
    {
        textelement(Universals)
        {
            tableelement(Universal; "ALT Universal")
            {
                AutoReplace = true;
                XmlName = 'Universal';
                fieldelement(EntryNo; Universal."Entry No.") { }
                fieldelement(IntegerValue; Universal."Integer Field") { }
            }
        }
    }
}

// corpus _fixtures/xmlports/ALTUniversalManualXmlPort.al
xmlport 60028 "ALT Universal Manual XmlPort"
{
    Direction = Import; Format = Xml; UseRequestPage = false;
    schema
    {
        textelement(Universals)
        {
            tableelement(Universal; "ALT Universal")
            {
                AutoSave = false;
                AutoUpdate = true;
                XmlName = 'Universal';
                fieldelement(EntryNo; Universal."Entry No.") { }
                fieldelement(IntegerValue; Universal."Integer Field") { }
                fieldelement(TextValue; Universal."Text Field") { }
                trigger OnAfterInitRecord()
                begin
                    Universal."Description Field" := 'init';
                end;
                trigger OnBeforeInsertRecord()
                begin
                    Universal."Description Field" := CopyStr(StrSubstNo('manual-insert-%1', Universal."Text Field"), 1, MaxStrLen(Universal."Description Field"));
                    Universal.Insert();
                end;
                trigger OnAfterInsertRecord()
                var Persisted: Record "ALT Universal";
                begin
                    Persisted.Get(Universal."Entry No.");
                    Persisted."Description Field" := CopyStr(Persisted."Description Field" + '-after', 1, MaxStrLen(Persisted."Description Field"));
                    Persisted.Modify();
                end;
                trigger OnBeforeModifyRecord()
                var Persisted: Record "ALT Universal";
                begin
                    Persisted.Get(Universal."Entry No.");
                    Persisted."Integer Field" := Universal."Integer Field";
                    Persisted."Text Field" := Universal."Text Field";
                    Persisted."Description Field" := 'manual-modify';
                    Persisted.Modify();
                end;
                trigger OnAfterModifyRecord()
                var Persisted: Record "ALT Universal";
                begin
                    Persisted.Get(Universal."Entry No.");
                    Persisted."Description Field" := CopyStr(Persisted."Description Field" + '-after', 1, MaxStrLen(Persisted."Description Field"));
                    Persisted.Modify();
                end;
            }
        }
    }
}

// corpus _fixtures/xmlports/ALTUniversalTemporaryXmlPort.al
xmlport 60029 "ALT Temp Universal XmlPort"
{
    Direction = Import; Format = Xml; UseRequestPage = false;
    schema
    {
        textelement(Universals)
        {
            tableelement(Universal; "ALT Universal")
            {
                UseTemporary = true;
                XmlName = 'Universal';
                fieldelement(EntryNo; Universal."Entry No.") { }
                fieldelement(IntegerValue; Universal."Integer Field") { }
                fieldelement(TextValue; Universal."Text Field") { }
                trigger OnAfterInsertRecord()
                var Persisted: Record "ALT Universal";
                begin
                    Persisted.Init();
                    Persisted.TransferFields(Universal, false);
                    Persisted."Entry No." := Universal."Entry No." + 1000;
                    Persisted."Description Field" := 'temp-copy';
                    Persisted.Insert();
                end;
            }
        }
    }
}

xmlport 60030 "ALT Export Only XmlPort"
{
    Direction = Export; Format = Xml; UseRequestPage = false;
    schema { textelement(Root) { tableelement(Universal; "ALT Universal") { fieldelement(EntryNo; Universal."Entry No.") { } } } }
}

xmlport 60031 "ALT Import Only XmlPort"
{
    Direction = Import; Format = Xml; UseRequestPage = false;
    schema { textelement(Root) { tableelement(Universal; "ALT Universal") { fieldelement(EntryNo; Universal."Entry No.") { } } } }
}

xmlport 60032 "ALT Variable Text XmlPort"
{
    Direction = Export; Format = VariableText; UseRequestPage = false;
    schema { textelement(Root) { tableelement(Universal; "ALT Universal") { fieldelement(EntryNo; Universal."Entry No.") { } } } }
}

xmlport 60033 "ALT Request Page XmlPort"
{
    Direction = Export; Format = Xml; UseRequestPage = true;
    schema { textelement(Root) { tableelement(Universal; "ALT Universal") { fieldelement(EntryNo; Universal."Entry No.") { } } } }
    requestpage { layout { area(Content) { field(Flag; Flag) { } } } }
    var Flag: Boolean;
}

codeunit 50100 Helpers
{
    procedure Row(EntryNo: Integer; IntegerValue: Integer; TextValue: Text; Description: Text)
    var U: Record "ALT Universal";
    begin
        U.Init(); U."Entry No." := EntryNo; U."Integer Field" := IntegerValue;
        U."Text Field" := TextValue; U."Description Field" := Description; U.Insert();
    end;

    procedure Stage(var B: Record "ALT Blob"; var InStr: InStream; Xml: Text)
    var OutStr: OutStream;
    begin
        B.Init(); B.Code := 'TMP'; B.Insert();
        B.Data.CreateOutStream(OutStr);
        OutStr.WriteText(Xml);
        B.Data.CreateInStream(InStr);
    end;

    procedure ReadAll(var B: Record "ALT Blob"): Text
    var InStr: InStream; Segment: Text; All: Text;
    begin
        B.CalcFields(Data);
        B.Data.CreateInStream(InStr);
        while not InStr.EOS() do begin
            InStr.ReadText(Segment);
            All += Segment;
        end;
        exit(All);
    end;
}
"#;

fn run(vars: &str, body: &str) -> Result<Value, Signal> {
    let src = format!("{FIXTURES}\ncodeunit 50101 C {{ procedure F(): Text var {vars} begin {body} end; }}");
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
        .position(|r| r.name == "F")
        .unwrap();
    if let Some(Some(r)) = blocked.get(oi).and_then(|o| o.get(ri)) {
        return Err(Signal::Unsupported(r.clone()));
    }
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn text(vars: &str, body: &str) -> String {
    match run(vars, body) {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

fn refusal(vars: &str, body: &str) -> String {
    match run(vars, body) {
        Err(Signal::Unsupported(r)) => r,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

const EXPORT_VARS: &str = "H: Codeunit Helpers; B: Record \"ALT Blob\"; OutStr: OutStream; P: XmlPort \"ALT Universal XmlPort\"; Ok: Boolean;";

// xmlport/TestXmlPortObject.al:XmlPort_Export_SetDestination_ProducesWellFormedXml
#[test]
fn export_writes_the_root_and_every_row_to_the_destination() {
    let out = text(
        EXPORT_VARS,
        "H.Row(1, 10, 'First', ''); H.Row(2, 20, 'Second', '');
         B.Code := 'XP1'; B.Insert(); B.Data.CreateOutStream(OutStr);
         P.SetDestination(OutStr);
         Ok := P.Export();
         B.Modify();
         exit(Format(Ok) + '|' + H.ReadAll(B));",
    );
    let (ok, xml) = out.split_once('|').unwrap();
    assert_eq!(ok, "Yes");
    assert!(xml.contains("<Universals>"), "{xml}");
    assert!(xml.contains("<EntryNo>1</EntryNo>"), "{xml}");
    assert!(xml.contains("<TextValue>Second</TextValue>"), "{xml}");
}

// xmlport/TestXmlPortObject.al:XmlPort_SetTableView_RestrictsExportedRows
#[test]
fn set_table_view_restricts_the_exported_rows() {
    let out = text(
        &format!("{EXPORT_VARS} U: Record \"ALT Universal\";"),
        "H.Row(1, 10, 'First', ''); H.Row(2, 20, 'Second', '');
         U.SetRange(\"Entry No.\", 2);
         B.Code := 'XP2'; B.Insert(); B.Data.CreateOutStream(OutStr);
         P.SetTableView(U); P.SetDestination(OutStr);
         Ok := P.Export();
         B.Modify();
         exit(H.ReadAll(B));",
    );
    assert!(out.contains("<EntryNo>2</EntryNo>"), "{out}");
    assert!(!out.contains("<EntryNo>1</EntryNo>"), "{out}");
}

// xmlport/TestXmlPortObject.al:XmlPort_Export_StaticWithRecord_RespectsFilters
#[test]
fn the_static_export_respects_the_filters_of_the_given_record() {
    let out = text(
        "H: Codeunit Helpers; B: Record \"ALT Blob\"; OutStr: OutStream; U: Record \"ALT Universal\"; Ok: Boolean;",
        "H.Row(1, 10, 'First', ''); H.Row(2, 20, 'Second', '');
         U.SetRange(\"Entry No.\", 2);
         B.Code := 'XP3'; B.Insert(); B.Data.CreateOutStream(OutStr);
         Ok := XmlPort.Export(60023, OutStr, U);
         B.Modify();
         exit(Format(Ok) + '|' + H.ReadAll(B));",
    );
    assert!(out.starts_with("Yes|"), "{out}");
    assert!(out.contains("<EntryNo>2</EntryNo>"), "{out}");
    assert!(!out.contains("<EntryNo>1</EntryNo>"), "{out}");
}

const IMPORT_VARS: &str = "H: Codeunit Helpers; B: Record \"ALT Blob\" temporary; InStr: InStream; U: Record \"ALT Universal\"; P: XmlPort \"ALT Universal XmlPort\";";
const TWO_ROWS: &str = "'<?xml version=\"1.0\" encoding=\"UTF-8\"?><Universals><Universal><EntryNo>1</EntryNo><IntegerValue>10</IntegerValue><TextValue>First</TextValue></Universal><Universal><EntryNo>2</EntryNo><IntegerValue>20</IntegerValue><TextValue>Second</TextValue></Universal></Universals>'";

// xmlport/TestXmlPortObject.al:XmlPort_Import_SetSource_InsertsRows
#[test]
fn import_through_set_source_inserts_every_row() {
    let out = text(
        IMPORT_VARS,
        &format!(
            "H.Stage(B, InStr, {TWO_ROWS});
             P.SetSource(InStr); P.Import();
             U.Get(2);
             exit(Format(U.Count()) + '|' + Format(U.\"Integer Field\") + '|' + U.\"Text Field\");"
        ),
    );
    assert_eq!(out, "2|20|Second");
}

// xmlport/TestXmlPortObject.al:XmlPort_Import_StaticFromStream_InsertsRows
#[test]
fn the_static_import_inserts_every_row() {
    let out = text(
        IMPORT_VARS,
        &format!("H.Stage(B, InStr, {TWO_ROWS}); XmlPort.Import(60023, InStr); exit(Format(U.Count()));"),
    );
    assert_eq!(out, "2");
}

// xmlport/TestXmlPortObject.al:XmlPort_Import_StaticWithRecord_GivenVariableUnchangedWithoutExplicitGet
#[test]
fn the_record_given_to_the_static_import_only_seeds_the_table_view() {
    let out = text(
        IMPORT_VARS,
        "H.Stage(B, InStr, '<Universals><Universal><EntryNo>1</EntryNo><IntegerValue>10</IntegerValue><TextValue>First</TextValue></Universal></Universals>');
         XmlPort.Import(60023, InStr, U);
         exit(Format(U.\"Entry No.\") + '|' + Format(U.\"Integer Field\") + '|' + Format(U.Count()));",
    );
    assert_eq!(out, "0|0|1");
}

// xmlport/TestXmlPortAdvanced.al:XmlPort_NestedTableElements_ExportIncludesChildRecordsAndAttribute
#[test]
fn a_nested_table_element_exports_its_linked_rows_and_field_attributes() {
    let out = text(
        "H: Codeunit Helpers; B: Record \"ALT Blob\"; OutStr: OutStream; P: XmlPort \"ALT Parent Child XmlPort\"; Pa: Record \"ALT Parent\"; C: Record \"ALT Child\";",
        "Pa.\"Entry No.\" := 1; Pa.Code := 'P1'; Pa.Insert();
         C.\"Entry No.\" := 10; C.\"Parent Entry No.\" := 1; C.Code := 'C10'; C.Amount := 15; C.Insert();
         C.\"Entry No.\" := 11; C.\"Parent Entry No.\" := 1; C.Code := 'C11'; C.Amount := 25; C.Insert();
         C.\"Entry No.\" := 12; C.\"Parent Entry No.\" := 2; C.Code := 'C12'; C.Amount := 35; C.Insert();
         B.Code := 'XPA1'; B.Insert(); B.Data.CreateOutStream(OutStr);
         P.SetDestination(OutStr); P.Export(); B.Modify();
         exit(H.ReadAll(B));",
    );
    assert!(out.contains("<ParentEntryNo>1</ParentEntryNo>"), "{out}");
    assert!(out.contains("<ChildEntryNo>10</ChildEntryNo>"), "{out}");
    assert!(out.contains("<ChildEntryNo>11</ChildEntryNo>"), "{out}");
    assert!(!out.contains("<ChildEntryNo>12</ChildEntryNo>"), "{out}");
    // Amount is a Decimal: BC writes it with two decimal places (the test's own comment).
    assert!(out.contains("Amount=\"15.00\""), "{out}");
}

// xmlport/TestXmlPortAdvanced.al:XmlPort_NestedTableElements_ImportCreatesParentAndChildren
#[test]
fn a_nested_import_creates_the_children_with_the_link_field_of_their_parent() {
    let out = text(
        "H: Codeunit Helpers; B: Record \"ALT Blob\" temporary; InStr: InStream; Pa: Record \"ALT Parent\"; C: Record \"ALT Child\";",
        "H.Stage(B, InStr, '<Parents><Parent><ParentEntryNo>1</ParentEntryNo><ParentCode>P1</ParentCode><Child Amount=\"15.00\"><ChildEntryNo>10</ChildEntryNo><ChildCode>C10</ChildCode></Child><Child Amount=\"25.00\"><ChildEntryNo>11</ChildEntryNo><ChildCode>C11</ChildCode></Child></Parent></Parents>');
         XmlPort.Import(60024, InStr);
         C.Get(10);
         exit(Format(Pa.Count()) + '|' + Format(C.Count()) + '|' + Format(C.\"Parent Entry No.\") + '|' + Format(C.Amount));",
    );
    assert_eq!(out, "1|2|1|15");
}

// xmlport/TestXmlPortAdvanced.al:XmlPort_TextVariableTriggers_ExportManipulatesTextAndAttributes
#[test]
fn before_pass_variable_shapes_exported_text_and_attributes() {
    let out = text(
        "H: Codeunit Helpers; B: Record \"ALT Blob\"; OutStr: OutStream; P: XmlPort \"ALT Variable XmlPort\";",
        "H.Row(1, 2, 'First', '');
         B.Code := 'XPA3'; B.Insert(); B.Data.CreateOutStream(OutStr);
         P.SetDestination(OutStr); P.Export(); B.Modify();
         exit(H.ReadAll(B));",
    );
    assert!(out.contains("NoteAttr=\"FIRST\""), "{out}");
    assert!(out.contains("<DisplayText>TXT:First</DisplayText>"), "{out}");
}

// xmlport/TestXmlPortAdvanced.al:XmlPort_TextVariableTriggers_ImportMutatesAssignedValues
#[test]
fn after_assign_triggers_rewrite_what_an_import_stores() {
    let out = text(
        "H: Codeunit Helpers; B: Record \"ALT Blob\" temporary; InStr: InStream; U: Record \"ALT Universal\";",
        "H.Stage(B, InStr, '<Universals><Universal NoteAttr=\"ALPHA\"><EntryNo>1</EntryNo><IntegerValue>3</IntegerValue><DisplayText>TXT:alpha</DisplayText></Universal></Universals>');
         XmlPort.Import(60025, InStr);
         U.Get(1);
         exit(Format(U.\"Integer Field\") + '|' + U.\"Text Field\" + '|' + U.\"Description Field\");",
    );
    assert_eq!(out, "30|alpha|ALPHA");
}

fn partial(entry: i32, int: i32) -> String {
    format!(
        "'<Universals><Universal><EntryNo>{entry}</EntryNo><IntegerValue>{int}</IntegerValue></Universal></Universals>'"
    )
}

// xmlport/TestXmlPortAdvanced.al:XmlPort_AutoUpdate_UpdatesSpecifiedFieldsAndPreservesOthers
#[test]
fn auto_update_changes_only_the_fields_present_in_the_import() {
    let out = text(
        IMPORT_VARS,
        &format!(
            "H.Row(1, 10, 'Keep', 'KeepDesc');
             H.Stage(B, InStr, {});
             XmlPort.Import(60026, InStr);
             U.Get(1);
             exit(Format(U.\"Integer Field\") + '|' + U.\"Text Field\" + '|' + U.\"Description Field\");",
            partial(1, 99)
        ),
    );
    assert_eq!(out, "99|Keep|KeepDesc");
}

// xmlport/TestXmlPortAdvanced.al:XmlPort_AutoReplace_ReinitializesOmittedFields
#[test]
fn auto_replace_resets_the_fields_the_import_omits() {
    let out = text(
        IMPORT_VARS,
        &format!(
            "H.Row(1, 10, 'Keep', 'KeepDesc');
             H.Stage(B, InStr, {});
             XmlPort.Import(60027, InStr);
             U.Get(1);
             exit(Format(U.\"Integer Field\") + '|' + U.\"Text Field\" + '|' + U.\"Description Field\" + '|' + Format(U.Count()));",
            partial(1, 99)
        ),
    );
    assert_eq!(out, "99|||1");
}

fn full(entry: i32, int: i32, txt: &str) -> String {
    format!(
        "'<Universals><Universal><EntryNo>{entry}</EntryNo><IntegerValue>{int}</IntegerValue><TextValue>{txt}</TextValue></Universal></Universals>'"
    )
}

// xmlport/TestXmlPortAdvanced.al:XmlPort_AutoSaveFalse_OnBeforeAndAfterInsertControlPersistence
#[test]
fn with_auto_save_off_the_insert_triggers_decide_what_is_stored() {
    let out = text(
        IMPORT_VARS,
        &format!(
            "H.Stage(B, InStr, {}); XmlPort.Import(60028, InStr); U.Get(1); exit(U.\"Description Field\");",
            full(1, 10, "First")
        ),
    );
    assert_eq!(out, "manual-insert-First-after");
}

// xmlport/TestXmlPortAdvanced.al:XmlPort_AutoSaveFalse_OnBeforeAndAfterModifyControlPersistence
#[test]
fn with_auto_save_off_the_modify_triggers_decide_what_is_stored() {
    let out = text(
        IMPORT_VARS,
        &format!(
            "H.Row(1, 10, 'Before', 'Existing'); H.Stage(B, InStr, {}); XmlPort.Import(60028, InStr); U.Get(1);
             exit(Format(U.\"Integer Field\") + '|' + U.\"Text Field\" + '|' + U.\"Description Field\");",
            full(1, 42, "After")
        ),
    );
    assert_eq!(out, "42|After|manual-modify-after");
}

// xmlport/TestXmlPortAdvanced.al:XmlPort_UseTemporary_RequiresManualPersistence
#[test]
fn a_temporary_table_element_stores_nothing_in_the_real_table() {
    let out = text(
        IMPORT_VARS,
        &format!(
            "H.Stage(B, InStr, {}); XmlPort.Import(60029, InStr);
             if U.Get(1) then exit('real row 1 exists');
             U.Get(1001);
             exit(Format(U.\"Integer Field\") + '|' + U.\"Text Field\" + '|' + U.\"Description Field\");",
            full(1, 10, "Temp")
        ),
    );
    assert_eq!(out, "10|Temp|temp-copy");
}

// xmlport/TestXmlPortImportWriteTx.al:StaticGuardedImport_WithUncommittedWrite_ReturnsFalse_AndImportsNothing
#[test]
fn a_guarded_static_import_with_a_pending_write_returns_false_and_imports_nothing() {
    let out = text(
        &format!("{IMPORT_VARS} Ok: Boolean;"),
        &format!(
            "Commit(); H.Stage(B, InStr, {TWO_ROWS}); H.Row(9, 0, '', '');
             Ok := XmlPort.Import(60023, InStr);
             U.SetFilter(\"Entry No.\", '1..2'); exit(Format(Ok) + '|' + Format(U.Count()));"
        ),
    );
    assert_eq!(out, "No|0");
}

// xmlport/TestXmlPortImportWriteTx.al:InstanceGuardedImport_WithUncommittedWrite_IsRefused_AndImportsNothing
#[test]
fn a_guarded_instance_import_with_a_pending_write_is_refused_with_an_error() {
    let out = text(
        &format!("{IMPORT_VARS} Ok: Boolean;"),
        &format!(
            "Commit(); H.Stage(B, InStr, {TWO_ROWS}); H.Row(9, 0, '', '');
             P.SetSource(InStr);
             asserterror Ok := P.Import();
             U.SetFilter(\"Entry No.\", '1..2'); exit(GetLastErrorText() + '|' + Format(Ok) + '|' + Format(U.Count()));"
        ),
    );
    assert!(out.contains("the transaction is stopped"), "{out}");
    assert!(out.ends_with("|No|0"), "{out}");
}

// xmlport/TestXmlPortImportWriteTx.al:StaticStatementImport_WithUncommittedWrite_IsAllowed_AndImportsRows
// and InstanceStatementImport_WithUncommittedWrite_IsAllowed_AndImportsRows
#[test]
fn the_statement_form_joins_the_callers_transaction() {
    let out = text(
        IMPORT_VARS,
        &format!(
            "Commit(); H.Stage(B, InStr, {TWO_ROWS}); H.Row(9, 0, '', '');
             P.SetSource(InStr); P.Import();
             exit(Format(U.Count()));"
        ),
    );
    assert_eq!(out, "3");
}

// xmlport/TestXmlPortImportWriteTx.al:InstanceGuardedImport_AfterCommit_ReturnsTrue_AndImportsRows
// and GuardedImport_LeavingImportedRows_DoesNotBlockTheNextGuardedImport
#[test]
fn a_guarded_import_commits_its_own_transaction() {
    let out = text(
        &format!("{IMPORT_VARS} Ok: Boolean; Ok2: Boolean;"),
        &format!(
            "Commit(); H.Stage(B, InStr, {TWO_ROWS});
             P.SetSource(InStr); Ok := P.Import();
             exit(Format(Ok) + '|' + Format(U.Count()) + '|' + Format(Database.IsInWriteTransaction()));"
        ),
    );
    assert_eq!(out, "Yes|2|No");
}

// xmlport/TestXmlPortImportWriteTx.al:StaticGuardedImport_FailingSecondElement_ReturnsFalse_AndRollsBackTheFirstRow
// and InstanceGuardedImport_FailingSecondElement_ReturnsFalse_AndRollsBackTheFirstRow
#[test]
fn a_failing_guarded_import_returns_false_and_rolls_back_its_rows() {
    let bad = "'<Universals><Universal><EntryNo>9321</EntryNo><IntegerValue>10</IntegerValue><TextValue>First</TextValue></Universal><Universal><EntryNo>9322</EntryNo><IntegerValue>NotAnInteger</IntegerValue><TextValue>Second</TextValue></Universal></Universals>'";
    for call in [
        "Ok := XmlPort.Import(60023, InStr);",
        "P.SetSource(InStr); Ok := P.Import();",
    ] {
        let out = text(
            &format!("{IMPORT_VARS} Ok: Boolean;"),
            &format!(
                "H.Row(1, 0, '', ''); Commit(); H.Stage(B, InStr, {bad}); {call}
                 exit(Format(Ok) + '|' + Format(U.Count()) + '|' + Format(U.Get(1)));"
            ),
        );
        assert_eq!(out, "No|1|Yes", "{call}");
    }
}

// The statement form leaves the failure to the caller (xmlport/TestXmlPortImportWriteTx.al does not
// pin its text, so the failing statement import is refused).
#[test]
fn a_failing_statement_import_is_refused_because_its_error_text_is_not_pinned() {
    let r = refusal(
        IMPORT_VARS,
        "H.Stage(B, InStr, '<Universals><Universal><EntryNo>1</EntryNo><IntegerValue>x</IntegerValue></Universal></Universals>'); XmlPort.Import(60023, InStr); exit('');",
    );
    assert!(r.contains("not pinned"), "{r}");
}

// xmlport/TestXmlPortPrecompiledObject.al:PrecompiledXmlPort_DeclaredExportDirection_RefusesImport
// (the message is a platform resource string the corpus does not pin).
#[test]
fn a_direction_mismatch_is_refused_until_its_message_is_pinned() {
    let r = refusal(
        "B: Record \"ALT Blob\" temporary; H: Codeunit Helpers; InStr: InStream; P: XmlPort \"ALT Export Only XmlPort\";",
        "H.Stage(B, InStr, '<Root/>'); P.SetSource(InStr); P.Import(); exit('');",
    );
    assert!(r.contains("Direction"), "{r}");
    let r = refusal(
        "B: Record \"ALT Blob\"; OutStr: OutStream; P: XmlPort \"ALT Import Only XmlPort\";",
        "B.Code := 'X'; B.Insert(); B.Data.CreateOutStream(OutStr); P.SetDestination(OutStr); P.Export(); exit('');",
    );
    assert!(r.contains("Direction"), "{r}");
}

#[test]
fn xmlports_the_runtime_cannot_run_are_refused_before_they_start() {
    let r = refusal("P: XmlPort \"ALT Variable Text XmlPort\";", "exit('');");
    assert!(r.contains("VariableText"), "{r}");
    let r = refusal("P: XmlPort \"ALT Request Page XmlPort\";", "exit('');");
    assert!(r.contains("request page"), "{r}");
}

// The text of a failed import is not pinned by the corpus, so reading it is refused rather than
// answered with an invented sentence (hard rule 3); the pinned parts (false, rollback) still hold.
#[test]
fn the_last_error_of_a_failed_guarded_import_cannot_be_read() {
    let bad = "'<Universals><Universal><EntryNo>1</EntryNo><IntegerValue>x</IntegerValue></Universal></Universals>'";
    for read in ["GetLastErrorText()", "GetLastErrorCode()"] {
        let r = refusal(
            &format!("{IMPORT_VARS} Ok: Boolean;"),
            &format!("H.Stage(B, InStr, {bad}); Ok := XmlPort.Import(60023, InStr); exit({read});"),
        );
        assert!(r.contains("not pinned"), "{r}");
    }
    // Without a read the import still reports false.
    let out = text(
        &format!("{IMPORT_VARS} Ok: Boolean;"),
        &format!("H.Stage(B, InStr, {bad}); Ok := XmlPort.Import(60023, InStr); exit(Format(Ok));"),
    );
    assert_eq!(out, "No");
}
