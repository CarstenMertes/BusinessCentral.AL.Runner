//! `Report.SaveAs` of a report whose rendering layout is `Type = Custom`: the platform's `CustomDocumentMergerEx` is raised
//! with the layout file, and a subscriber that reads the ObjectPayload is refused (corpus topic #176). The raise itself
//! is the assumption custom-layout-merger; handlers/TestReportLayoutByNameMergerCapture.al pins the subscriber's shape.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

fn fixtures(subscriber_body: &str) -> String {
    format!(
        r#"
table 68400 "CL Row" {{ fields {{ field(1; Id; Integer) {{ }} }} keys {{ key(PK; Id) {{ Clustered = true; }} }} }}
report 68401 "CL Report"
{{
    UsageCategory = None;
    DefaultRenderingLayout = Mine;
    dataset {{ dataitem(R; "CL Row") {{ column(Id; Id) {{ }} }} }}
    rendering {{ layout(Mine) {{ Type = Custom; LayoutFile = './x/mine.cl'; MimeType = 'application/x-cl'; }} }}
}}
codeunit 68402 "CL Sub"
{{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Reporting Triggers", 'CustomDocumentMergerEx', '', false, false)]
    local procedure Merge(ObjectID: Integer; ReportAction: Option SaveAsPdf,SaveAsWord,SaveAsExcel,Preview,Print,SaveAsHtml; ObjectPayload: JsonObject; XmlData: InStream; LayoutData: InStream; var DocumentStream: OutStream; var Success: Boolean)
    var Line: Text; Layout: Text; Probe: JsonToken;
    begin
        while not LayoutData.EOS() do begin LayoutData.ReadText(Line); Layout += Line; end;
        {subscriber_body}
        DocumentStream.WriteText('rendered:' + Layout);
        Success := true;
    end;
}}
table 68403 "CL Blob" {{ fields {{ field(1; Id; Integer) {{ }} field(2; Data; Blob) {{ }} }} keys {{ key(PK; Id) {{ Clustered = true; }} }} }}
codeunit 50100 C
{{
    procedure F(): Text
    var B: Record "CL Blob"; O: OutStream; I: InStream; Text: Text; Ok: Boolean;
    begin
        B.Insert();
        B.Data.CreateOutStream(O);
        Ok := Report.SaveAs(Report::"CL Report", '', ReportFormat::Pdf, O);
        B.Modify();
        B.CalcFields(Data);
        B.Data.CreateInStream(I);
        I.ReadText(Text);
        exit(Format(Ok) + '|' + Text);
    end;
}}
"#
    )
}

fn run(subscriber_body: &str) -> Result<Value, Signal> {
    let apps = vec![AppSource {
        id: "a".into(),
        name: "a".into(),
        dependencies: vec![],
        files: vec![("f.al".into(), fixtures(subscriber_body))],
    }];
    let (ws, mut l) = al_sema::lower::compile(apps, &Default::default());
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    std::sync::Arc::make_mut(&mut l.program).apps[0].layout_files = vec![("x/mine.cl".into(), b"LAYOUT-TEXT".to_vec())];
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
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

#[test]
fn a_subscriber_that_does_not_read_the_payload_renders_the_layout_text() {
    match run("") {
        Ok(Value::Text(t)) => assert_eq!(t.to_string(), "Yes|rendered:LAYOUT-TEXT"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_subscriber_that_reads_the_payload_is_refused() {
    match run("if ObjectPayload.Get('layoutname', Probe) then;") {
        Err(Signal::Unsupported(why)) => assert!(
            why.contains("ObjectPayload of a custom layout merger is not pinned"),
            "{why}"
        ),
        other => panic!("{other:?}"),
    }
}
