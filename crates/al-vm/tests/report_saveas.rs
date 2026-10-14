//! `Report.SaveAs(.., ReportFormat::Xml, ..)`, end to end: the dataset a report run writes. Each
//! test names the corpus test that pins it. The fixtures copy the probe in
//! handlers/TestReportSaveAsDatasetContracts.al (tables 68300/68301, report 68302).

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_version::{BcVersion, Target};
use al_vm::{Engine, Session};

const FIXTURES: &str = r#"
table 68300 "Probe Rep Parent"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Name; Text[30]) { }
        field(3; Amt; Decimal) { }
        field(4; D; Date) { }
        field(5; B; Boolean) { }
        field(10; Data; Blob) { }
    }
    keys { key(PK; Id) { Clustered = true; } }
}

table 68301 "Probe Rep Child"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; ParentId; Integer) { }
        field(3; Code; Code[10]) { }
    }
    keys { key(PK; Id) { Clustered = true; } }
}

report 68302 "Probe SaveAs Report"
{
    UsageCategory = None;
    DefaultRenderingLayout = Rdlc;

    dataset
    {
        dataitem(P; "Probe Rep Parent")
        {
            column(PNo; Id) { }
            column(PName; Name) { }
            column(PAmt; Amt) { }
            column(PDate; D) { }
            column(PBool; B) { }

            dataitem(C; "Probe Rep Child")
            {
                DataItemLink = ParentId = field(Id);

                column(CNo; Id) { }
                column(CCode; Code) { }
            }
        }
    }

    requestpage
    {
        layout
        {
            area(Content)
            {
                group(Options)
                {
                    field(Prefix; Prefix) { ApplicationArea = All; }
                }
            }
        }
    }

    rendering
    {
        layout(Rdlc)
        {
            Type = RDLC;
            LayoutFile = './ProbeSaveAsDatasetLayout.rdl';
            Caption = 'Probe layout';
        }
    }

    var
        Prefix: Text;
}

// A report without a layout, whose columns are expressions and whose triggers log.
report 68310 "Probe Plain Report"
{
    ProcessingOnly = false;

    dataset
    {
        dataitem(P; "Probe Rep Parent")
        {
            column(Tag; 'T-' + Format(Id) + '-' + Format(Seen)) { }
            column(Tens; Seen * 10) { }

            trigger OnAfterGetRecord()
            begin
                Seen += 1;
            end;
        }
    }

    trigger OnPreReport()
    begin
        if FailPre then
            Error('plain pre failed');
    end;

    var
        Seen: Integer;
        FailPre: Boolean;

    procedure RowsSeen(): Integer
    begin
        exit(Seen);
    end;

    procedure FailInPre()
    begin
        FailPre := true;
    end;
}

// A report with an RDLC layout whose OnPreReport fails (corpus handlers/TestReportSaveAsRecordRefFilterReport.al).
report 68311 "Probe Layout Guard Report"
{
    DefaultRenderingLayout = Rdlc;

    dataset
    {
        dataitem(P; "Probe Rep Parent")
        {
            column(PNo; Id) { }
        }
    }

    rendering
    {
        layout(Rdlc)
        {
            Type = RDLC;
            LayoutFile = './x.rdl';
        }
    }

    trigger OnPreReport()
    begin
        if P.GetFilters() = '' then
            Error('You must specify one or more filters.');
    end;
}

codeunit 68304 Helpers
{
    procedure Seed()
    var
        Parent: Record "Probe Rep Parent";
        Child: Record "Probe Rep Child";
    begin
        Child.DeleteAll();
        Parent.DeleteAll();
        Parent.Init();
        Parent.Id := 1;
        Parent.Name := 'one <&>';
        Parent.Amt := 12.5;
        Parent.D := DMY2Date(5, 3, 2024);
        Parent.B := true;
        Parent.Insert();
        Parent.Init();
        Parent.Id := 2;
        Parent.Name := 'two';
        Parent.Insert();
        Child.Init();
        Child.Id := 10;
        Child.ParentId := 1;
        Child.Code := 'a';
        Child.Insert();
        Child.Id := 11;
        Child.Insert();
        Child.Id := 12;
        Child.ParentId := 2;
        Child.Code := 'c';
        Child.Insert();
        Commit();
    end;

    // The lines of the stream joined with '|'.
    procedure Lines(var Buf: Record "Probe Rep Parent"): Text
    var
        InStr: InStream;
        Line: Text;
        Result: Text;
    begin
        Buf.CalcFields(Data);
        Buf.Data.CreateInStream(InStr, TextEncoding::UTF8);
        while not InStr.EOS() do begin
            InStr.ReadText(Line);
            Result += Line + '|';
        end;
        exit(Result);
    end;

    procedure BomBytes(var Buf: Record "Probe Rep Parent"): Text
    var
        InStr: InStream;
        A: Byte;
        B: Byte;
        C: Byte;
    begin
        Buf.CalcFields(Data);
        Buf.Data.CreateInStream(InStr);
        InStr.Read(A);
        InStr.Read(B);
        InStr.Read(C);
        exit(Format(A) + '.' + Format(B) + '.' + Format(C));
    end;

    // The dataset the probe report writes, from its DataItems element on, with the tableId/systemId pairs kept.
    procedure Dataset(ReportId: Integer; Parameters: Text): Text
    var
        Buf: Record "Probe Rep Parent";
        OutStr: OutStream;
        Saved: Boolean;
        All: Text;
        At: Integer;
    begin
        Buf.Data.CreateOutStream(OutStr);
        Saved := Report.SaveAs(ReportId, Parameters, ReportFormat::Xml, OutStr);
        if not Saved then
            exit('FALSE:' + GetLastErrorText());
        All := Lines(Buf);
        exit(All);
    end;
}
"#;

fn run_on(bc: Option<&str>, body: &str) -> Result<Value, Signal> {
    let src = format!("{FIXTURES}\ncodeunit 50100 C {{ procedure F(): Text {body} }}");
    let target = bc.map(|v| Target::for_bc(BcVersion::parse(v).unwrap()));
    let apps = vec![AppSource {
        id: "a".into(),
        name: "a".into(),
        dependencies: vec![],
        files: vec![("f.al".into(), src)],
    }];
    let (ws, l) = match &target {
        Some(t) => al_sema::lower::compile_for(apps, &Default::default(), t),
        None => al_sema::lower::compile(apps, &Default::default()),
    };
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi].routines.iter().position(|r| r.name == "F").unwrap();
    if let Some(Some(r)) = blocked.get(oi).and_then(|o| o.get(ri)) {
        return Err(Signal::Unsupported(r.clone()));
    }
    let engine = Engine::with_target(l.program, blocked, target);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn run(body: &str) -> Result<Value, Signal> {
    run_on(None, body)
}

fn f(body: &str) -> String {
    match run(body) {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

fn refusal(body: &str) -> String {
    match run(body) {
        Err(Signal::Unsupported(r)) => r,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Lines given as (indent in spaces, text), joined the way the AL helper joins a stream.
fn lines(ls: &[(usize, &str)]) -> String {
    ls.iter().map(|(n, t)| format!("{}{t}|", " ".repeat(*n))).collect()
}

// handlers/TestReportSaveAsDatasetContracts.al:SaveAs_Xml_DataItemsBlock_ExactText
#[test]
fn the_data_items_block_is_written_exactly() {
    let text = f("var H: Codeunit Helpers; begin H.Seed(); exit(H.Dataset(Report::\"Probe SaveAs Report\", '')); end;");
    let at = text.find("  <DataItems>|").expect("a DataItems block");
    let top = "<DataItem language=\"en-US\" formatRegion=\"en-US\" name=\"P\">";
    let expected = lines(&[
        (2, "<DataItems>"),
        (4, top),
        (6, "<Columns>"),
        (8, "<Column name=\"PNo\">1</Column>"),
        (8, "<Column name=\"PName\">one &lt;&amp;&gt;</Column>"),
        (8, "<Column name=\"PAmt\" decimalformatter=\"#,##0.00\">12.5</Column>"),
        (8, "<Column name=\"PDate\">3/5/2024</Column>"),
        (8, "<Column name=\"PBool\">True</Column>"),
        (6, "</Columns>"),
        (6, "<DataItems>"),
        (8, "<DataItem name=\"C\">"),
        (10, "<Columns>"),
        (12, "<Column name=\"CNo\">10</Column>"),
        (12, "<Column name=\"CCode\">A</Column>"),
        (10, "</Columns>"),
        (8, "</DataItem>"),
        (8, "<DataItem name=\"C\">"),
        (10, "<Columns>"),
        (12, "<Column name=\"CNo\">11</Column>"),
        (12, "<Column name=\"CCode\">A</Column>"),
        (10, "</Columns>"),
        (8, "</DataItem>"),
        (6, "</DataItems>"),
        (4, "</DataItem>"),
        (4, top),
        (6, "<Columns>"),
        (8, "<Column name=\"PNo\">2</Column>"),
        (8, "<Column name=\"PName\">two</Column>"),
        (8, "<Column name=\"PAmt\" decimalformatter=\"#,##0.00\">0</Column>"),
        (8, "<Column name=\"PDate\"></Column>"),
        (8, "<Column name=\"PBool\">False</Column>"),
        (6, "</Columns>"),
        (6, "<DataItems>"),
        (8, "<DataItem name=\"C\">"),
        (10, "<Columns>"),
        (12, "<Column name=\"CNo\">12</Column>"),
        (12, "<Column name=\"CCode\">C</Column>"),
        (10, "</Columns>"),
        (8, "</DataItem>"),
        (6, "</DataItems>"),
        (4, "</DataItem>"),
        (2, "</DataItems>"),
        (0, "</ReportDataSet>"),
    ]);
    assert_eq!(&text[at..], expected);
}

const DATASET: &str = "var H: Codeunit Helpers; begin H.Seed(); exit(H.Dataset(Report::\"Probe SaveAs Report\", '')); end;";

// handlers/TestReportSaveAsDatasetContracts.al:SaveAs_Xml_WritesAByteOrderMarkDeclarationAndHeader (:206-212)
#[test]
fn the_stream_starts_with_a_byte_order_mark_a_declaration_and_the_root_element() {
    assert_eq!(
        f("var H: Codeunit Helpers; Buf: Record \"Probe Rep Parent\"; Out: OutStream; begin
            H.Seed();
            Buf.Data.CreateOutStream(Out);
            Report.SaveAs(Report::\"Probe SaveAs Report\", '', ReportFormat::Xml, Out);
            exit(H.BomBytes(Buf));
        end;"),
        "239.187.191"
    );
    let text = f(DATASET);
    let mut lines = text.split('|');
    assert_eq!(lines.next().unwrap().trim_start_matches('\u{feff}'), "<?xml version=\"1.0\" standalone=\"yes\"?>");
    assert_eq!(
        lines.next().unwrap(),
        "<ReportDataSet name=\"Probe SaveAs Report\" id=\"68302\" language=\"en-US\" formatRegion=\"en-US\">"
    );
}

/// The names of the header elements that carry a value on one line, in order.
fn header_tags(text: &str) -> String {
    let mut tags = String::new();
    for line in text.split('|') {
        let t = line.trim();
        if t == "<DataItems>" {
            break;
        }
        if t.starts_with('<') && !t.starts_with("</") && t.contains("</") {
            tags.push_str(&t[1..t.find('>').unwrap()]);
            tags.push(' ');
        }
    }
    tags
}

// handlers/TestReportSaveAsDatasetContracts.al:SaveAs_Xml_WritesAByteOrderMarkDeclarationAndHeader (:213-220):
// BC 27 and 28 write one list, BC 29 adds ReportCaption and the company information block.
#[test]
fn the_header_lists_its_elements_in_the_order_the_probe_pins() {
    let before = "ExtensionId ExtensionName ExtensionPublisher ExtensionVersion ReportId ReportName AboutThisReportTitle AboutThisReportText ReportHelpLink TenantEntraId EnvironmentName EnvironmentType CompanyName CompanyDisplayName CompanyId UserName DateAndTime Language FormatRegion Year MonthNumber DayNumber Hour Minute ";
    let after = "ExtensionId ExtensionName ExtensionPublisher ExtensionVersion ReportId ReportName ReportCaption AboutThisReportTitle AboutThisReportText ReportHelpLink TenantEntraId EnvironmentName EnvironmentType CompanyName CompanyDisplayName CompanyId UserName DateAndTime Language FormatRegion Year MonthNumber DayNumber Hour Minute CompanyName CompanyDisplayName CompanyAddressLine1 CompanyAddressLine2 CompanyAddressLine3 CompanyAddressLine4 CompanyAddressLine5 CompanyAddressLine6 CompanyAddressLine7 CompanyAddressLine8 CompanyPhone CompanyPhoneCaption CompanyFaxNo CompanyFaxNoCaption CompanyEmail CompanyEmailCaption CompanyHomePage CompanyHomePageCaption CompanyLogo CompanyVATRegistrationNo CompanyVATRegistrationNoCaption CompanyRegistrationNo CompanyRegistrationNoCaption CompanyBankName CompanyBankNameCaption CompanyBankAccountNo CompanyBankAccountNoCaption CompanyBankBranchNo CompanyBankBranchNoCaption CompanyIBAN CompanyIBANCaption CompanyBankSWIFT CompanyBankSWIFTCaption CompanyGiroNo CompanyGiroNoCaption ";
    for bc in ["27.5", "28.5"] {
        let text = match run_on(Some(bc), DATASET) {
            Ok(Value::Text(t)) => t,
            other => panic!("{other:?}"),
        };
        assert_eq!(header_tags(&text), before, "BC {bc}");
    }
    for bc in [Some("29.0"), None] {
        let text = match run_on(bc, DATASET) {
            Ok(Value::Text(t)) => t,
            other => panic!("{other:?}"),
        };
        assert_eq!(header_tags(&text), after, "BC {bc:?}");
    }
}

// handlers/TestReportSaveAsDatasetContracts.al:SaveAs_Xml_WritesAByteOrderMarkDeclarationAndHeader (:221-222)
#[test]
fn the_header_names_the_report_and_the_company() {
    let text = f(DATASET);
    assert!(text.contains("      <ReportName>Probe SaveAs Report</ReportName>|"), "{text}");
    assert!(text.contains("<ReportId>68302</ReportId>|"), "{text}");
    let company = f("begin exit(CompanyName()); end;");
    assert!(text.contains(&format!("<CompanyName>{company}</CompanyName>|")), "{text}");
}

const FILTERED: &str = "<?xml version=\"1.0\" standalone=\"yes\"?><ReportParameters name=\"Probe SaveAs Report\" id=\"68302\"><Options><Field name=\"Prefix\">PFX</Field></Options><DataItems><DataItem name=\"P\">VERSION(1) SORTING(Field1) WHERE(Field1=1(2))</DataItem><DataItem name=\"C\">VERSION(1) SORTING(Field1)</DataItem></DataItems></ReportParameters>";

// handlers/TestReportSaveAsDatasetContracts.al:RunRequestPage_ReturnsTheParametersString_AndSaveAsHonoursItsFilter (:370-378)
#[test]
fn the_parameters_string_filters_the_dataset() {
    let text = f(&format!(
        "var H: Codeunit Helpers; begin H.Seed(); exit(H.Dataset(Report::\"Probe SaveAs Report\", '{FILTERED}')); end;"
    ));
    assert!(text.contains("<Column name=\"PNo\">2</Column>"), "{text}");
    assert!(!text.contains("<Column name=\"PNo\">1</Column>"), "{text}");
    assert!(text.contains("<Column name=\"CNo\">12</Column>"), "{text}");
    assert!(!text.contains("<Column name=\"CNo\">10</Column>"), "{text}");
}

// handlers/TestReportSaveAsDatasetContracts.al:SaveAs_Html_WithAnRdlcLayout_IsRefused
#[test]
fn html_of_a_report_with_an_rdlc_layout_answers_false_with_the_layout_error_and_writes_nothing() {
    assert_eq!(
        f("var H: Codeunit Helpers; Buf: Record \"Probe Rep Parent\"; Out: OutStream; Ok: Boolean; begin
            H.Seed();
            Buf.Data.CreateOutStream(Out);
            ClearLastError();
            Ok := Report.SaveAs(Report::\"Probe SaveAs Report\", '', ReportFormat::Html, Out);
            exit(Format(Ok) + '|' + GetLastErrorText() + '|' + H.Lines(Buf));
        end;"),
        "No|The Html output format is not supported by the current report layout format.|"
    );
}

#[test]
fn a_rendered_format_is_refused_not_guessed() {
    for format in ["Pdf", "Word", "Excel"] {
        let why = refusal(&format!(
            "var H: Codeunit Helpers; Buf: Record \"Probe Rep Parent\"; Out: OutStream; begin
                H.Seed();
                Buf.Data.CreateOutStream(Out);
                Report.SaveAs(Report::\"Probe SaveAs Report\", '', ReportFormat::{format}, Out);
            end;"
        ));
        assert!(why.contains(&format!("as {format}")), "{why}");
    }
}

// handlers/TestReportRunExecution.al:InstanceSaveAsXml_ExecutesTriggersAndBody; the columns read what
// OnAfterGetRecord left (handlers/TestReportFlowFieldColumnAutoCalc.al:FlowFieldColumn_IsCalculatedBeforeOnAfterGetRecord
// reads a column bound to a variable the trigger sets).
#[test]
fn an_instance_saveas_runs_the_triggers_and_the_columns_see_their_result() {
    assert_eq!(
        f("var H: Codeunit Helpers; R: Report \"Probe Plain Report\"; Buf: Record \"Probe Rep Parent\"; Out: OutStream; begin
            H.Seed();
            Clear(R);
            Buf.Data.CreateOutStream(Out);
            R.SaveAs('', ReportFormat::Xml, Out);
            exit(Format(R.RowsSeen()) + '|' + H.Lines(Buf));
        end;")
        .split('|')
        .filter(|l| l.contains("<Column ") || l.len() == 1)
        .map(|l| l.trim().to_string())
        .collect::<Vec<_>>()
        .join(","),
        "2,<Column name=\"Tag\">T-1-1</Column>,<Column name=\"Tens\">10</Column>,<Column name=\"Tag\">T-2-2</Column>,<Column name=\"Tens\">20</Column>"
    );
}

// reportextension/TestReportExtensionTriggers.al:SaveAsXml_ExtPreReportError_FailsTheSave: a report with no
// layout raises the error of its trigger.
#[test]
fn an_error_in_a_report_without_a_layout_reaches_the_caller() {
    match run("var H: Codeunit Helpers; R: Report \"Probe Plain Report\"; Buf: Record \"Probe Rep Parent\"; Out: OutStream; begin
            H.Seed();
            Buf.Data.CreateOutStream(Out);
            R.FailInPre();
            R.SaveAs('', ReportFormat::Xml, Out);
        end;")
    {
        Err(Signal::Error(e)) => assert_eq!(e.message, "plain pre failed"),
        other => panic!("{other:?}"),
    }
}

// handlers/TestReportSaveAsRecordRefFilter.al:TestReport_SaveAs_NoRecordRef_LeavesDataItemUnfiltered: the
// guard in OnPreReport of a report with an RDLC layout fails the SaveAs, which answers false.
#[test]
fn an_error_in_a_report_with_an_rdlc_layout_answers_false() {
    assert_eq!(
        f("var H: Codeunit Helpers; Buf: Record \"Probe Rep Parent\"; Out: OutStream; Ok: Boolean; begin
            H.Seed();
            Buf.Data.CreateOutStream(Out);
            Ok := Report.SaveAs(Report::\"Probe Layout Guard Report\", '', ReportFormat::Xml, Out);
            exit(Format(Ok) + '|' + H.Lines(Buf));
        end;"),
        "No|"
    );
}

// handlers/TestReportSaveAsRecordRefFilter.al:TestReport_SaveAs_FilteredRecordRef_LimitsDatasetToThatRecord
#[test]
fn a_filtered_recordref_limits_the_data_item_over_its_table() {
    let text = f("var H: Codeunit Helpers; Buf: Record \"Probe Rep Parent\"; Parent: Record \"Probe Rep Parent\"; RR: RecordRef; Out: OutStream; Ok: Boolean; begin
            H.Seed();
            Parent.Get(2);
            Parent.SetRecFilter();
            RR.GetTable(Parent);
            Buf.Data.CreateOutStream(Out);
            Ok := Report.SaveAs(Report::\"Probe Layout Guard Report\", '', ReportFormat::Xml, Out, RR);
            exit(Format(Ok) + '|' + H.Lines(Buf));
        end;");
    assert!(text.starts_with("Yes|"), "{text}");
    assert!(text.contains("<Column name=\"PNo\">2</Column>"), "{text}");
    assert!(!text.contains("<Column name=\"PNo\">1</Column>"), "{text}");
}
